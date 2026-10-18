use base64::Engine;
use jsonwebtoken::{decode, decode_header, jwk::JwkSet, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

pub const API_BASE: &str = "https://api.openai.com/v1";
const ISSUER: &str = "https://auth.openai.com";
const AUTHORIZE: &str = "https://auth.openai.com/api/accounts/authorize";
const TOKEN: &str = "https://auth.openai.com/api/accounts/oauth/token";
const SCOPE: &str = "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const DYNAMIC_CLIENT: &str = "dynamic_agent_client";

// Do not derive Debug: these structures hold credentials or a PKCE verifier.
pub struct LoginAttempt {
    pub state: String,
    nonce: String,
    verifier: String,
    redirect_uri: String,
    pub client_id: String,
    pub expected_subject: Option<String>,
    pub authorize_url: String,
}

#[derive(Deserialize)]
pub(super) struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    pub expires_in: i64,
    pub scope: Option<String>,
    pub token_type: String,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Model {
    pub slug: String,
    pub display_name: String,
}

#[derive(Clone, Deserialize)]
pub(super) struct Identity {
    pub sub: String,
    pub email: Option<String>,
    nonce: String,
    iat: u64,
}

fn random() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

impl LoginAttempt {
    pub(super) fn new(
        host: &str,
        port: u16,
        client: Option<&str>,
        subject: Option<String>,
    ) -> Self {
        let state = random();
        let nonce = random();
        let verifier = random();
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(verifier.as_bytes()));
        let redirect_uri = format!("http://127.0.0.1:{port}/auth/callback");
        let client_id = client.unwrap_or(DYNAMIC_CLIENT).to_string();
        let mut url = reqwest::Url::parse(AUTHORIZE).expect("constant URL");
        url.query_pairs_mut().extend_pairs([
            ("response_type", "code"),
            ("client_id", &client_id),
            ("redirect_uri", &redirect_uri),
            ("scope", SCOPE),
            ("resource", API_BASE),
            ("state", &state),
            ("nonce", &nonce),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("ext_agent_host_id", host),
        ]);
        if client.is_none() {
            url.query_pairs_mut()
                .append_pair("agent_name_hint", "Nuphus");
        }
        // No ID-token hint in browser-facing URLs. Returning users select their
        // account and we still verify that sub matches the saved registration.
        Self {
            state,
            nonce,
            verifier,
            redirect_uri,
            client_id,
            expected_subject: subject,
            authorize_url: url.to_string(),
        }
    }

    pub(super) fn callback(
        &self,
        params: &HashMap<String, String>,
    ) -> Result<(String, String), String> {
        if params.get("state") != Some(&self.state) {
            return Err("ChatGPT callback state mismatch".into());
        }
        if params.contains_key("error") {
            return Err("ChatGPT authorization was declined; please try signing in again".into());
        }
        let client = params
            .get("client_id")
            .map(String::as_str)
            .unwrap_or(&self.client_id);
        if client == DYNAMIC_CLIENT || !client.starts_with("oaiapp_") {
            return Err("ChatGPT registration did not return an issued client ID".into());
        }
        if self.client_id != DYNAMIC_CLIENT && client != self.client_id {
            return Err("ChatGPT callback changed the selected registration".into());
        }
        let code = params
            .get("code")
            .filter(|v| !v.is_empty())
            .ok_or("ChatGPT callback is missing an authorization code")?;
        Ok((client.to_string(), code.clone()))
    }

    pub(super) fn exchange(
        &self,
        client_id: &str,
        code: &str,
    ) -> Result<(Tokens, Identity), String> {
        let http = http()?;
        let tokens = token_request(
            &http,
            &[
                ("grant_type", "authorization_code"),
                ("client_id", client_id),
                ("code", code),
                ("code_verifier", &self.verifier),
                ("redirect_uri", &self.redirect_uri),
                ("resource", API_BASE),
            ],
        )?;
        require_scope(tokens.scope.as_deref().unwrap_or(""))?;
        let id_token = tokens
            .id_token
            .as_deref()
            .ok_or("ChatGPT did not return an ID token")?;
        let jwks: JwkSet = get_json(&http, &format!("{ISSUER}/.well-known/jwks.json"), None)?;
        let identity = verify_identity(id_token, &jwks, client_id, &self.nonce)?;
        if self
            .expected_subject
            .as_ref()
            .is_some_and(|sub| *sub != identity.sub)
        {
            return Err("ChatGPT identity does not match the selected account".into());
        }
        Ok((tokens, identity))
    }
}

pub(super) fn verify_identity(
    token: &str,
    jwks: &JwkSet,
    client: &str,
    nonce: &str,
) -> Result<Identity, String> {
    let header = decode_header(token).map_err(|_| "Invalid ChatGPT ID token")?;
    if header.alg != Algorithm::RS256 {
        return Err("Unsupported ChatGPT ID-token signing algorithm".into());
    }
    let key = header
        .kid
        .as_deref()
        .and_then(|kid| jwks.find(kid))
        .ok_or("Unknown ChatGPT signing key")?;
    let key = DecodingKey::from_jwk(key).map_err(|_| "Invalid ChatGPT signing key")?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[ISSUER]);
    validation.set_audience(&[client]);
    validation.set_required_spec_claims(&["sub", "exp", "iat", "iss", "aud"]);
    validation.leeway = 5;
    let identity = decode::<Identity>(token, &key, &validation)
        .map_err(|_| "ChatGPT ID-token signature or claims validation failed")?
        .claims;
    if identity.sub.is_empty()
        || identity.nonce != nonce
        || identity.iat > super::super::oauth::now_unix() as u64 + 5
    {
        return Err("ChatGPT ID-token subject or nonce mismatch".into());
    }
    Ok(identity)
}

pub(super) fn require_scope(scope: &str) -> Result<(), String> {
    if scope
        .split_whitespace()
        .any(|s| s == "chatgpt.tokens.use.direct")
    {
        Ok(())
    } else {
        Err("ChatGPT plan usage was not granted. Sign in again and enable plan usage.".into())
    }
}

pub(super) fn http() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not create ChatGPT HTTP client".into())
}

fn token_request(
    http: &reqwest::blocking::Client,
    form: &[(&str, &str)],
) -> Result<Tokens, String> {
    let response = http
        .post(TOKEN)
        .form(form)
        .send()
        .map_err(|_| "ChatGPT token request failed; check your connection")?;
    if !response.status().is_success() {
        // Never echo arbitrary error bodies (which may contain credentials).
        return Err(format!(
            "ChatGPT token request failed (HTTP {}). Sign in again if authorization expired.",
            response.status().as_u16()
        ));
    }
    let tokens: Tokens = response
        .json()
        .map_err(|_| "Invalid ChatGPT token response")?;
    if tokens.access_token.is_empty()
        || tokens.expires_in <= 0
        || !tokens.token_type.eq_ignore_ascii_case("bearer")
    {
        return Err("Invalid ChatGPT token credentials or expiry".into());
    }
    Ok(tokens)
}

pub(super) fn refresh(client: &str, token: &str) -> Result<Tokens, String> {
    token_request(
        &http()?,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", client),
            ("refresh_token", token),
            ("resource", API_BASE),
        ],
    )
}

fn get_json<T: serde::de::DeserializeOwned>(
    http: &reqwest::blocking::Client,
    url: &str,
    token: Option<&str>,
) -> Result<T, String> {
    let mut req = http.get(url);
    if let Some(token) = token {
        req = req.bearer_auth(token);
    }
    let response = req
        .send()
        .map_err(|_| "ChatGPT request failed; check your connection")?;
    if !response.status().is_success() {
        return Err(format!(
            "ChatGPT request failed (HTTP {})",
            response.status().as_u16()
        ));
    }
    response
        .json()
        .map_err(|_| "Invalid ChatGPT response".into())
}

pub(super) fn models(token: &str) -> Result<Vec<Model>, String> {
    let value: serde_json::Value = get_json(&http()?, &format!("{API_BASE}/models"), Some(token))?;
    parse_models(&value)
}

pub(super) fn parse_models(value: &serde_json::Value) -> Result<Vec<Model>, String> {
    value
        .get("models")
        .and_then(|v| v.as_array())
        .ok_or("Missing ChatGPT model catalog")?
        .iter()
        .filter(|m| m["visibility"] == "list")
        .map(|m| {
            let model: Model =
                serde_json::from_value(m.clone()).map_err(|_| "Invalid ChatGPT model entry")?;
            if model.slug.is_empty() {
                return Err("Empty ChatGPT model slug".into());
            }
            Ok(model)
        })
        .collect()
}

pub(super) fn revoke(client: &str, refresh: &str) -> bool {
    let Ok(http) = http() else {
        return false;
    };
    let Ok(discovery) = get_json::<serde_json::Value>(
        &http,
        &format!("{ISSUER}/.well-known/openid-configuration"),
        None,
    ) else {
        return false;
    };
    let Some(endpoint) = discovery["revocation_endpoint"].as_str() else {
        return false;
    };
    let Ok(url) = reqwest::Url::parse(endpoint) else {
        return false;
    };
    if url.scheme() != "https"
        || url.host_str() != Some("auth.openai.com")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return false;
    }
    http.post(url)
        .form(&[
            ("token", refresh),
            ("token_type_hint", "refresh_token"),
            ("client_id", client),
        ])
        .send()
        .is_ok_and(|r| r.status().as_u16() == 200)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authorization_and_callback_bind_registration() {
        let attempt = LoginAttempt::new("urn:uuid:host", 54321, None, None);
        let url = reqwest::Url::parse(&attempt.authorize_url).unwrap();
        let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(query["client_id"], DYNAMIC_CLIENT);
        assert_eq!(query["agent_name_hint"], "Nuphus");
        assert_eq!(
            query["redirect_uri"],
            "http://127.0.0.1:54321/auth/callback"
        );
        let mut callback = HashMap::from([
            ("state".into(), attempt.state.clone()),
            ("code".into(), "code".into()),
        ]);
        assert!(attempt.callback(&callback).is_err());
        callback.insert("client_id".into(), "oaiapp_a".into());
        assert_eq!(attempt.callback(&callback).unwrap().0, "oaiapp_a");
        callback.insert("state".into(), "forged".into());
        assert!(attempt.callback(&callback).is_err());
        let returning =
            LoginAttempt::new("urn:uuid:host", 54322, Some("oaiapp_b"), Some("sub".into()));
        assert!(!returning.authorize_url.contains("agent_name_hint"));
        callback.insert("state".into(), returning.state.clone());
        assert!(returning.callback(&callback).is_err());
        callback.remove("client_id");
        assert_eq!(returning.callback(&callback).unwrap().0, "oaiapp_b");
        callback.insert("error".into(), "access_denied".into());
        assert!(returning.callback(&callback).is_err());
    }
    #[test]
    fn scope_and_catalog_are_account_specific() {
        assert!(require_scope("openid chatgpt.tokens.use.direct email").is_ok());
        assert!(require_scope("chatgpt.tokens.use.direct.fake").is_err());
        let list = parse_models(&serde_json::json!({"models":[
            {"slug":"b","display_name":"B","visibility":"list"},
            {"slug":"hidden","display_name":"Hidden","visibility":"hide"},
            {"slug":"a","display_name":"A","visibility":"list"}
        ]}))
        .unwrap();
        assert_eq!(
            list.iter().map(|m| m.slug.as_str()).collect::<Vec<_>>(),
            vec!["b", "a"]
        );
        assert!(parse_models(&serde_json::json!({"data":[]})).is_err());
    }
    #[test]
    fn verifies_signature_issuer_audience_expiry_nonce_and_subject() {
        // Public, synthetic fixture keys are used only in tests.
        let key = jsonwebtoken::EncodingKey::from_rsa_pem(include_bytes!("test_key.pem")).unwrap();
        let jwks: JwkSet = serde_json::from_str(include_str!("test_jwks.json")).unwrap();
        let now = super::super::super::oauth::now_unix();
        let claims = serde_json::json!({"iss":ISSUER,"aud":"oaiapp_test","sub":"user-1",
            "iat":now,"exp":now+3600,"nonce":"nonce-1","email":"test@example.invalid"});
        let sign = |claims: &serde_json::Value| {
            let mut header = jsonwebtoken::Header::new(Algorithm::RS256);
            header.kid = Some("test-only".into());
            jsonwebtoken::encode(&header, claims, &key).unwrap()
        };
        let token = sign(&claims);
        assert!(verify_identity(&token, &jwks, "oaiapp_test", "nonce-1").is_ok());
        assert!(verify_identity(&token, &jwks, "oaiapp_other", "nonce-1").is_err());
        assert!(verify_identity(&token, &jwks, "oaiapp_test", "wrong-nonce").is_err());
        for (field, bad) in [
            ("iss", serde_json::json!("https://attacker.invalid")),
            ("sub", serde_json::json!("")),
            ("exp", serde_json::json!(now - 60)),
            ("iat", serde_json::json!(now + 60)),
        ] {
            let mut altered = claims.clone();
            altered[field] = bad;
            assert!(
                verify_identity(&sign(&altered), &jwks, "oaiapp_test", "nonce-1").is_err(),
                "{field}"
            );
        }
        let mut missing_iat = claims.clone();
        missing_iat.as_object_mut().unwrap().remove("iat");
        assert!(verify_identity(&sign(&missing_iat), &jwks, "oaiapp_test", "nonce-1").is_err());
        let parts: Vec<_> = token.split('.').collect();
        let forged = format!(
            "{}.{}.{}",
            parts[0],
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"sub":"attacker"}"#),
            parts[2]
        );
        assert!(verify_identity(&forged, &jwks, "oaiapp_test", "nonce-1").is_err());
    }
}
