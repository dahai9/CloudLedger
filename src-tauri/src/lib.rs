use std::{path::PathBuf, sync::Mutex};

use cloudledger_service::{
    AppConfirmTransactionReceiptInput, AppCreateCategoryInput, AppCreateTransactionInput,
    AppDecideApprovalInput, AppLedgerService, AppMarkTransactionPaidInput, AppServiceError,
    AppVoidTransactionInput, AuditPeriodDto, AuditPeriodGranularity, CategoryDto,
    FinancialAnalysisDto, FinancialMemberDetailDto, FinancialMonthDetailDto, LedgerOverview,
    TransactionDto, TransactionMonthDto,
};
use serde::{Deserialize, Serialize};
use tauri::{Manager, State};
use uuid::Uuid;

mod offline_store;
use offline_store::OfflineStore;

struct AppState {
    ledger_service: Mutex<AppLedgerService>,
    storage_path: PathBuf,
    offline_store: OfflineStore,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SecureRefreshSession {
    refresh_token: String,
    installation_id: String,
}

#[derive(Default)]
struct SecureSessionState {
    #[cfg(not(target_os = "android"))]
    desktop_session: Mutex<Option<SecureRefreshSession>>,
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
const CREDENTIAL_SERVICE: &str = "com.cloudledger.app";
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
const CREDENTIAL_USER: &str = "refresh-session-v1";

#[tauri::command]
fn health() -> &'static str {
    "ok"
}

#[tauri::command]
fn offline_cache_load(state: State<'_, AppState>) -> Result<Option<serde_json::Value>, String> {
    state.offline_store.load_last()
}

#[tauri::command]
fn offline_cache_store(
    state: State<'_, AppState>,
    user_id: String,
    document: serde_json::Value,
) -> Result<(), String> {
    Uuid::parse_str(&user_id).map_err(|_| "invalid offline cache user id".to_string())?;
    state.offline_store.save(&user_id, &document)
}

#[tauri::command]
fn offline_cache_clear(state: State<'_, AppState>, user_id: String) -> Result<(), String> {
    Uuid::parse_str(&user_id).map_err(|_| "invalid offline cache user id".to_string())?;
    state.offline_store.clear(&user_id)
}

#[tauri::command]
fn secure_session_store(
    _state: State<'_, SecureSessionState>,
    _window: tauri::WebviewWindow,
    refresh_token: String,
    installation_id: String,
) -> Result<(), String> {
    if refresh_token.trim().is_empty() || installation_id.trim().is_empty() {
        return Err("refresh token and installation id are required".to_string());
    }
    let session = SecureRefreshSession {
        refresh_token,
        installation_id,
    };
    #[cfg(target_os = "android")]
    android_secure_session::store(&_window, &session)?;
    #[cfg(not(target_os = "android"))]
    {
        desktop_secure_session::store(&_state, session)?;
    }
    Ok(())
}

#[tauri::command]
fn secure_session_load(
    _state: State<'_, SecureSessionState>,
    _window: tauri::WebviewWindow,
) -> Result<Option<SecureRefreshSession>, String> {
    #[cfg(target_os = "android")]
    return android_secure_session::load(&_window);
    #[cfg(not(target_os = "android"))]
    {
        desktop_secure_session::load(&_state)
    }
}

#[tauri::command]
fn secure_session_clear(
    _state: State<'_, SecureSessionState>,
    _window: tauri::WebviewWindow,
) -> Result<(), String> {
    #[cfg(target_os = "android")]
    android_secure_session::clear(&_window)?;
    #[cfg(not(target_os = "android"))]
    {
        desktop_secure_session::clear(&_state)?;
    }
    Ok(())
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
fn get_financial_analysis(
    state: State<'_, AppState>,
    ledger_id: Uuid,
    months: u8,
) -> Result<FinancialAnalysisDto, String> {
    let service = state
        .ledger_service
        .lock()
        .map_err(|_| "ledger service lock poisoned".to_string())?;
    service
        .financial_analysis(service.current_user_id(), ledger_id, months)
        .map_err(service_error)
}

#[tauri::command]
fn get_financial_month_detail(
    state: State<'_, AppState>,
    ledger_id: Uuid,
    month: String,
) -> Result<FinancialMonthDetailDto, String> {
    let service = state
        .ledger_service
        .lock()
        .map_err(|_| "ledger service lock poisoned".to_string())?;
    service
        .financial_month_detail(service.current_user_id(), ledger_id, &month)
        .map_err(service_error)
}

#[tauri::command]
fn get_financial_member_detail(
    state: State<'_, AppState>,
    ledger_id: Uuid,
    months: u8,
    member_id: Uuid,
) -> Result<FinancialMemberDetailDto, String> {
    let service = state
        .ledger_service
        .lock()
        .map_err(|_| "ledger service lock poisoned".to_string())?;
    service
        .financial_member_detail(service.current_user_id(), ledger_id, months, member_id)
        .map_err(service_error)
}

#[tauri::command]
fn get_transactions_for_month(
    state: State<'_, AppState>,
    ledger_id: Uuid,
    month: Option<String>,
    day: Option<String>,
) -> Result<TransactionMonthDto, String> {
    let service = state
        .ledger_service
        .lock()
        .map_err(|_| "ledger service lock poisoned".to_string())?;
    match day.as_deref() {
        Some(day) => service
            .transactions_for_day(service.current_user_id(), ledger_id, month.as_deref(), day)
            .map_err(service_error),
        None => service
            .transactions_for_month(service.current_user_id(), ledger_id, month.as_deref())
            .map_err(service_error),
    }
}

#[tauri::command]
fn get_audit_period(
    state: State<'_, AppState>,
    ledger_id: Uuid,
    granularity: String,
    period: String,
) -> Result<AuditPeriodDto, String> {
    let granularity = match granularity.as_str() {
        "day" => AuditPeriodGranularity::Day,
        "month" => AuditPeriodGranularity::Month,
        _ => return Err(service_error(AppServiceError::InvalidAuditGranularity)),
    };
    let service = state
        .ledger_service
        .lock()
        .map_err(|_| "ledger service lock poisoned".to_string())?;
    service
        .audit_period(service.current_user_id(), ledger_id, granularity, &period)
        .map_err(service_error)
}

#[tauri::command]
fn create_category(
    state: State<'_, AppState>,
    mut input: AppCreateCategoryInput,
) -> Result<CategoryDto, String> {
    let mut service = state
        .ledger_service
        .lock()
        .map_err(|_| "ledger service lock poisoned".to_string())?;
    input.actor_user_id = service.current_user_id();
    let category = service.create_category(input).map_err(service_error)?;
    service
        .save_to_path(&state.storage_path)
        .map_err(service_error)?;
    Ok(category)
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
fn void_transaction(
    state: State<'_, AppState>,
    mut input: AppVoidTransactionInput,
) -> Result<TransactionDto, String> {
    let mut service = state
        .ledger_service
        .lock()
        .map_err(|_| "ledger service lock poisoned".to_string())?;
    input.actor_user_id = service.current_user_id();
    let transaction = service.void_transaction(input).map_err(service_error)?;
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
            let offline_store =
                OfflineStore::open(app.path().app_data_dir()?.join("offline-cache.sqlite"))
                    .map_err(std::io::Error::other)?;
            let ledger_service = AppLedgerService::load_or_seed(&storage_path)?;
            app.manage(AppState {
                ledger_service: Mutex::new(ledger_service),
                storage_path,
                offline_store,
            });
            app.manage(SecureSessionState::default());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            health,
            offline_cache_load,
            offline_cache_store,
            offline_cache_clear,
            secure_session_store,
            secure_session_load,
            secure_session_clear,
            get_overview,
            get_financial_analysis,
            get_financial_month_detail,
            get_financial_member_detail,
            get_transactions_for_month,
            get_audit_period,
            create_category,
            create_transaction,
            decide_approval,
            mark_transaction_paid,
            void_transaction,
            confirm_transaction_receipt
        ])
        .run(tauri::generate_context!())
        .expect("error while running CloudLedger");
}

fn service_error(error: AppServiceError) -> String {
    error.to_string()
}

#[cfg(not(target_os = "android"))]
mod desktop_secure_session {
    use tauri::State;

    use super::{SecureRefreshSession, SecureSessionState};

    pub fn store(
        state: &State<'_, SecureSessionState>,
        session: SecureRefreshSession,
    ) -> Result<(), String> {
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        {
            let payload = serde_json::to_string(&session).map_err(|error| error.to_string())?;
            if credential_entry()
                .and_then(|entry| entry.set_password(&payload))
                .is_ok()
            {
                *lock_memory(state)? = None;
                return Ok(());
            }
        }
        *lock_memory(state)? = Some(session);
        Ok(())
    }

    pub fn load(
        state: &State<'_, SecureSessionState>,
    ) -> Result<Option<SecureRefreshSession>, String> {
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        if let Ok(entry) = credential_entry() {
            if let Ok(payload) = entry.get_password() {
                if let Ok(session) = serde_json::from_str(&payload) {
                    return Ok(Some(session));
                }
                let _ = entry.delete_credential();
            }
        }
        Ok(lock_memory(state)?.clone())
    }

    pub fn clear(state: &State<'_, SecureSessionState>) -> Result<(), String> {
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        if let Ok(entry) = credential_entry() {
            let _ = entry.delete_credential();
        }
        *lock_memory(state)? = None;
        Ok(())
    }

    fn lock_memory<'a>(
        state: &'a State<'_, SecureSessionState>,
    ) -> Result<std::sync::MutexGuard<'a, Option<SecureRefreshSession>>, String> {
        state
            .desktop_session
            .lock()
            .map_err(|_| "secure session lock poisoned".to_string())
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    fn credential_entry() -> keyring::Result<keyring::Entry> {
        keyring::Entry::new(super::CREDENTIAL_SERVICE, super::CREDENTIAL_USER)
    }
}

#[cfg(target_os = "android")]
mod android_secure_session {
    use std::{sync::mpsc::sync_channel, time::Duration};

    use jni::objects::{JClass, JObject, JString, JValue};

    use super::SecureRefreshSession;

    const CLASS: &str = "com.cloudledger.app.SecureSessionStore";
    const JNI_TIMEOUT: Duration = Duration::from_secs(10);

    pub fn store(
        window: &tauri::WebviewWindow,
        session: &SecureRefreshSession,
    ) -> Result<(), String> {
        let payload = serde_json::to_string(session).map_err(|error| error.to_string())?;
        with_activity(window, move |env, activity| {
            let payload = env.new_string(payload).map_err(|error| error.to_string())?;
            let class = session_class(env, activity)?;
            env.call_static_method(
                class,
                "store",
                "(Landroid/content/Context;Ljava/lang/String;)V",
                &[JValue::Object(activity), JValue::Object(&payload)],
            )
            .map_err(|error| error.to_string())?;
            Ok(())
        })
    }

    pub fn load(window: &tauri::WebviewWindow) -> Result<Option<SecureRefreshSession>, String> {
        with_activity(window, |env, activity| {
            let class = session_class(env, activity)?;
            let value = env
                .call_static_method(
                    class,
                    "load",
                    "(Landroid/content/Context;)Ljava/lang/String;",
                    &[JValue::Object(activity)],
                )
                .map_err(|error| error.to_string())?
                .l()
                .map_err(|error| error.to_string())?;
            if value.is_null() {
                return Ok(None);
            }
            let value = JString::from(value);
            let payload: String = env
                .get_string(&value)
                .map_err(|error| error.to_string())?
                .into();
            serde_json::from_str(&payload)
                .map(Some)
                .map_err(|error| error.to_string())
        })
    }

    pub fn clear(window: &tauri::WebviewWindow) -> Result<(), String> {
        with_activity(window, |env, activity| {
            let class = session_class(env, activity)?;
            env.call_static_method(
                class,
                "clear",
                "(Landroid/content/Context;)V",
                &[JValue::Object(activity)],
            )
            .map_err(|error| error.to_string())?;
            Ok(())
        })
    }

    fn session_class<'local>(
        env: &mut jni::JNIEnv<'local>,
        activity: &JObject<'_>,
    ) -> Result<JClass<'local>, String> {
        let loader = env
            .call_method(activity, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
            .and_then(|value| value.l())
            .map_err(|error| error.to_string())?;
        let class_name = env.new_string(CLASS).map_err(|error| error.to_string())?;
        let class = env
            .call_method(
                loader,
                "loadClass",
                "(Ljava/lang/String;)Ljava/lang/Class;",
                &[JValue::Object(&class_name)],
            )
            .and_then(|value| value.l())
            .map_err(|error| error.to_string())?;
        Ok(JClass::from(class))
    }

    fn with_activity<T, F>(window: &tauri::WebviewWindow, operation: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce(&mut jni::JNIEnv<'_>, &JObject<'_>) -> Result<T, String> + Send + 'static,
    {
        let (sender, receiver) = sync_channel(1);
        window
            .with_webview(move |webview| {
                webview.jni_handle().exec(move |env, activity, _webview| {
                    let result = operation(env, activity);
                    if result.is_err() && env.exception_check().unwrap_or(false) {
                        let _ = env.exception_clear();
                    }
                    let _ = sender.send(result);
                });
            })
            .map_err(|error| error.to_string())?;
        receiver
            .recv_timeout(JNI_TIMEOUT)
            .map_err(|error| format!("Android secure session bridge failed: {error}"))?
    }
}
