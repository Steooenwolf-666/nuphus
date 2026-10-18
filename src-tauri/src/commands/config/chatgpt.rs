//! Desktop-only SIWC commands. Tokens, PKCE and nonce never cross IPC.
use nuphus::config::chatgpt::{self, AccountInfo, Model};
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};
use tokio::sync::{Mutex, Notify};
static LOGIN_CANCELLED: AtomicBool = AtomicBool::new(false);

fn login_guard() -> &'static Mutex<()> {
    static GUARD: OnceLock<Mutex<()>> = OnceLock::new();
    GUARD.get_or_init(|| Mutex::new(()))
}
fn cancel_signal() -> &'static Notify {
    static SIGNAL: OnceLock<Notify> = OnceLock::new();
    SIGNAL.get_or_init(Notify::new)
}
fn path() -> Result<std::path::PathBuf, String> {
    super::get_config_path().ok_or_else(|| "Provider configuration not found".into())
}

#[tauri::command(async)]
pub fn chatgpt_accounts() -> Result<Vec<AccountInfo>, String> {
    chatgpt::accounts(&path()?)
}

#[tauri::command(async)]
pub fn chatgpt_cancel_login() {
    LOGIN_CANCELLED.store(true, Ordering::Release);
    cancel_signal().notify_waiters();
}

#[tauri::command]
pub async fn chatgpt_login(account_id: Option<String>) -> Result<AccountInfo, String> {
    let _guard = login_guard()
        .try_lock()
        .map_err(|_| "A ChatGPT sign-in is already in progress")?;
    LOGIN_CANCELLED.store(false, Ordering::Release);
    let cancelled = cancel_signal().notified();
    tokio::pin!(cancelled);
    cancelled.as_mut().enable();
    let config = path()?;
    // Bind before opening the browser; the exact same URI is used in exchange.
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|_| "Could not start the ChatGPT loopback callback")?;
    let port = listener
        .local_addr()
        .map_err(|_| "Could not read callback port")?
        .port();
    let prepare_path = config.clone();
    let attempt = tokio::task::spawn_blocking(move || {
        chatgpt::prepare_login(&prepare_path, port, account_id.as_deref())
    })
    .await
    .map_err(|_| "Could not prepare ChatGPT sign-in")??;
    let state = attempt.state.clone();
    let (tx, rx) = tokio::sync::oneshot::channel::<HashMap<String, String>>();
    let sender = Arc::new(std::sync::Mutex::new(Some(tx)));
    let router = axum::Router::new().route(
        "/auth/callback",
        axum::routing::get(
            move |axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>| {
                let sender = sender.clone();
                let state = state.clone();
                async move {
                    if params.get("state") != Some(&state) {
                        return (
                            axum::http::StatusCode::BAD_REQUEST,
                            "Invalid authorization callback",
                        );
                    }
                    if let Some(tx) = sender.lock().ok().and_then(|mut s| s.take()) {
                        let _ = tx.send(params);
                        (
                            axum::http::StatusCode::OK,
                            "Return to Nuphus to complete sign-in.",
                        )
                    } else {
                        (
                            axum::http::StatusCode::CONFLICT,
                            "This callback has already been used.",
                        )
                    }
                }
            },
        ),
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let mut server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });
    let opened = crate::commands::external::open_external(attempt.authorize_url.clone());
    let callback = if opened.is_err() {
        Err("Could not open your browser for ChatGPT sign-in".to_string())
    } else {
        tokio::select! {
            result = tokio::time::timeout(std::time::Duration::from_secs(300), rx) => {
                match result {
                    Ok(Ok(params)) => Ok(params),
                    Ok(Err(_)) => Err("ChatGPT callback was interrupted".to_string()),
                    Err(_) => Err("ChatGPT sign-in timed out".to_string()),
                }
            }
            _ = &mut cancelled => Err("ChatGPT sign-in cancelled".to_string()),
        }
    };
    let _ = shutdown_tx.send(());
    // Do not let an idle loopback connection hold sign-in open indefinitely.
    if tokio::time::timeout(std::time::Duration::from_secs(1), &mut server)
        .await
        .is_err()
    {
        server.abort();
    }
    let params = callback?;
    tokio::task::spawn_blocking(move || {
        chatgpt::finish_login(&config, attempt, params, &LOGIN_CANCELLED)
    })
    .await
    .map_err(|_| "ChatGPT sign-in task failed")?
}

/** Blocking HTTP and token refresh must run outside Tauri's async runtime workers. */
#[tauri::command]
pub async fn chatgpt_models(account_id: String) -> Result<Vec<Model>, String> {
    let config = path()?;
    tokio::task::spawn_blocking(move || chatgpt::sync_models(&config, &account_id))
        .await
        .map_err(|_| "ChatGPT model synchronization task failed")?
}

#[tauri::command]
pub async fn chatgpt_logout(account_id: String) -> Result<bool, String> {
    let _guard = login_guard()
        .try_lock()
        .map_err(|_| "Cancel the active ChatGPT sign-in before signing out")?;
    let config = path()?;
    tokio::task::spawn_blocking(move || chatgpt::logout(&config, &account_id))
        .await
        .map_err(|_| "ChatGPT sign-out task failed")?
}
