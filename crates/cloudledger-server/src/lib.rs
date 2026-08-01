pub mod admin;
pub mod app_api;
pub mod auth;
pub mod auth_routes;
pub mod config;
pub mod login_protection;
pub mod platform_auth;
pub mod state;
pub mod storage;
pub mod sync;
pub mod turnstile;

use axum::{
    extract::{DefaultBodyLimit, Request, State},
    http::{
        header::{
            CACHE_CONTROL, CONTENT_SECURITY_POLICY, REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS,
            X_FRAME_OPTIONS,
        },
        HeaderValue,
    },
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
pub use state::ServerState;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
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
    Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/sync/ping", get(sync::sync_ping))
        .route("/auth/login", post(auth_routes::login))
        .route("/auth/refresh", post(auth_routes::refresh))
        .route(
            "/auth/me",
            get(auth_routes::me).patch(auth_routes::update_me),
        )
        .route("/auth/logout", post(auth_routes::logout))
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
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
}

pub fn admin_router(state: ServerState) -> Router {
    admin::router(state)
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(middleware::from_fn(admin_security_headers))
        .layer(TraceLayer::new_for_http())
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
