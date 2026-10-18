use super::protocol::{self, LoginAttempt, Model, Tokens, API_BASE};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::File,
    path::{Path, PathBuf},
};

#[derive(Default, Serialize, Deserialize)]
struct Store {
    host_id: String,
    accounts: Vec<Account>,
}

#[derive(Serialize, Deserialize)]
struct Account {
    client_id: String,
    subject: String,
    email: String,
    access_token: String,
    refresh_token: String,
    id_token: String,
    scope: String,
    expires_at: i64,
}

#[derive(Serialize)]
pub struct AccountInfo {
    pub account_id: String,
    pub provider: String,
    pub label: String,
    pub logged_in: bool,
}

impl Account {
    fn info(&self) -> AccountInfo {
        AccountInfo {
            account_id: self.client_id.clone(),
            provider: provider_name(&self.client_id),
            // Client IDs distinguish same-email workspace registrations.
            label: format!(
                "{} · {}",
                if self.email.is_empty() {
                    "ChatGPT"
                } else {
                    &self.email
                },
                self.client_id
            ),
            logged_in: !self.access_token.is_empty(),
        }
    }
    fn update(&mut self, tokens: Tokens) -> Result<(), String> {
        let scope = tokens.scope.unwrap_or_else(|| self.scope.clone());
        // Save rotated credentials even when permission was removed; subsequent
        // requests fail closed at require_scope rather than reuse stale tokens.
        self.scope = scope;
        self.access_token = protect(&tokens.access_token)?;
        if let Some(refresh) = tokens.refresh_token {
            self.refresh_token = protect(&refresh)?;
        }
        if let Some(id) = tokens.id_token {
            self.id_token = protect(&id)?;
        }
        self.expires_at = super::super::oauth::now_unix().saturating_add(tokens.expires_in);
        Ok(())
    }
}

fn protect(token: &str) -> Result<String, String> {
    let encrypted = crate::cookies::encrypt_secret(token);
    #[cfg(windows)]
    if !token.is_empty() && !encrypted.starts_with("enc:") {
        return Err("Could not protect ChatGPT credentials with Windows DPAPI".into());
    }
    // Unix credential files are owner-only, including on replacement.
    Ok(encrypted)
}
fn reveal(token: &str) -> Result<String, String> {
    crate::cookies::decrypt_secret(token)
        .ok_or_else(|| "Cannot decrypt ChatGPT credentials; sign in again".into())
}

struct LockedStore {
    _lock: File,
    path: PathBuf,
    data: Store,
}
impl LockedStore {
    fn open(config: &Path) -> Result<Self, String> {
        let dir = config
            .parent()
            .ok_or("Missing ChatGPT configuration directory")?;
        let path = dir.join("chatgpt-accounts.json");
        let mut options = std::fs::OpenOptions::new();
        options.create(true).read(true).write(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options
            .open(dir.join("chatgpt-accounts.lock"))
            .map_err(|_| "Could not open ChatGPT credential lock")?;
        lock.lock_exclusive()
            .map_err(|_| "Could not lock ChatGPT credentials")?;
        let data = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| "Invalid ChatGPT credential file; existing file was preserved")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Store::default(),
            Err(_) => return Err("Could not read ChatGPT credentials".into()),
        };
        Ok(Self {
            _lock: lock,
            path,
            data,
        })
    }
    fn save(&self) -> Result<(), String> {
        // Enforce permissions before the shared atomic writer copies them.
        #[cfg(unix)]
        if self.path.exists() {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600))
                .map_err(|_| "Could not protect ChatGPT credential file")?;
        }
        let json = serde_json::to_string_pretty(&self.data)
            .map_err(|_| "Could not encode ChatGPT credentials")?;
        crate::config::write_provider_config(&self.path, &json)
            .map_err(|_| "Could not save ChatGPT credentials".into())
    }
    fn account_mut(&mut self, id: &str) -> Result<&mut Account, String> {
        self.data
            .accounts
            .iter_mut()
            .find(|a| a.client_id == id)
            .ok_or_else(|| "ChatGPT account not found".into())
    }
}

pub fn accounts(config: &Path) -> Result<Vec<AccountInfo>, String> {
    Ok(LockedStore::open(config)?
        .data
        .accounts
        .iter()
        .map(Account::info)
        .collect())
}

pub fn prepare_login(
    config: &Path,
    port: u16,
    account: Option<&str>,
) -> Result<LoginAttempt, String> {
    let mut store = LockedStore::open(config)?;
    if store.data.host_id.is_empty() {
        store.data.host_id = format!("urn:uuid:{}", uuid::Uuid::new_v4());
        store.save()?;
    }
    let subject = if let Some(id) = account {
        Some(store.account_mut(id)?.subject.clone())
    } else {
        None
    };
    Ok(LoginAttempt::new(
        &store.data.host_id,
        port,
        account,
        subject,
    ))
}

pub fn finish_login(
    config: &Path,
    attempt: LoginAttempt,
    params: HashMap<String, String>,
    cancelled: &std::sync::atomic::AtomicBool,
) -> Result<AccountInfo, String> {
    let (client_id, code) = attempt.callback(&params)?;
    let (tokens, identity) = attempt.exchange(&client_id, &code)?;
    let mut store = LockedStore::open(config)?;
    if cancelled.load(std::sync::atomic::Ordering::Acquire) {
        return Err("ChatGPT sign-in cancelled".into());
    }
    if let Some(existing) = store
        .data
        .accounts
        .iter()
        .find(|a| a.client_id == client_id)
    {
        if existing.subject != identity.sub {
            return Err("ChatGPT registration identity changed".into());
        }
    }
    let mut account = Account {
        client_id,
        subject: identity.sub,
        email: identity.email.unwrap_or_default(),
        access_token: String::new(),
        refresh_token: String::new(),
        id_token: String::new(),
        scope: String::new(),
        expires_at: 0,
    };
    account.update(tokens)?;
    let info = account.info();
    store
        .data
        .accounts
        .retain(|a| a.client_id != account.client_id);
    store.data.accounts.push(account);
    store.save()?;
    Ok(info)
}

pub fn fresh_token(config: &Path, id: &str) -> Result<String, String> {
    fresh_token_with(config, id, protocol::refresh)
}

fn fresh_token_with(
    config: &Path,
    id: &str,
    refresh_tokens: impl FnOnce(&str, &str) -> Result<Tokens, String>,
) -> Result<String, String> {
    let mut store = LockedStore::open(config)?;
    let account = store.account_mut(id)?;
    protocol::require_scope(&account.scope)?;
    let access = reveal(&account.access_token)?;
    if super::super::oauth::token_is_fresh(
        Some(account.expires_at),
        &access,
        super::super::oauth::now_unix(),
    ) {
        return Ok(access);
    }
    let refresh = reveal(&account.refresh_token)?;
    if refresh.is_empty() {
        return Err("Sign in with ChatGPT to continue".into());
    }
    let tokens = refresh_tokens(&account.client_id, &refresh)?;
    account.update(tokens)?;
    let access = reveal(&account.access_token)?;
    let scope = account.scope.clone();
    store.save()?;
    protocol::require_scope(&scope)?;
    Ok(access)
}

/// Returns false when local sign-out succeeded but remote revocation was not confirmed.
pub fn logout(config: &Path, id: &str) -> Result<bool, String> {
    let mut store = LockedStore::open(config)?;
    let account = store.account_mut(id)?;
    let refresh = reveal(&account.refresh_token).unwrap_or_default();
    let revoked = refresh.is_empty() || protocol::revoke(&account.client_id, &refresh);
    account.access_token.clear();
    account.refresh_token.clear();
    account.id_token.clear();
    account.expires_at = 0;
    store.save()?;
    Ok(revoked)
}

pub fn provider_name(id: &str) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(id.as_bytes());
    format!(
        "custom-chatgpt-{}",
        hash[..12]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}

pub fn sync_models(config: &Path, id: &str) -> Result<Vec<Model>, String> {
    let token = fresh_token(config, id)?;
    let models = protocol::models(&token)?;
    // Recheck session after I/O so logout cannot silently reconnect a provider.
    let store = LockedStore::open(config)?;
    let account = store
        .data
        .accounts
        .iter()
        .find(|a| a.client_id == id && !a.access_token.is_empty())
        .ok_or("Sign in with ChatGPT to continue")?;
    write_provider(config, &account.info(), &models)?;
    Ok(models)
}

fn write_provider(config: &Path, account: &AccountInfo, models: &[Model]) -> Result<(), String> {
    let _guard = crate::config::lock_provider_config();
    let text =
        std::fs::read_to_string(config).map_err(|_| "Could not read provider configuration")?;
    let mut doc: toml::Value = text.parse().map_err(|_| "Invalid provider configuration")?;
    let root = doc.as_table_mut().ok_or("Invalid provider configuration")?;
    let entries = root
        .entry("providers")
        .or_insert_with(|| toml::Value::Array(vec![]))
        .as_array_mut()
        .ok_or("Invalid provider list")?;
    let previous = entries
        .iter()
        .find(|p| p.get("name").and_then(|v| v.as_str()) == Some(&account.provider))
        .cloned();
    let mut provider = previous.unwrap_or_else(|| toml::Value::Table(Default::default()));
    let map = provider.as_table_mut().ok_or("Invalid ChatGPT provider")?;
    for (key, value) in [
        ("name", account.provider.as_str()),
        ("display_name", account.label.as_str()),
        ("provider_type", "custom"),
        ("api_key", ""),
        ("base_url", API_BASE),
    ] {
        map.insert(key.into(), toml::Value::String(value.into()));
    }
    // No authorization URL or token is stored in exportable configuration.
    map.insert(
        "oauth".into(),
        toml::Value::try_from(super::super::ProviderOAuth {
            chatgpt_account: Some(account.account_id.clone()),
            ..Default::default()
        })
        .map_err(|_| "Could not encode ChatGPT provider")?,
    );
    let old_models = map
        .get("models")
        .and_then(|m| m.as_array())
        .cloned()
        .unwrap_or_default();
    map.insert(
        "models".into(),
        toml::Value::Array(
            models
                .iter()
                .map(|m| {
                    old_models
                        .iter()
                        .find(|old| old.get("id").and_then(|v| v.as_str()) == Some(&m.slug))
                        .cloned()
                        .unwrap_or_else(|| {
                            let mut value = toml::value::Table::new();
                            value.insert("id".into(), toml::Value::String(m.slug.clone()));
                            // No guessed context window or modality capabilities.
                            toml::Value::Table(value)
                        })
                })
                .collect(),
        ),
    );
    entries.retain(|p| p.get("name").and_then(|v| v.as_str()) != Some(&account.provider));
    entries.push(provider);
    crate::config::write_provider_config(
        config,
        &toml::to_string_pretty(&doc).map_err(|_| "Could not encode providers")?,
    )
    .map_err(|_| "Could not save ChatGPT models".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_is_stable_and_credentials_do_not_enter_provider_settings() {
        let dir = std::env::temp_dir().join(format!("nuphus-chatgpt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("providers.toml");
        std::fs::write(&config, "model = ''\n").unwrap();
        let a = prepare_login(&config, 1234, None).unwrap();
        let b = prepare_login(&config, 1235, None).unwrap();
        let host = |s: &str| {
            reqwest::Url::parse(s)
                .unwrap()
                .query_pairs()
                .find(|(k, _)| k == "ext_agent_host_id")
                .unwrap()
                .1
                .into_owned()
        };
        assert_eq!(host(&a.authorize_url), host(&b.authorize_url));
        assert_ne!(a.state, b.state);
        let info = AccountInfo {
            account_id: "oaiapp_test".into(),
            provider: provider_name("oaiapp_test"),
            label: "Account".into(),
            logged_in: true,
        };
        write_provider(
            &config,
            &info,
            &[Model {
                slug: "model-a".into(),
                display_name: "Model A".into(),
            }],
        )
        .unwrap();
        let registry = crate::config::ModelRegistry::from_toml(config.to_str().unwrap()).unwrap();
        let p = &registry.providers[0];
        assert!(p.api_key.is_empty());
        assert_eq!(
            p.oauth.as_ref().unwrap().chatgpt_account.as_deref(),
            Some("oaiapp_test")
        );
        assert!(fresh_token(&config, "oaiapp_missing").is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(dir.join("chatgpt-accounts.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn rotating_refresh_is_serialized_and_signed_out_sessions_fail_closed() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        let dir =
            std::env::temp_dir().join(format!("nuphus-chatgpt-refresh-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("providers.toml");
        {
            let mut store = LockedStore::open(&config).unwrap();
            store.data.accounts.push(Account {
                client_id: "oaiapp_test".into(),
                subject: "sub".into(),
                email: String::new(),
                access_token: protect("expired").unwrap(),
                refresh_token: protect("refresh-1").unwrap(),
                id_token: String::new(),
                scope: "chatgpt.tokens.use.direct".into(),
                expires_at: 1,
            });
            store.save().unwrap();
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let workers: Vec<_> = (0..2)
            .map(|_| {
                let path = config.clone();
                let calls = calls.clone();
                std::thread::spawn(move || {
                    fresh_token_with(&path, "oaiapp_test", |client, refresh| {
                        assert_eq!(client, "oaiapp_test");
                        assert_eq!(refresh, "refresh-1");
                        calls.fetch_add(1, Ordering::SeqCst);
                        Ok(Tokens {
                            access_token: "new-access".into(),
                            refresh_token: Some("refresh-2".into()),
                            id_token: None,
                            expires_in: 3600,
                            scope: None,
                            token_type: "Bearer".into(),
                        })
                    })
                    .unwrap()
                })
            })
            .collect();
        for worker in workers {
            assert_eq!(worker.join().unwrap(), "new-access");
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        {
            let mut store = LockedStore::open(&config).unwrap();
            let account = store.account_mut("oaiapp_test").unwrap();
            assert_eq!(reveal(&account.refresh_token).unwrap(), "refresh-2");
            account.access_token.clear();
            account.refresh_token.clear();
            store.save().unwrap();
        }
        assert!(fresh_token_with(&config, "oaiapp_test", |_, _| panic!(
            "must not refresh signed-out account"
        ))
        .is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
