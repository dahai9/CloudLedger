use axum::{extract::State, http::HeaderMap, Json};
use cloudledger_service::{
    AppCreateTransactionInput, AppDecideApprovalInput, LedgerOverview, TransactionDto,
};

use crate::{auth_routes, auth_routes::ApiError, ServerState};

pub async fn overview(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<LedgerOverview>, ApiError> {
    let session = auth_routes::authenticate(&state, &headers)?;
    let service = state
        .ledger_service
        .lock()
        .map_err(|_| ApiError::internal("ledger service lock poisoned"))?;
    Ok(Json(service.overview(session.user.id)))
}

pub async fn create_transaction(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(mut input): Json<AppCreateTransactionInput>,
) -> Result<Json<TransactionDto>, ApiError> {
    let session = auth_routes::authenticate(&state, &headers)?;
    input.actor_user_id = session.user.id;
    let mut service = state
        .ledger_service
        .lock()
        .map_err(|_| ApiError::internal("ledger service lock poisoned"))?;
    let transaction = service
        .create_transaction(input)
        .map_err(ApiError::from_service)?;
    service
        .save_to_path(&state.ledger_state_path)
        .map_err(ApiError::from_service)?;
    Ok(Json(transaction))
}

pub async fn decide_approval(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(mut input): Json<AppDecideApprovalInput>,
) -> Result<Json<TransactionDto>, ApiError> {
    let session = auth_routes::authenticate(&state, &headers)?;
    input.actor_user_id = session.user.id;
    let mut service = state
        .ledger_service
        .lock()
        .map_err(|_| ApiError::internal("ledger service lock poisoned"))?;
    let transaction = service
        .decide_approval(input)
        .map_err(ApiError::from_service)?;
    service
        .save_to_path(&state.ledger_state_path)
        .map_err(ApiError::from_service)?;
    Ok(Json(transaction))
}
