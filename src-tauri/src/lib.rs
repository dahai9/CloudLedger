use std::sync::Mutex;

use cloudledger_service::{
    AppCreateTransactionInput, AppLedgerService, AppServiceError, LedgerOverview, TransactionDto,
};
use tauri::State;

struct AppState {
    ledger_service: Mutex<AppLedgerService>,
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
    input: AppCreateTransactionInput,
) -> Result<TransactionDto, String> {
    let mut service = state
        .ledger_service
        .lock()
        .map_err(|_| "ledger service lock poisoned".to_string())?;
    service.create_transaction(input).map_err(service_error)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState {
            ledger_service: Mutex::new(AppLedgerService::seeded()),
        })
        .invoke_handler(tauri::generate_handler![
            health,
            get_overview,
            create_transaction
        ])
        .run(tauri::generate_context!())
        .expect("error while running CloudLedger");
}

fn service_error(error: AppServiceError) -> String {
    error.to_string()
}
