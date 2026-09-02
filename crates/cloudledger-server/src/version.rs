use axum::{
    extract::{Request, State},
    http::{HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use semver::Version;
use serde::Serialize;

use crate::ServerState;

pub const CLIENT_VERSION_HEADER: &str = "X-CloudLedger-Client-Version";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientVersionResponse {
    pub current_version: String,
    pub min_supported_version: String,
    pub download_url: String,
    pub update_required: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpgradeRequiredResponse {
    error: &'static str,
    code: &'static str,
    current_version: String,
    min_supported_version: String,
    download_url: String,
}

pub async fn client_version(
    State(state): State<ServerState>,
    request: Request,
) -> Json<ClientVersionResponse> {
    let update_required = request
        .headers()
        .get(CLIENT_VERSION_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Version::parse(value.trim()).ok())
        .map(|version| version < minimum_version(&state))
        .unwrap_or(true);
    Json(ClientVersionResponse {
        current_version: state.client_version.as_ref().clone(),
        min_supported_version: state.min_supported_client_version.as_ref().clone(),
        download_url: state.client_download_url.as_ref().clone(),
        update_required,
    })
}

pub async fn enforce_client_version(
    State(state): State<ServerState>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    if request.method() == axum::http::Method::OPTIONS
        || matches!(
            path,
            "/health" | "/ready" | "/client/version" | "/auth/bootstrap"
        )
    {
        return next.run(request).await;
    }

    let client_version = request
        .headers()
        .get(CLIENT_VERSION_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Version::parse(value.trim()).ok());
    let minimum = minimum_version(&state);
    let missing_version_allowed = client_version.is_none() && minimum == Version::new(0, 0, 0);
    if !missing_version_allowed && client_version.is_none_or(|version| version < minimum) {
        let mut response = (
            StatusCode::UPGRADE_REQUIRED,
            Json(UpgradeRequiredResponse {
                error: "客户端版本过低，请升级后继续使用",
                code: "client_update_required",
                current_version: state.client_version.as_ref().clone(),
                min_supported_version: state.min_supported_client_version.as_ref().clone(),
                download_url: state.client_download_url.as_ref().clone(),
            }),
        )
            .into_response();
        response
            .headers_mut()
            .insert("cache-control", HeaderValue::from_static("no-store"));
        return response;
    }
    next.run(request).await
}

fn minimum_version(state: &ServerState) -> Version {
    Version::parse(state.min_supported_client_version.as_ref())
        .expect("validated minimum client version")
}

#[cfg(test)]
mod tests {
    use semver::Version;

    #[test]
    fn semantic_versions_order_prerelease_before_stable() {
        assert!(Version::parse("0.1.13-alpha.2").unwrap() < Version::parse("0.1.13").unwrap());
        assert!(Version::parse("0.1.14").unwrap() > Version::parse("0.1.13").unwrap());
    }
}
