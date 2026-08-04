pub mod admin;
pub mod app_api;
pub mod audit;
pub mod auth;
pub mod auth_routes;
pub mod config;
pub mod login_protection;
pub mod platform_auth;
pub mod request_security;
pub mod state;
pub mod storage;
pub mod sync;
pub mod turnstile;

use crate::{login_protection::SecurityRateKind, request_security::RequestContext};
use axum::{
    extract::{DefaultBodyLimit, Request, State},
    http::{
        header::{
            AUTHORIZATION, CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, REFERRER_POLICY,
            X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS,
        },
        HeaderValue, Method, StatusCode,
    },
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
pub use state::ServerState;
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    trace::TraceLayer,
};
use uuid::Uuid;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub status: &'static str,
    pub service: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadyResponse {
    pub status: &'static str,
    pub service: &'static str,
    pub server_id: Uuid,
    pub sync_model: &'static str,
    pub public_ledger_authority: &'static str,
}

pub fn router(state: ServerState) -> Router {
    let cors = cors_layer(&state);
    let request_security = state.request_security.clone();
    let rate_limit_state = state.clone();
    Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/sync/ping", get(sync::sync_ping))
        .route("/auth/login", post(auth_routes::legacy_upgrade))
        .route("/auth/refresh", post(auth_routes::legacy_upgrade))
        .route(
            "/auth/me",
            get(auth_routes::me).patch(auth_routes::update_me),
        )
        .route("/auth/logout", post(auth_routes::legacy_upgrade))
        .route("/auth/security", get(auth_routes::auth_security))
        .route("/auth/tauri/login", post(auth_routes::tauri_login))
        .route("/auth/tauri/refresh", post(auth_routes::tauri_refresh))
        .route("/auth/tauri/logout", post(auth_routes::tauri_logout))
        .route(
            "/auth/tauri/me",
            get(auth_routes::me).patch(auth_routes::update_me),
        )
        .route("/auth/web/login", post(auth_routes::web_login))
        .route("/auth/web/refresh", post(auth_routes::web_refresh))
        .route("/auth/web/logout", post(auth_routes::web_logout))
        .route("/app/overview", get(app_api::overview))
        .route("/app/analytics", get(app_api::financial_analysis))
        .route(
            "/app/transactions",
            get(app_api::transactions_for_month).post(app_api::create_transaction),
        )
        .route("/app/categories", post(app_api::create_category))
        .route("/app/approvals/decide", post(app_api::decide_approval))
        .route(
            "/app/payments/mark-paid",
            post(app_api::mark_transaction_paid),
        )
        .route(
            "/app/payments/confirm-receipt",
            post(app_api::confirm_transaction_receipt),
        )
        .with_state(state)
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(cors)
        .layer(middleware::from_fn_with_state(
            rate_limit_state,
            security_rate_limits,
        ))
        .layer(middleware::from_fn_with_state(
            request_security,
            request_security::resolve_request_context,
        ))
        .layer(TraceLayer::new_for_http())
}

async fn security_rate_limits(
    State(state): State<ServerState>,
    request: Request,
    next: Next,
) -> Response {
    let context = request.extensions().get::<RequestContext>().copied();
    let has_bearer = request.headers().contains_key(AUTHORIZATION);
    let anonymous_probe = request.uri().path().starts_with("/app/") && !has_bearer;
    if let (Some(context), true) = (context, anonymous_probe) {
        match state
            .login_protection
            .check_security_request(SecurityRateKind::AnonymousProbe, context.client_ip)
            .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return auth_routes::ApiError::from_auth(error).into_response(),
            Err(error) => return auth_routes::ApiError::from_storage(error).into_response(),
        }
    }

    let response = next.run(request).await;
    if let (Some(context), true, StatusCode::UNAUTHORIZED) =
        (context, has_bearer, response.status())
    {
        match state
            .login_protection
            .check_security_request(SecurityRateKind::InvalidBearer, context.client_ip)
            .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return auth_routes::ApiError::from_auth(error).into_response(),
            Err(error) => return auth_routes::ApiError::from_storage(error).into_response(),
        }
    }
    response
}

pub fn admin_router(state: ServerState) -> Router {
    let request_security = state.request_security.clone();
    admin::router(state)
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(middleware::from_fn(admin_security_headers))
        .layer(middleware::from_fn_with_state(
            request_security,
            request_security::resolve_request_context,
        ))
        .layer(TraceLayer::new_for_http())
}

fn cors_layer(state: &ServerState) -> CorsLayer {
    let origins = state
        .cors_allowed_origins
        .iter()
        .map(|origin| HeaderValue::from_str(origin).expect("validated CORS origin"))
        .collect::<Vec<_>>();
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET, Method::POST, Method::PATCH])
        .allow_headers([AUTHORIZATION, CONTENT_TYPE])
        .allow_credentials(true)
}

async fn admin_security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; base-uri 'none'; connect-src 'self' https://challenges.cloudflare.com; frame-ancestors 'none'; frame-src https://challenges.cloudflare.com; form-action 'self'; img-src 'self' data:; object-src 'none'; script-src 'self' 'unsafe-inline' https://challenges.cloudflare.com; style-src 'self' 'unsafe-inline'",
        ),
    );
    headers.insert(
        "permissions-policy",
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    response
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        service: "cloudledger-server",
    })
}

async fn ready(State(state): State<ServerState>) -> Json<ReadyResponse> {
    Json(ReadyResponse {
        status: "ready",
        service: "cloudledger-server",
        server_id: state.server_id,
        sync_model: "cloud_authoritative_public_ledgers",
        public_ledger_authority: "server",
    })
}

#[cfg(test)]
mod http_tests {
    use std::{fs, net::SocketAddr};

    use axum::{
        body::{to_bytes, Body},
        extract::ConnectInfo,
        http::{header, Request},
    };
    use serde_json::json;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{AccountKind, AdminCreateUserInput};

    fn request(method: Method, uri: &str, body: serde_json::Value) -> Request<Body> {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-forwarded-for", "198.51.100.9")
            .header("x-forwarded-proto", "https")
            .body(Body::from(body.to_string()))
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(
            "127.0.0.1:42000".parse::<SocketAddr>().unwrap(),
        ));
        request
    }

    #[tokio::test]
    async fn legacy_routes_cors_cookie_and_turnstile_threshold_are_enforced() {
        let data_dir = std::env::temp_dir().join(format!("cloudledger-http-{}", Uuid::new_v4()));
        let mut state = ServerState::load(data_dir.clone()).expect("load test state");
        state.web_login_enabled = true;
        state
            .auth_service
            .lock()
            .unwrap()
            .create_or_update_admin_user(AdminCreateUserInput {
                user_id: Uuid::new_v4(),
                display_name: "Web User".to_string(),
                email: Some("web@example.com".to_string()),
                phone: None,
                password: Some("correct-password".to_string()),
                account_kind: AccountKind::Business,
                organization_id: None,
            })
            .unwrap();
        let app = router(state);

        let legacy = app
            .clone()
            .oneshot(request(Method::POST, "/auth/login", json!({})))
            .await
            .unwrap();
        assert_eq!(legacy.status(), StatusCode::UPGRADE_REQUIRED);
        let legacy_body = to_bytes(legacy.into_body(), 4096).await.unwrap();
        assert!(String::from_utf8_lossy(&legacy_body).contains("client_upgrade_required"));

        let security = app
            .clone()
            .oneshot(request(Method::GET, "/auth/security", json!({})))
            .await
            .unwrap();
        assert_eq!(security.status(), StatusCode::OK);
        let security_body = to_bytes(security.into_body(), 4096).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&security_body).unwrap(),
            json!({"turnstileEnabled": false, "turnstileSiteKey": null})
        );

        let preflight = Request::builder()
            .method(Method::OPTIONS)
            .uri("/app/overview")
            .header(header::ORIGIN, "tauri://localhost")
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
            .body(Body::empty())
            .unwrap();
        let preflight = app.clone().oneshot(preflight).await.unwrap();
        assert_eq!(
            preflight
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .and_then(|value| value.to_str().ok()),
            Some("tauri://localhost")
        );

        let rejected_origin = Request::builder()
            .method(Method::OPTIONS)
            .uri("/app/overview")
            .header(header::ORIGIN, "https://evil.example")
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
            .body(Body::empty())
            .unwrap();
        let rejected_origin = app.clone().oneshot(rejected_origin).await.unwrap();
        assert!(rejected_origin
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none());

        let rejected_method = Request::builder()
            .method(Method::OPTIONS)
            .uri("/app/overview")
            .header(header::ORIGIN, "tauri://localhost")
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, "DELETE")
            .body(Body::empty())
            .unwrap();
        let rejected_method = app.clone().oneshot(rejected_method).await.unwrap();
        let allowed_methods = rejected_method
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_METHODS)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert!(!allowed_methods.contains("DELETE"));

        let oversized = app
            .clone()
            .oneshot(request(
                Method::POST,
                "/auth/tauri/login",
                json!({
                    "email": "web@example.com",
                    "password": "x".repeat(65 * 1024),
                    "installationId": "oversized"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);

        let login_body = json!({
            "email": "web@example.com",
            "password": "correct-password",
            "installationId": "browser-1"
        });
        let login = app
            .clone()
            .oneshot(request(Method::POST, "/auth/web/login", login_body))
            .await
            .unwrap();
        assert_eq!(login.status(), StatusCode::OK);
        let cookie = login
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(cookie.contains("Secure"));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Strict"));
        let login_body = to_bytes(login.into_body(), 16 * 1024).await.unwrap();
        assert!(!String::from_utf8_lossy(&login_body).contains("refreshToken"));

        for attempt in 1..=3 {
            let response = app
                .clone()
                .oneshot(request(
                    Method::POST,
                    "/auth/tauri/login",
                    json!({
                        "email": "web@example.com",
                        "password": "wrong-password",
                        "installationId": "browser-1"
                    }),
                ))
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                if attempt < 3 {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::PRECONDITION_REQUIRED
                }
            );
        }

        drop(app);
        fs::remove_dir_all(data_dir).expect("remove test data");
    }
}
