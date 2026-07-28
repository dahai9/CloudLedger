use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use cloudledger_service::AppLedgerService;
use uuid::Uuid;

use crate::auth::AuthService;
use crate::config::BackendConfig;
use crate::login_protection::{LoginProtection, LoginProtectionConfig};
use crate::platform_auth::PlatformSessions;
use crate::storage::{BackendStore, PostgresStore};
use crate::turnstile::TurnstileVerifier;

const SERVER_ID_FILE: &str = "server-id";
const LEDGER_STATE_FILE: &str = "ledger-state.json";
const AUTH_STATE_FILE: &str = "auth-state.json";

#[derive(Debug, Clone)]
pub struct ServerState {
    pub server_id: Uuid,
    pub data_dir: PathBuf,
    pub ledger_state_path: PathBuf,
    pub auth_state_path: PathBuf,
    pub storage: Arc<BackendStore>,
    pub(crate) write_gate: Arc<tokio::sync::Mutex<()>>,
    pub ledger_service: Arc<Mutex<AppLedgerService>>,
    pub auth_service: Arc<Mutex<AuthService>>,
    pub login_protection: Arc<Mutex<LoginProtection>>,
    pub platform_sessions: Arc<Mutex<PlatformSessions>>,
    pub admin_token: Arc<String>,
    pub admin_path: Arc<String>,
    pub turnstile: Arc<TurnstileVerifier>,
}

struct StateInitialization {
    data_dir: PathBuf,
    admin_token: String,
    admin_path: String,
    login_protection_config: LoginProtectionConfig,
    turnstile: TurnstileVerifier,
    ledger_service: AppLedgerService,
    auth_service: AuthService,
    storage: BackendStore,
}

impl ServerState {
    pub async fn load_from_config(config: &BackendConfig) -> anyhow::Result<Self> {
        config.validate()?;
        let data_dir = config.server.data_dir.clone();
        fs::create_dir_all(&data_dir)?;
        let ledger_state_path = data_dir.join(LEDGER_STATE_FILE);
        let auth_state_path = data_dir.join(AUTH_STATE_FILE);
        let postgres = PostgresStore::connect(&config.database).await?;
        let (ledger_service, mut auth_service, imported_legacy) = postgres
            .load_or_import(&ledger_state_path, &auth_state_path)
            .await?;
        if imported_legacy {
            println!(
                "imported legacy JSON state into PostgreSQL; JSON files are now read-only migration sources"
            );
        }
        if migrate_organization_admin_accounts(&ledger_service, &mut auth_service) {
            postgres.save_auth(auth_service.snapshot()).await?;
        }
        Self::load_with_security(StateInitialization {
            data_dir,
            admin_token: config.admin.token.clone(),
            admin_path: config.admin.path.clone(),
            login_protection_config: LoginProtectionConfig::from_settings(&config.security.login),
            turnstile: TurnstileVerifier::from_config(&config.security.turnstile)?,
            ledger_service,
            auth_service,
            storage: BackendStore::Postgres(postgres),
        })
    }

    pub fn load(data_dir: PathBuf) -> anyhow::Result<Self> {
        let config = BackendConfig::load_or_create_for_data_dir(data_dir)?;
        let data_dir = config.server.data_dir.clone();
        fs::create_dir_all(&data_dir)?;
        let ledger_state_path = data_dir.join(LEDGER_STATE_FILE);
        let auth_state_path = data_dir.join(AUTH_STATE_FILE);
        let ledger_service = AppLedgerService::load_or_seed(&ledger_state_path)?;
        let mut auth_service = AuthService::load_or_default(&auth_state_path)?;
        if migrate_organization_admin_accounts(&ledger_service, &mut auth_service) {
            auth_service.save_to_path(&auth_state_path)?;
        }
        Self::load_with_security(StateInitialization {
            data_dir,
            admin_token: config.admin.token,
            admin_path: config.admin.path,
            login_protection_config: LoginProtectionConfig::from_settings(&config.security.login),
            turnstile: TurnstileVerifier::from_config(&config.security.turnstile)?,
            ledger_service,
            auth_service,
            storage: BackendStore::json(ledger_state_path, auth_state_path),
        })
    }

    fn load_with_security(initialization: StateInitialization) -> anyhow::Result<Self> {
        let StateInitialization {
            data_dir,
            admin_token,
            admin_path,
            login_protection_config,
            turnstile,
            ledger_service,
            auth_service,
            storage,
        } = initialization;
        fs::create_dir_all(&data_dir)?;
        let server_id_path = data_dir.join(SERVER_ID_FILE);
        let server_id = load_or_create_server_id(&server_id_path)?;
        let ledger_state_path = data_dir.join(LEDGER_STATE_FILE);
        let auth_state_path = data_dir.join(AUTH_STATE_FILE);
        Ok(Self {
            server_id,
            ledger_state_path,
            auth_state_path,
            storage: Arc::new(storage),
            write_gate: Arc::new(tokio::sync::Mutex::new(())),
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

fn migrate_organization_admin_accounts(
    ledger_service: &AppLedgerService,
    auth_service: &mut AuthService,
) -> bool {
    ledger_service
        .organization_admin_accounts()
        .into_iter()
        .fold(false, |changed, (user_id, organization_id)| {
            auth_service.mark_organization_admin(user_id, organization_id) || changed
        })
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
        assert!(data_dir.join("config.toml").exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                fs::metadata(data_dir.join("config.toml"))
                    .expect("backend config metadata")
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
