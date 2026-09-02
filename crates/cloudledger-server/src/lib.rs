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
pub mod version;

use crate::{login_protection::SecurityRateKind, request_security::RequestContext};
use axum::{
    extract::{DefaultBodyLimit, Request, State},
    http::{
        header::{
            AUTHORIZATION, CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, REFERRER_POLICY,
            X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS,
        },
        HeaderName, HeaderValue, Method, StatusCode,
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthBootstrapResponse {
    pub ngrok_warning_bypass_enabled: bool,
}

pub fn router(state: ServerState) -> Router {
    let cors = cors_layer(&state);
    let request_security = state.request_security.clone();
    let rate_limit_state = state.clone();
    let version_state = state.clone();
    Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/client/version", get(version::client_version))
        .route("/sync/ping", get(sync::sync_ping))
        .route("/auth/login", post(auth_routes::legacy_upgrade))
        .route("/auth/refresh", post(auth_routes::legacy_upgrade))
        .route(
            "/auth/me",
            get(auth_routes::me).patch(auth_routes::update_me),
        )
        .route("/auth/logout", post(auth_routes::legacy_upgrade))
        .route("/auth/security", get(auth_routes::auth_security))
        .route("/auth/bootstrap", post(auth_bootstrap))
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
            "/app/analytics/month-detail",
            get(app_api::financial_month_detail),
        )
        .route(
            "/app/analytics/member-detail",
            get(app_api::financial_member_detail),
        )
        .route("/app/audit-period", get(app_api::audit_period))
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
        .route("/app/transactions/void", post(app_api::void_transaction))
        .route(
            "/app/payments/confirm-receipt",
            post(app_api::confirm_transaction_receipt),
        )
        .with_state(state)
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(middleware::from_fn_with_state(
            rate_limit_state,
            security_rate_limits,
        ))
        .layer(middleware::from_fn_with_state(
            request_security,
            request_security::resolve_request_context,
        ))
        .layer(middleware::from_fn_with_state(
            version_state,
            version::enforce_client_version,
        ))
        .layer(TraceLayer::new_for_http())
        // Keep CORS outermost so rate-limit and authentication failures remain
        // observable to an allowed browser origin instead of looking like a
        // transport failure.
        .layer(cors)
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
    let mut headers = vec![
        AUTHORIZATION,
        CONTENT_TYPE,
        HeaderName::from_static("x-cloudledger-client-version"),
    ];
    if state.ngrok_warning_bypass_enabled {
        headers.push(HeaderName::from_static("ngrok-skip-browser-warning"));
    }
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET, Method::POST, Method::PATCH])
        .allow_headers(headers)
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

async fn auth_bootstrap(State(state): State<ServerState>) -> Json<AuthBootstrapResponse> {
    Json(AuthBootstrapResponse {
        ngrok_warning_bypass_enabled: state.ngrok_warning_bypass_enabled,
    })
}

#[cfg(test)]
mod http_tests {
    use std::{fs, net::SocketAddr, sync::Arc};

    use axum::{
        body::{to_bytes, Body},
        extract::ConnectInfo,
        http::{header, Request},
    };
    use serde_json::json;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{AccountKind, AdminCreateUserInput, RegisterInput};
    use crate::login_protection::INVALID_BEARER_LIMIT;

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

    fn authorized_get(uri: &str, access_token: &str) -> Request<Body> {
        let mut request = request(Method::GET, uri, json!({}));
        request.headers_mut().insert(
            header::AUTHORIZATION,
            format!("Bearer {access_token}")
                .parse()
                .expect("bearer header"),
        );
        request
    }

    #[tokio::test]
    async fn client_version_endpoint_and_gate_enforce_the_minimum_version() {
        let data_dir = std::env::temp_dir().join(format!("cloudledger-http-{}", Uuid::new_v4()));
        let mut state = ServerState::load(data_dir.clone()).expect("load test state");
        *Arc::make_mut(&mut state.client_version) = "0.1.14".to_string();
        *Arc::make_mut(&mut state.min_supported_client_version) = "0.1.13".to_string();
        *Arc::make_mut(&mut state.client_download_url) =
            "https://github.com/dahai9/CloudLedger/releases/latest".to_string();
        let app = router(state);

        let mut version_request = request(Method::GET, "/client/version", json!({}));
        version_request.headers_mut().insert(
            "x-cloudledger-client-version",
            HeaderValue::from_static("0.1.12"),
        );
        let version_response = app.clone().oneshot(version_request).await.unwrap();
        assert_eq!(version_response.status(), StatusCode::OK);
        let version_body = to_bytes(version_response.into_body(), 4096).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&version_body).unwrap(),
            json!({
                "currentVersion": "0.1.14",
                "minSupportedVersion": "0.1.13",
                "downloadUrl": "https://github.com/dahai9/CloudLedger/releases/latest",
                "updateRequired": true
            })
        );

        let mut outdated = request(Method::GET, "/app/overview", json!({}));
        outdated.headers_mut().insert(
            "x-cloudledger-client-version",
            HeaderValue::from_static("0.1.12"),
        );
        let outdated = app.clone().oneshot(outdated).await.unwrap();
        assert_eq!(outdated.status(), StatusCode::UPGRADE_REQUIRED);
        let outdated_body = to_bytes(outdated.into_body(), 4096).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&outdated_body).unwrap()["code"],
            "client_update_required"
        );

        let mut supported = request(Method::GET, "/app/overview", json!({}));
        supported.headers_mut().insert(
            "x-cloudledger-client-version",
            HeaderValue::from_static("0.1.13"),
        );
        let supported = app.clone().oneshot(supported).await.unwrap();
        assert_eq!(supported.status(), StatusCode::UNAUTHORIZED);

        let preflight = Request::builder()
            .method(Method::OPTIONS)
            .uri("/app/overview")
            .header(header::ORIGIN, "tauri://localhost")
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
            .header(
                header::ACCESS_CONTROL_REQUEST_HEADERS,
                "authorization,x-cloudledger-client-version",
            )
            .body(Body::empty())
            .unwrap();
        let preflight = app.oneshot(preflight).await.unwrap();
        assert_eq!(preflight.status(), StatusCode::OK);
        assert!(preflight
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_HEADERS)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|headers| headers.contains("x-cloudledger-client-version")));

        fs::remove_dir_all(data_dir).expect("remove test data");
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

        let bootstrap = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/auth/bootstrap")
                    .header(header::ORIGIN, "tauri://localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bootstrap.status(), StatusCode::OK);
        let bootstrap_body = to_bytes(bootstrap.into_body(), 4096).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bootstrap_body).unwrap(),
            json!({"ngrokWarningBypassEnabled": true})
        );

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
            .header(
                header::ACCESS_CONTROL_REQUEST_HEADERS,
                "authorization,ngrok-skip-browser-warning",
            )
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
        assert!(preflight
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_HEADERS)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|headers| headers.contains("ngrok-skip-browser-warning")));

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

        for attempt in 1..=INVALID_BEARER_LIMIT {
            let mut invalid_bearer = request(Method::GET, "/auth/tauri/me", json!({}));
            invalid_bearer.headers_mut().insert(
                header::ORIGIN,
                HeaderValue::from_static("tauri://localhost"),
            );
            invalid_bearer.headers_mut().insert(
                header::AUTHORIZATION,
                HeaderValue::from_static("Bearer invalid-access-token"),
            );
            let response = app.clone().oneshot(invalid_bearer).await.unwrap();
            assert_eq!(
                response.status(),
                if attempt < INVALID_BEARER_LIMIT {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::TOO_MANY_REQUESTS
                }
            );
            assert_eq!(
                response
                    .headers()
                    .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                    .and_then(|value| value.to_str().ok()),
                Some("tauri://localhost")
            );
        }

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

    #[tokio::test]
    async fn reverse_proxy_mode_disables_ngrok_warning_bypass() {
        let data_dir = std::env::temp_dir().join(format!("cloudledger-http-{}", Uuid::new_v4()));
        let mut state = ServerState::load(data_dir.clone()).expect("load test state");
        state.ngrok_warning_bypass_enabled = false;
        let app = router(state);

        let bootstrap = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/auth/bootstrap")
                    .header(header::ORIGIN, "tauri://localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bootstrap_body = to_bytes(bootstrap.into_body(), 4096).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bootstrap_body).unwrap(),
            json!({"ngrokWarningBypassEnabled": false})
        );

        let preflight = Request::builder()
            .method(Method::OPTIONS)
            .uri("/auth/tauri/me")
            .header(header::ORIGIN, "tauri://localhost")
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
            .header(
                header::ACCESS_CONTROL_REQUEST_HEADERS,
                "authorization,ngrok-skip-browser-warning",
            )
            .body(Body::empty())
            .unwrap();
        let preflight = app.oneshot(preflight).await.unwrap();
        assert!(preflight
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_HEADERS)
            .and_then(|value| value.to_str().ok())
            .is_none_or(|headers| !headers.contains("ngrok-skip-browser-warning")));

        fs::remove_dir_all(data_dir).expect("remove test data");
    }

    #[tokio::test]
    async fn employee_http_responses_hide_balances_and_reject_all_analytics_routes() {
        let data_dir = std::env::temp_dir().join(format!("cloudledger-http-{}", Uuid::new_v4()));
        let state = ServerState::load(data_dir.clone()).expect("load test state");
        let service = cloudledger_service::AppLedgerService::seeded();
        let bob = service
            .users()
            .into_iter()
            .find(|user| user.display_name == "Bob")
            .expect("seeded Bob");
        let bob_id = Uuid::parse_str(&bob.id).expect("Bob id");
        let public_ledger_id = service
            .overview(bob_id)
            .ledgers
            .into_iter()
            .find(|ledger| ledger.kind == "organization_public")
            .map(|ledger| ledger.id)
            .expect("public ledger");
        *state.ledger_service.lock().expect("ledger lock") = service;
        let session = state
            .auth_service
            .lock()
            .expect("auth lock")
            .register(RegisterInput {
                user_id: Some(bob_id),
                display_name: "Bob".to_string(),
                email: Some("bob-http@example.com".to_string()),
                phone: None,
                password: "correct-password".to_string(),
                installation_id: "bob-http-test".to_string(),
            })
            .expect("register Bob session");
        let app = router(state);

        let overview = app
            .clone()
            .oneshot(authorized_get("/app/overview", &session.access_token))
            .await
            .expect("overview response");
        assert_eq!(overview.status(), StatusCode::OK);
        let overview_body = to_bytes(overview.into_body(), 64 * 1024)
            .await
            .expect("overview body");
        let overview: serde_json::Value =
            serde_json::from_slice(&overview_body).expect("overview json");
        let public_ledger = overview["ledgers"]
            .as_array()
            .expect("ledgers")
            .iter()
            .find(|ledger| ledger["id"] == public_ledger_id)
            .expect("public ledger response");
        assert_eq!(public_ledger["canViewBalances"], false);
        assert!(overview["accounts"]
            .as_array()
            .expect("accounts")
            .iter()
            .filter(|account| account["ledgerId"] == public_ledger_id)
            .all(|account| account["balanceMinor"].is_null()));

        for uri in [
            format!("/app/analytics?ledgerId={public_ledger_id}&months=3"),
            format!(
                "/app/analytics/month-detail?ledgerId={public_ledger_id}&month=2026-08"
            ),
            format!(
                "/app/analytics/member-detail?ledgerId={public_ledger_id}&months=3&memberId={bob_id}"
            ),
        ] {
            let response = app
                .clone()
                .oneshot(authorized_get(&uri, &session.access_token))
                .await
                .expect("analytics response");
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{uri}");
        }

        drop(app);
        fs::remove_dir_all(data_dir).expect("remove test data");
    }
}
