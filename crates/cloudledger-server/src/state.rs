use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use cloudledger_service::AppLedgerService;
use uuid::Uuid;

use crate::auth::AuthService;

const DEFAULT_DATA_DIR: &str = ".cloudledger-server";
const SERVER_ID_FILE: &str = "server-id";
const ADMIN_TOKEN_FILE: &str = "admin-token";
const LEDGER_STATE_FILE: &str = "ledger-state.json";
const AUTH_STATE_FILE: &str = "auth-state.json";

#[derive(Debug, Clone)]
pub struct ServerState {
    pub server_id: Uuid,
    pub data_dir: PathBuf,
    pub ledger_state_path: PathBuf,
    pub auth_state_path: PathBuf,
    pub ledger_service: Arc<Mutex<AppLedgerService>>,
    pub auth_service: Arc<Mutex<AuthService>>,
    pub admin_token: Arc<String>,
}

impl ServerState {
    pub fn load_from_env() -> anyhow::Result<Self> {
        let data_dir = std::env::var("CLOUDLEDGER_SERVER_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(DEFAULT_DATA_DIR));
        Self::load(data_dir)
    }

    pub fn load(data_dir: PathBuf) -> anyhow::Result<Self> {
        fs::create_dir_all(&data_dir)?;
        let server_id_path = data_dir.join(SERVER_ID_FILE);
        let server_id = load_or_create_server_id(&server_id_path)?;
        let admin_token_path = data_dir.join(ADMIN_TOKEN_FILE);
        let admin_token = load_or_create_admin_token(&admin_token_path)?;
        let ledger_state_path = data_dir.join(LEDGER_STATE_FILE);
        let ledger_service = AppLedgerService::load_or_seed(&ledger_state_path)?;
        let auth_state_path = data_dir.join(AUTH_STATE_FILE);
        let auth_service = AuthService::load_or_default(&auth_state_path)?;

        Ok(Self {
            server_id,
            ledger_state_path,
            auth_state_path,
            ledger_service: Arc::new(Mutex::new(ledger_service)),
            auth_service: Arc::new(Mutex::new(auth_service)),
            admin_token: Arc::new(admin_token),
            data_dir,
        })
    }
}

fn load_or_create_server_id(path: &Path) -> anyhow::Result<Uuid> {
    if path.exists() {
        let raw = fs::read_to_string(path)?;
        return Ok(Uuid::parse_str(raw.trim())?);
    }

    let server_id = Uuid::new_v4();
    fs::write(path, server_id.to_string())?;
    Ok(server_id)
}

fn load_or_create_admin_token(path: &Path) -> anyhow::Result<String> {
    if let Ok(token) = std::env::var("CLOUDLEDGER_ADMIN_TOKEN") {
        let token = token.trim().to_string();
        if !token.is_empty() {
            return Ok(token);
        }
    }

    if path.exists() {
        let raw = fs::read_to_string(path)?;
        let token = raw.trim().to_string();
        if !token.is_empty() {
            return Ok(token);
        }
    }

    let token = format!("admin_{}", Uuid::new_v4());
    fs::write(path, &token)?;
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_id_is_stable_in_data_dir() {
        let data_dir = std::env::temp_dir().join(format!("cloudledger-server-{}", Uuid::new_v4()));

        let first = ServerState::load(data_dir.clone()).expect("first load");
        let second = ServerState::load(data_dir.clone()).expect("second load");

        assert_eq!(first.server_id, second.server_id);
        assert_eq!(first.admin_token, second.admin_token);

        fs::remove_dir_all(data_dir).expect("remove temp dir");
    }
}
