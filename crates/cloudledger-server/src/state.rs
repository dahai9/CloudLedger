use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use cloudledger_service::AppLedgerService;
use uuid::Uuid;

use crate::auth::AuthService;
use crate::login_protection::{LoginProtection, LoginProtectionConfig};
use crate::platform_auth::PlatformSessions;
use crate::turnstile::TurnstileVerifier;

const DEFAULT_DATA_DIR: &str = ".cloudledger-server";
const SERVER_ID_FILE: &str = "server-id";
const ADMIN_TOKEN_FILE: &str = "admin-token";
const ADMIN_PATH_FILE: &str = "admin-path";
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
    pub login_protection: Arc<Mutex<LoginProtection>>,
    pub platform_sessions: Arc<Mutex<PlatformSessions>>,
    pub admin_token: Arc<String>,
    pub admin_path: Arc<String>,
    pub turnstile: Arc<TurnstileVerifier>,
}

impl ServerState {
    pub fn load_from_env() -> anyhow::Result<Self> {
        let data_dir = std::env::var("CLOUDLEDGER_SERVER_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(DEFAULT_DATA_DIR));
        Self::load_with_security(
            data_dir,
            LoginProtectionConfig::from_env()?,
            TurnstileVerifier::from_env()?,
        )
    }

    pub fn load(data_dir: PathBuf) -> anyhow::Result<Self> {
        Self::load_with_security(
            data_dir,
            LoginProtectionConfig::default(),
            TurnstileVerifier::disabled(),
        )
    }

    fn load_with_security(
        data_dir: PathBuf,
        login_protection_config: LoginProtectionConfig,
        turnstile: TurnstileVerifier,
    ) -> anyhow::Result<Self> {
        fs::create_dir_all(&data_dir)?;
        let server_id_path = data_dir.join(SERVER_ID_FILE);
        let server_id = load_or_create_server_id(&server_id_path)?;
        let admin_token_path = data_dir.join(ADMIN_TOKEN_FILE);
        let admin_token = load_or_create_admin_token(&admin_token_path)?;
        let admin_path_path = data_dir.join(ADMIN_PATH_FILE);
        let admin_path = load_or_create_admin_path(&admin_path_path)?;
        let ledger_state_path = data_dir.join(LEDGER_STATE_FILE);
        let ledger_service = AppLedgerService::load_or_seed(&ledger_state_path)?;
        let auth_state_path = data_dir.join(AUTH_STATE_FILE);
        let mut auth_service = AuthService::load_or_default(&auth_state_path)?;
        let migrated_admin_accounts = ledger_service
            .organization_admin_accounts()
            .into_iter()
            .fold(false, |changed, (user_id, organization_id)| {
                auth_service.mark_organization_admin(user_id, organization_id) || changed
            });
        if migrated_admin_accounts {
            auth_service.save_to_path(&auth_state_path)?;
        }

        Ok(Self {
            server_id,
            ledger_state_path,
            auth_state_path,
            ledger_service: Arc::new(Mutex::new(ledger_service)),
            auth_service: Arc::new(Mutex::new(auth_service)),
            login_protection: Arc::new(Mutex::new(LoginProtection::new(login_protection_config))),
            platform_sessions: Arc::new(Mutex::new(PlatformSessions::default())),
            admin_token: Arc::new(admin_token),
            admin_path: Arc::new(admin_path),
            turnstile: Arc::new(turnstile),
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
        restrict_private_file_permissions(path)?;
        let raw = fs::read_to_string(path)?;
        let token = raw.trim().to_string();
        if !token.is_empty() {
            return Ok(token);
        }
    }

    let token = format!("admin_{}", Uuid::new_v4());
    write_private_file(path, &token)?;
    Ok(token)
}

fn load_or_create_admin_path(path: &Path) -> anyhow::Result<String> {
    if let Ok(value) = std::env::var("CLOUDLEDGER_ADMIN_PATH") {
        return validate_admin_path(&value);
    }
    if path.exists() {
        restrict_private_file_permissions(path)?;
        return validate_admin_path(&fs::read_to_string(path)?);
    }

    let admin_path = format!("manage-{}", Uuid::new_v4().simple());
    write_private_file(path, &admin_path)?;
    Ok(admin_path)
}

fn validate_admin_path(value: &str) -> anyhow::Result<String> {
    let value = value.trim().trim_matches('/');
    let valid = (16..=128).contains(&value.len())
        && value != "admin"
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
    if !valid {
        anyhow::bail!(
            "CLOUDLEDGER_ADMIN_PATH must be one 16-128 character path segment using letters, numbers, '-' or '_', and cannot be 'admin'"
        );
    }
    Ok(value.to_string())
}

fn write_private_file(path: &Path, value: &str) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(value.as_bytes())?;
        file.sync_all()?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::write(path, value)?;
        Ok(())
    }
}

fn restrict_private_file_permissions(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{AccountKind, AdminCreateUserInput, AuthError, LoginInput};
    use cloudledger_service::AppCreateOrganizationInput;

    #[test]
    fn server_id_is_stable_in_data_dir() {
        let data_dir = std::env::temp_dir().join(format!("cloudledger-server-{}", Uuid::new_v4()));

        let first = ServerState::load(data_dir.clone()).expect("first load");
        let second = ServerState::load(data_dir.clone()).expect("second load");

        assert_eq!(first.server_id, second.server_id);
        assert_eq!(first.admin_token, second.admin_token);
        assert_eq!(first.admin_path, second.admin_path);
        assert_ne!(first.admin_path.as_str(), "admin");
        assert!(first.admin_path.starts_with("manage-"));
        assert_eq!(first.admin_path.len(), "manage-".len() + 32);
        assert!(data_dir.join(ADMIN_PATH_FILE).exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                fs::metadata(data_dir.join(ADMIN_TOKEN_FILE))
                    .expect("admin token metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(data_dir.join(ADMIN_PATH_FILE))
                    .expect("admin path metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }

        fs::remove_dir_all(data_dir).expect("remove temp dir");
    }

    #[test]
    fn existing_owner_account_migrates_to_backend_only_admin() {
        let data_dir = std::env::temp_dir().join(format!("cloudledger-server-{}", Uuid::new_v4()));
        fs::create_dir_all(&data_dir).expect("create data dir");
        let admin_user_id = Uuid::new_v4();
        let mut ledger_service = AppLedgerService::uninitialized();
        ledger_service
            .create_organization(AppCreateOrganizationInput {
                organization_name: "Existing Organization".to_string(),
                admin_user_id,
                admin_display_name: "Existing Owner".to_string(),
                admin_email: Some("owner@example.com".to_string()),
                admin_phone: None,
            })
            .expect("create existing organization");
        ledger_service
            .save_to_path(data_dir.join(LEDGER_STATE_FILE))
            .expect("save ledger state");

        let mut auth_service = AuthService::default();
        auth_service
            .create_or_update_admin_user(AdminCreateUserInput {
                user_id: admin_user_id,
                display_name: "Existing Owner".to_string(),
                email: Some("owner@example.com".to_string()),
                phone: None,
                password: Some("owner-password".to_string()),
                account_kind: AccountKind::Business,
                organization_id: None,
            })
            .expect("create legacy business auth user");
        auth_service
            .save_to_path(data_dir.join(AUTH_STATE_FILE))
            .expect("save auth state");

        let state = ServerState::load(data_dir.clone()).expect("load and migrate server state");
        let mut auth = state.auth_service.lock().expect("auth lock");
        assert_eq!(
            auth.account_kind(admin_user_id),
            Some(AccountKind::OrganizationAdmin)
        );
        assert_eq!(
            auth.login(LoginInput {
                email: Some("owner@example.com".to_string()),
                phone: None,
                password: "owner-password".to_string(),
                installation_id: "legacy-phone".to_string(),
            })
            .unwrap_err(),
            AuthError::BusinessAppAccessDenied
        );

        drop(auth);
        fs::remove_dir_all(data_dir).expect("remove temp dir");
    }
}
