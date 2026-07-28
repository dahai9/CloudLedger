use std::{net::IpAddr, time::Duration};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::{TurnstileConfig, DEFAULT_TURNSTILE_VERIFY_URL};

const EXPECTED_ACTION: &str = "admin-login";

#[derive(Debug, Error)]
pub enum TurnstileError {
    #[error("turnstile verification token is required")]
    TokenRequired,
    #[error("turnstile verification was rejected")]
    Rejected,
    #[error("turnstile verification service is unavailable")]
    Unavailable,
}

#[derive(Debug, Clone)]
pub struct TurnstileVerifier {
    site_key: Option<String>,
    secret_key: Option<String>,
    verify_url: String,
    client: reqwest::Client,
}

impl Default for TurnstileVerifier {
    fn default() -> Self {
        Self::disabled()
    }
}

impl TurnstileVerifier {
    pub fn from_config(config: &TurnstileConfig) -> anyhow::Result<Self> {
        let site_key = nonempty_value(&config.site_key);
        let secret_key = nonempty_value(&config.secret_key);
        match (&site_key, &secret_key) {
            (Some(_), Some(_)) | (None, None) => {}
            _ => anyhow::bail!(
                "security.turnstile.site_key and security.turnstile.secret_key must be configured together"
            ),
        }
        let verify_url = nonempty_value(&config.verify_url)
            .ok_or_else(|| anyhow::anyhow!("security.turnstile.verify_url cannot be empty"))?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .build()?;
        Ok(Self {
            site_key,
            secret_key,
            verify_url,
            client,
        })
    }

    pub fn disabled() -> Self {
        Self {
            site_key: None,
            secret_key: None,
            verify_url: DEFAULT_TURNSTILE_VERIFY_URL.to_string(),
            client: reqwest::Client::new(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.site_key.is_some() && self.secret_key.is_some()
    }

    pub fn site_key(&self) -> Option<&str> {
        self.site_key.as_deref()
    }

    pub async fn verify(&self, token: &str, remote_ip: IpAddr) -> Result<(), TurnstileError> {
        let Some(secret_key) = self.secret_key.as_deref() else {
            return Ok(());
        };
        let token = token.trim();
        if token.is_empty() {
            return Err(TurnstileError::TokenRequired);
        }
        let form = TurnstileVerificationRequest {
            secret: secret_key,
            response: token,
            remote_ip: remote_ip.to_string(),
        };
        let response = self
            .client
            .post(&self.verify_url)
            .form(&form)
            .send()
            .await
            .map_err(|_| TurnstileError::Unavailable)?;
        if !response.status().is_success() {
            return Err(TurnstileError::Unavailable);
        }
        let verification = response
            .json::<TurnstileVerificationResponse>()
            .await
            .map_err(|_| TurnstileError::Unavailable)?;
        if verification.success && verification.action.as_deref() == Some(EXPECTED_ACTION) {
            Ok(())
        } else {
            Err(TurnstileError::Rejected)
        }
    }
}

#[derive(Serialize)]
struct TurnstileVerificationRequest<'a> {
    secret: &'a str,
    response: &'a str,
    #[serde(rename = "remoteip")]
    remote_ip: String,
}

#[derive(Deserialize)]
struct TurnstileVerificationResponse {
    success: bool,
    #[serde(default)]
    action: Option<String>,
}

fn nonempty_value(value: &str) -> Option<String> {
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn disabled_verifier_allows_local_development() {
        TurnstileVerifier::disabled()
            .verify("", IpAddr::V4(std::net::Ipv4Addr::LOCALHOST))
            .await
            .expect("disabled verification");
    }

    #[test]
    fn parses_successful_and_rejected_responses() {
        let success: TurnstileVerificationResponse =
            serde_json::from_str(r#"{"success":true,"action":"admin-login"}"#)
                .expect("success response");
        let rejected: TurnstileVerificationResponse =
            serde_json::from_str(r#"{"success":false,"error-codes":["invalid-input-response"]}"#)
                .expect("rejected response");
        assert!(success.success);
        assert_eq!(success.action.as_deref(), Some(EXPECTED_ACTION));
        assert!(!rejected.success);
    }

    #[tokio::test]
    async fn verifies_action_with_remote_service() {
        use axum::{routing::post, Json, Router};
        use serde_json::json;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock turnstile server");
        let address = listener.local_addr().expect("mock server address");
        let app = Router::new()
            .route(
                "/ok",
                post(|| async { Json(json!({"success": true, "action": EXPECTED_ACTION})) }),
            )
            .route(
                "/wrong-action",
                post(|| async { Json(json!({"success": true, "action": "other-action"})) }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve mock turnstile response");
        });

        let verifier = |path: &str| TurnstileVerifier {
            site_key: Some("test-site-key".to_string()),
            secret_key: Some("test-secret-key".to_string()),
            verify_url: format!("http://{address}/{path}"),
            client: reqwest::Client::new(),
        };
        let remote_ip = "192.0.2.10".parse().expect("remote IP");

        verifier("ok")
            .verify("valid-token", remote_ip)
            .await
            .expect("accepted turnstile response");
        assert!(matches!(
            verifier("wrong-action")
                .verify("valid-token", remote_ip)
                .await,
            Err(TurnstileError::Rejected)
        ));
        assert!(matches!(
            verifier("ok").verify("", remote_ip).await,
            Err(TurnstileError::TokenRequired)
        ));

        server.abort();
    }
}
