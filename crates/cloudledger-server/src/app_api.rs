use axum::{extract::State, http::HeaderMap, Json};
use cloudledger_service::{
    AppConfirmTransactionReceiptInput, AppCreateTransactionInput, AppDecideApprovalInput,
    AppMarkTransactionPaidInput, LedgerOverview, TransactionDto,
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
    let _write_guard = state.write_gate.lock().await;
    let session = auth_routes::authenticate(&state, &headers)?;
    input.actor_user_id = session.user.id;
    let (transaction, staged_service) = {
        let service = state
            .ledger_service
            .lock()
            .map_err(|_| ApiError::internal("ledger service lock poisoned"))?;
        let mut staged_service = service.clone();
        let transaction = staged_service
            .create_transaction(input)
            .map_err(ApiError::from_service)?;
        (transaction, staged_service)
    };
    state
        .storage
        .save_ledger(staged_service.snapshot())
        .await
        .map_err(ApiError::from_storage)?;
    *state
        .ledger_service
        .lock()
        .map_err(|_| ApiError::internal("ledger service lock poisoned"))? = staged_service;
    Ok(Json(transaction))
}

pub async fn decide_approval(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(mut input): Json<AppDecideApprovalInput>,
) -> Result<Json<TransactionDto>, ApiError> {
    let _write_guard = state.write_gate.lock().await;
    let session = auth_routes::authenticate(&state, &headers)?;
    input.actor_user_id = session.user.id;
    let (transaction, staged_service) = {
        let service = state
            .ledger_service
            .lock()
            .map_err(|_| ApiError::internal("ledger service lock poisoned"))?;
        let mut staged_service = service.clone();
        let transaction = staged_service
            .decide_approval(input)
            .map_err(ApiError::from_service)?;
        (transaction, staged_service)
    };
    state
        .storage
        .save_ledger(staged_service.snapshot())
        .await
        .map_err(ApiError::from_storage)?;
    *state
        .ledger_service
        .lock()
        .map_err(|_| ApiError::internal("ledger service lock poisoned"))? = staged_service;
    Ok(Json(transaction))
}

pub async fn mark_transaction_paid(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(mut input): Json<AppMarkTransactionPaidInput>,
) -> Result<Json<TransactionDto>, ApiError> {
    let _write_guard = state.write_gate.lock().await;
    let session = auth_routes::authenticate(&state, &headers)?;
    input.actor_user_id = session.user.id;
    let (transaction, staged_service) = {
        let service = state
            .ledger_service
            .lock()
            .map_err(|_| ApiError::internal("ledger service lock poisoned"))?;
        let mut staged_service = service.clone();
        let transaction = staged_service
            .mark_transaction_paid(input)
            .map_err(ApiError::from_service)?;
        (transaction, staged_service)
    };
    state
        .storage
        .save_ledger(staged_service.snapshot())
        .await
        .map_err(ApiError::from_storage)?;
    *state
        .ledger_service
        .lock()
        .map_err(|_| ApiError::internal("ledger service lock poisoned"))? = staged_service;
    Ok(Json(transaction))
}

pub async fn confirm_transaction_receipt(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(mut input): Json<AppConfirmTransactionReceiptInput>,
) -> Result<Json<TransactionDto>, ApiError> {
    let _write_guard = state.write_gate.lock().await;
    let session = auth_routes::authenticate(&state, &headers)?;
    input.actor_user_id = session.user.id;
    let (transaction, staged_service) = {
        let service = state
            .ledger_service
            .lock()
            .map_err(|_| ApiError::internal("ledger service lock poisoned"))?;
        let mut staged_service = service.clone();
        let transaction = staged_service
            .confirm_transaction_receipt(input)
            .map_err(ApiError::from_service)?;
        (transaction, staged_service)
    };
    state
        .storage
        .save_ledger(staged_service.snapshot())
        .await
        .map_err(ApiError::from_storage)?;
    *state
        .ledger_service
        .lock()
        .map_err(|_| ApiError::internal("ledger service lock poisoned"))? = staged_service;
    Ok(Json(transaction))
}
