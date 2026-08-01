use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};
use cloudledger_service::{
    AppConfirmTransactionReceiptInput, AppCreateCategoryInput, AppCreateTransactionInput,
    AppDecideApprovalInput, AppMarkTransactionPaidInput, CategoryDto, FinancialAnalysisDto,
    LedgerOverview, TransactionDto, TransactionMonthDto,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::{auth_routes, auth_routes::ApiError, ServerState};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FinancialAnalysisQuery {
    pub ledger_id: Uuid,
    #[serde(default = "default_analysis_months")]
    pub months: u8,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionMonthQuery {
    pub ledger_id: Uuid,
    pub month: Option<String>,
}

fn default_analysis_months() -> u8 {
    6
}

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

pub async fn financial_analysis(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<FinancialAnalysisQuery>,
) -> Result<Json<FinancialAnalysisDto>, ApiError> {
    let session = auth_routes::authenticate(&state, &headers)?;
    let service = state
        .ledger_service
        .lock()
        .map_err(|_| ApiError::internal("ledger service lock poisoned"))?;
    Ok(Json(
        service
            .financial_analysis(session.user.id, query.ledger_id, query.months)
            .map_err(ApiError::from_service)?,
    ))
}

pub async fn transactions_for_month(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<TransactionMonthQuery>,
) -> Result<Json<TransactionMonthDto>, ApiError> {
    let session = auth_routes::authenticate(&state, &headers)?;
    let service = state
        .ledger_service
        .lock()
        .map_err(|_| ApiError::internal("ledger service lock poisoned"))?;
    Ok(Json(
        service
            .transactions_for_month(session.user.id, query.ledger_id, query.month.as_deref())
            .map_err(ApiError::from_service)?,
    ))
}

pub async fn create_category(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(mut input): Json<AppCreateCategoryInput>,
) -> Result<Json<CategoryDto>, ApiError> {
    let _write_guard = state.write_gate.lock().await;
    let session = auth_routes::authenticate(&state, &headers)?;
    input.actor_user_id = session.user.id;
    let (category, staged_service) = {
        let service = state
            .ledger_service
            .lock()
            .map_err(|_| ApiError::internal("ledger service lock poisoned"))?;
        let mut staged_service = service.clone();
        let category = staged_service
            .create_category(input)
            .map_err(ApiError::from_service)?;
        (category, staged_service)
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
    Ok(Json(category))
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
