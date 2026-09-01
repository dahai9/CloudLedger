use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};
use cloudledger_service::{
    AppConfirmTransactionReceiptInput, AppCreateCategoryInput, AppCreateTransactionInput,
    AppDecideApprovalInput, AppMarkTransactionPaidInput, AppVoidTransactionInput, AuditPeriodDto,
    AuditPeriodGranularity, CategoryDto, FinancialAnalysisDto, FinancialMemberDetailDto,
    FinancialMonthDetailDto, LedgerOverview, TransactionDto, TransactionMonthDto,
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
    pub day: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditPeriodQuery {
    pub ledger_id: Uuid,
    pub granularity: String,
    pub period: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FinancialMonthDetailQuery {
    pub ledger_id: Uuid,
    pub month: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FinancialMemberDetailQuery {
    pub ledger_id: Uuid,
    #[serde(default = "default_analysis_months")]
    pub months: u8,
    pub member_id: Uuid,
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

pub async fn financial_month_detail(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<FinancialMonthDetailQuery>,
) -> Result<Json<FinancialMonthDetailDto>, ApiError> {
    let session = auth_routes::authenticate(&state, &headers)?;
    let service = state
        .ledger_service
        .lock()
        .map_err(|_| ApiError::internal("ledger service lock poisoned"))?;
    Ok(Json(
        service
            .financial_month_detail(session.user.id, query.ledger_id, &query.month)
            .map_err(ApiError::from_service)?,
    ))
}

pub async fn financial_member_detail(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<FinancialMemberDetailQuery>,
) -> Result<Json<FinancialMemberDetailDto>, ApiError> {
    let session = auth_routes::authenticate(&state, &headers)?;
    let service = state
        .ledger_service
        .lock()
        .map_err(|_| ApiError::internal("ledger service lock poisoned"))?;
    Ok(Json(
        service
            .financial_member_detail(
                session.user.id,
                query.ledger_id,
                query.months,
                query.member_id,
            )
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
    Ok(Json(match query.day.as_deref() {
        Some(day) => service
            .transactions_for_day(
                session.user.id,
                query.ledger_id,
                query.month.as_deref(),
                day,
            )
            .map_err(ApiError::from_service)?,
        None => service
            .transactions_for_month(session.user.id, query.ledger_id, query.month.as_deref())
            .map_err(ApiError::from_service)?,
    }))
}

pub async fn audit_period(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<AuditPeriodQuery>,
) -> Result<Json<AuditPeriodDto>, ApiError> {
    let session = auth_routes::authenticate(&state, &headers)?;
    let granularity = match query.granularity.as_str() {
        "day" => AuditPeriodGranularity::Day,
        "month" => AuditPeriodGranularity::Month,
        _ => {
            return Err(ApiError::from_service(
                cloudledger_service::AppServiceError::InvalidAuditGranularity,
            ))
        }
    };
    let service = state
        .ledger_service
        .lock()
        .map_err(|_| ApiError::internal("ledger service lock poisoned"))?;
    Ok(Json(
        service
            .audit_period(session.user.id, query.ledger_id, granularity, &query.period)
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

pub async fn void_transaction(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(mut input): Json<AppVoidTransactionInput>,
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
            .void_transaction(input)
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
