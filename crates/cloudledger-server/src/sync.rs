use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ServerState;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPingResponse {
    pub server_id: Uuid,
    pub sync_model: &'static str,
    pub public_ledger_authority: &'static str,
}

pub async fn sync_ping(State(state): State<ServerState>) -> Json<SyncPingResponse> {
    Json(SyncPingResponse {
        server_id: state.server_id,
        sync_model: "cloud_authoritative_public_ledgers",
        public_ledger_authority: "server",
    })
}
