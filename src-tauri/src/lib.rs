use std::{path::PathBuf, sync::Mutex};

use cloudledger_service::{
    AppConfirmTransactionReceiptInput, AppCreateTransactionInput, AppDecideApprovalInput,
    AppLedgerService, AppMarkTransactionPaidInput, AppServiceError, LedgerOverview, TransactionDto,
};
use tauri::{Manager, State};

struct AppState {
    ledger_service: Mutex<AppLedgerService>,
    storage_path: PathBuf,
}

#[tauri::command]
fn health() -> &'static str {
    "ok"
}

#[tauri::command]
fn get_overview(state: State<'_, AppState>) -> Result<LedgerOverview, String> {
    let service = state
        .ledger_service
        .lock()
        .map_err(|_| "ledger service lock poisoned".to_string())?;
    Ok(service.overview(service.current_user_id()))
}

#[tauri::command]
fn create_transaction(
    state: State<'_, AppState>,
    mut input: AppCreateTransactionInput,
) -> Result<TransactionDto, String> {
    let mut service = state
        .ledger_service
        .lock()
        .map_err(|_| "ledger service lock poisoned".to_string())?;
    input.actor_user_id = service.current_user_id();
    let transaction = service.create_transaction(input).map_err(service_error)?;
    service
        .save_to_path(&state.storage_path)
        .map_err(service_error)?;
    Ok(transaction)
}

#[tauri::command]
fn decide_approval(
    state: State<'_, AppState>,
    mut input: AppDecideApprovalInput,
) -> Result<TransactionDto, String> {
    let mut service = state
        .ledger_service
        .lock()
        .map_err(|_| "ledger service lock poisoned".to_string())?;
    input.actor_user_id = service.current_user_id();
    let transaction = service.decide_approval(input).map_err(service_error)?;
    service
        .save_to_path(&state.storage_path)
        .map_err(service_error)?;
    Ok(transaction)
}

#[tauri::command]
fn mark_transaction_paid(
    state: State<'_, AppState>,
    mut input: AppMarkTransactionPaidInput,
) -> Result<TransactionDto, String> {
    let mut service = state
        .ledger_service
        .lock()
        .map_err(|_| "ledger service lock poisoned".to_string())?;
    input.actor_user_id = service.current_user_id();
    let transaction = service
        .mark_transaction_paid(input)
        .map_err(service_error)?;
    service
        .save_to_path(&state.storage_path)
        .map_err(service_error)?;
    Ok(transaction)
}

#[tauri::command]
fn confirm_transaction_receipt(
    state: State<'_, AppState>,
    mut input: AppConfirmTransactionReceiptInput,
) -> Result<TransactionDto, String> {
    let mut service = state
        .ledger_service
        .lock()
        .map_err(|_| "ledger service lock poisoned".to_string())?;
    input.actor_user_id = service.current_user_id();
    let transaction = service
        .confirm_transaction_receipt(input)
        .map_err(service_error)?;
    service
        .save_to_path(&state.storage_path)
        .map_err(service_error)?;
    Ok(transaction)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let storage_path = app.path().app_data_dir()?.join("ledger-state.json");
            let ledger_service = AppLedgerService::load_or_seed(&storage_path)?;
            app.manage(AppState {
                ledger_service: Mutex::new(ledger_service),
                storage_path,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            health,
            get_overview,
            create_transaction,
            decide_approval,
            mark_transaction_paid,
            confirm_transaction_receipt
        ])
        .run(tauri::generate_context!())
        .expect("error while running CloudLedger");
}

fn service_error(error: AppServiceError) -> String {
    error.to_string()
}
