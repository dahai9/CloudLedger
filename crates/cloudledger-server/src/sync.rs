use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPingResponse {
    pub server_id: Uuid,
    pub sync_model: &'static str,
    pub public_ledger_authority: &'static str,
}

pub async fn sync_ping() -> Json<SyncPingResponse> {
    Json(SyncPingResponse {
        server_id: Uuid::nil(),
        sync_model: "cloud_authoritative_public_ledgers",
        public_ledger_authority: "server",
    })
}
