//! Sign in with ChatGPT for locally hosted open-source clients.
//!
//! Credentials live separately from exportable provider settings. Only opaque
//! account references enter providers.toml. All credential transactions acquire
//! an OS file lock (including refresh) so rotating refresh tokens cannot race.
//! Protocol: https://developers.openai.com/siwc/token-sharing-open-source
mod protocol;
mod store;

pub use protocol::{LoginAttempt, Model, API_BASE};
pub use store::{
    accounts, finish_login, fresh_token, logout, prepare_login, sync_models, AccountInfo,
};

/// A reference, never a bearer credential. Resolved for every HTTP request.
#[derive(Debug, Clone)]
pub struct SessionRef {
    pub config_path: std::path::PathBuf,
    pub account_id: String,
}
