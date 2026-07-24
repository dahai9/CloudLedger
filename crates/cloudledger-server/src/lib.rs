pub mod admin;
pub mod app_api;
pub mod auth;
pub mod auth_routes;
pub mod state;
pub mod sync;

use axum::{
    extract::State,
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
        .route("/app/transactions", post(app_api::create_transaction))
        .route("/app/approvals/decide", post(app_api::decide_approval))
        .with_state(state)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
}

pub fn admin_router(state: ServerState) -> Router {
    admin::router(state).layer(TraceLayer::new_for_http())
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
