use std::{
    fs,
    io::Write,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
};

use anyhow::Context;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ipnet::IpNet;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

pub const DEFAULT_CONFIG_PATH: &str = ".cloudledger-server/config.toml";
const DEFAULT_DATA_DIR: &str = ".cloudledger-server";
pub(crate) const DEFAULT_TURNSTILE_VERIFY_URL: &str =
    "https://challenges.cloudflare.com/turnstile/v0/siteverify";
const LEGACY_ADMIN_PATH_FILE: &str = "admin-path";
const LEGACY_ADMIN_TOKEN_FILE: &str = "admin-token";

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BackendConfig {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub admin: AdminConfig,
    pub security: SecurityConfig,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DatabaseConfig {
    pub url: String,
    pub auto_migrate: bool,
    pub max_connections: u32,
    pub connect_timeout_seconds: u64,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: "postgres://cloudledger@127.0.0.1:5432/cloudledger".to_string(),
            auto_migrate: true,
            max_connections: 10,
            connect_timeout_seconds: 10,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    pub mode: RunMode,
    pub api_bind_addr: SocketAddr,
    pub admin_bind_addr: SocketAddr,
    pub public_api_url: String,
    pub public_admin_url: String,
    pub allow_insecure_lan: bool,
    pub web_login_enabled: bool,
    pub data_dir: PathBuf,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    ReverseProxy,
    #[default]
    Development,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            mode: RunMode::Development,
            api_bind_addr: "127.0.0.1:8787".parse().expect("valid default API address"),
            admin_bind_addr: "127.0.0.1:8788"
                .parse()
                .expect("valid default admin address"),
            public_api_url: "http://127.0.0.1:8787".to_string(),
            public_admin_url: "http://127.0.0.1:8788".to_string(),
            allow_insecure_lan: false,
            web_login_enabled: false,
            data_dir: PathBuf::from(DEFAULT_DATA_DIR),
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminConfig {
    pub path: String,
    pub token: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SecurityConfig {
    pub login: LoginSecurityConfig,
    pub turnstile: TurnstileConfig,
    pub network: NetworkSecurityConfig,
    pub audit: AuditSecurityConfig,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NetworkSecurityConfig {
    pub trusted_proxy_cidrs: Vec<IpNet>,
    pub cors_allowed_origins: Vec<String>,
}

impl Default for NetworkSecurityConfig {
    fn default() -> Self {
        Self {
            trusted_proxy_cidrs: vec!["127.0.0.1/32".parse().expect("loopback CIDR")],
            cors_allowed_origins: vec![
                "tauri://localhost".to_string(),
                "https://tauri.localhost".to_string(),
            ],
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuditSecurityConfig {
    pub key_id: String,
    pub hmac_key: String,
    pub identifier_hmac_key: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoginSecurityConfig {
    pub turnstile_after_failures: u32,
    pub max_failures_per_login: u32,
    pub max_failures_per_ip: u32,
    pub window_seconds: u64,
    pub lockout_seconds: u64,
}

impl Default for LoginSecurityConfig {
    fn default() -> Self {
        Self {
            turnstile_after_failures: 3,
            max_failures_per_login: 5,
            max_failures_per_ip: 20,
            window_seconds: 15 * 60,
            lockout_seconds: 15 * 60,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TurnstileConfig {
    pub site_key: String,
    pub secret_key: String,
    pub verify_url: String,
}

impl Default for TurnstileConfig {
    fn default() -> Self {
        Self {
            site_key: String::new(),
            secret_key: String::new(),
            verify_url: DEFAULT_TURNSTILE_VERIFY_URL.to_string(),
        }
    }
}

impl BackendConfig {
    pub fn load_or_create(path: &Path) -> anyhow::Result<Self> {
        Self::load_or_create_with_default(path, Self::default())
    }

    pub(crate) fn load_or_create_for_data_dir(data_dir: PathBuf) -> anyhow::Result<Self> {
        let path = data_dir.join("config.toml");
        let mut default = Self::default();
        default.server.data_dir = data_dir;
        Self::load_or_create_with_default(&path, default)
    }

    fn load_or_create_with_default(path: &Path, default: Self) -> anyhow::Result<Self> {
        let existed = path.exists();
        let mut config = if existed {
            restrict_private_file_permissions(path)?;
            let contents = fs::read_to_string(path)
                .with_context(|| format!("failed to read backend config {}", path.display()))?;
            toml::from_str(&contents)
                .with_context(|| format!("invalid backend config {}", path.display()))?
        } else {
            default
        };

        let credentials_added = config.complete_admin_credentials()?;
        let security_keys_added = config.complete_security_keys();
        config.normalize();
        config.validate()?;

        if !existed || credentials_added || security_keys_added {
            write_private_config(path, &config)?;
        }
        Ok(config)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if self.server.data_dir.as_os_str().is_empty() {
            anyhow::bail!("server.data_dir cannot be empty");
        }
        let database_url = validate_database_url(&self.database.url)?;
        if self.database.max_connections == 0 {
            anyhow::bail!("database.max_connections must be greater than zero");
        }
        if self.database.connect_timeout_seconds == 0 {
            anyhow::bail!("database.connect_timeout_seconds must be greater than zero");
        }
        validate_public_url("server.public_api_url", &self.server.public_api_url)?;
        validate_public_url("server.public_admin_url", &self.server.public_admin_url)?;
        validate_server_mode(self, &database_url)?;
        validate_admin_path(&self.admin.path)?;
        if self.admin.token.is_empty() {
            anyhow::bail!("admin.token cannot be empty");
        }

        let login = &self.security.login;
        if login.turnstile_after_failures == 0
            || login.turnstile_after_failures >= login.max_failures_per_login
        {
            anyhow::bail!(
                "security.login.turnstile_after_failures must be below max_failures_per_login"
            );
        }
        if login.max_failures_per_login == 0 {
            anyhow::bail!("security.login.max_failures_per_login must be greater than zero");
        }
        if login.max_failures_per_ip < login.max_failures_per_login {
            anyhow::bail!(
                "security.login.max_failures_per_ip must be greater than or equal to security.login.max_failures_per_login"
            );
        }
        if login.window_seconds == 0 || login.lockout_seconds == 0 {
            anyhow::bail!(
                "security.login.window_seconds and security.login.lockout_seconds must be greater than zero"
            );
        }

        let turnstile = &self.security.turnstile;
        match (turnstile.site_key.is_empty(), turnstile.secret_key.is_empty()) {
            (true, true) | (false, false) => {}
            _ => anyhow::bail!(
                "security.turnstile.site_key and security.turnstile.secret_key must be configured together"
            ),
        }
        if turnstile.verify_url.is_empty() {
            anyhow::bail!("security.turnstile.verify_url cannot be empty");
        }
        if self
            .security
            .network
            .cors_allowed_origins
            .iter()
            .any(|origin| origin == "*" || origin.contains('*'))
        {
            anyhow::bail!("security.network.cors_allowed_origins must not contain wildcards");
        }
        for origin in &self.security.network.cors_allowed_origins {
            let parsed =
                Url::parse(origin).with_context(|| format!("invalid CORS origin {origin}"))?;
            if !matches!(parsed.path(), "" | "/")
                || parsed.query().is_some()
                || parsed.fragment().is_some()
            {
                anyhow::bail!("CORS origin must contain only scheme and authority: {origin}");
            }
        }
        if self.security.audit.key_id.is_empty()
            || decode_32_byte_key("security.audit.hmac_key", &self.security.audit.hmac_key).is_err()
            || decode_32_byte_key(
                "security.audit.identifier_hmac_key",
                &self.security.audit.identifier_hmac_key,
            )
            .is_err()
        {
            anyhow::bail!(
                "security.audit.key_id and 32-byte hmac_key/identifier_hmac_key are required"
            );
        }
        Ok(())
    }

    pub fn validate_migration_database_url(&self, value: &str) -> anyhow::Result<()> {
        let database_url = validate_database_url(value)?;
        if self.server.mode == RunMode::ReverseProxy {
            validate_production_database_url(&database_url)?;
        }
        Ok(())
    }

    fn complete_admin_credentials(&mut self) -> anyhow::Result<bool> {
        let mut changed = false;
        if self.admin.path.trim().is_empty() {
            self.admin.path =
                read_legacy_value(&self.server.data_dir.join(LEGACY_ADMIN_PATH_FILE))?
                    .unwrap_or_else(|| format!("manage-{}", Uuid::new_v4().simple()));
            changed = true;
        }
        if self.admin.token.trim().is_empty() {
            self.admin.token =
                read_legacy_value(&self.server.data_dir.join(LEGACY_ADMIN_TOKEN_FILE))?
                    .unwrap_or_else(|| format!("admin_{}", Uuid::new_v4()));
            changed = true;
        }
        Ok(changed)
    }

    fn complete_security_keys(&mut self) -> bool {
        let mut changed = false;
        if self.security.audit.key_id.trim().is_empty() {
            self.security.audit.key_id = format!("audit-{}", Uuid::new_v4().simple());
            changed = true;
        }
        if self.security.audit.hmac_key.trim().is_empty() {
            self.security.audit.hmac_key = random_key();
            changed = true;
        }
        if self.security.audit.identifier_hmac_key.trim().is_empty() {
            self.security.audit.identifier_hmac_key = random_key();
            changed = true;
        }
        changed
    }

    fn normalize(&mut self) {
        self.admin.path = self.admin.path.trim().trim_matches('/').to_string();
        self.admin.token = self.admin.token.trim().to_string();
        self.database.url = self.database.url.trim().to_string();
        self.server.public_api_url = self
            .server
            .public_api_url
            .trim()
            .trim_end_matches('/')
            .to_string();
        self.server.public_admin_url = self
            .server
            .public_admin_url
            .trim()
            .trim_end_matches('/')
            .to_string();
        self.security.network.cors_allowed_origins = self
            .security
            .network
            .cors_allowed_origins
            .iter()
            .map(|origin| origin.trim().trim_end_matches('/').to_string())
            .filter(|origin| !origin.is_empty())
            .collect();
        self.security.turnstile.site_key = self.security.turnstile.site_key.trim().to_string();
        self.security.turnstile.secret_key = self.security.turnstile.secret_key.trim().to_string();
        self.security.turnstile.verify_url = self.security.turnstile.verify_url.trim().to_string();
        self.security.audit.key_id = self.security.audit.key_id.trim().to_string();
        self.security.audit.hmac_key = self.security.audit.hmac_key.trim().to_string();
        self.security.audit.identifier_hmac_key =
            self.security.audit.identifier_hmac_key.trim().to_string();
    }
}

impl AuditSecurityConfig {
    pub fn hmac_key_bytes(&self) -> anyhow::Result<[u8; 32]> {
        decode_32_byte_key("security.audit.hmac_key", &self.hmac_key)
    }

    pub fn identifier_hmac_key_bytes(&self) -> anyhow::Result<[u8; 32]> {
        decode_32_byte_key(
            "security.audit.identifier_hmac_key",
            &self.identifier_hmac_key,
        )
    }
}

fn validate_database_url(value: &str) -> anyhow::Result<Url> {
    let parsed = Url::parse(value).context("database.url must be a valid URL")?;
    if !matches!(parsed.scheme(), "postgres" | "postgresql") {
        anyhow::bail!("database.url must be a PostgreSQL connection URL");
    }
    Ok(parsed)
}

fn validate_public_url(name: &str, value: &str) -> anyhow::Result<Url> {
    let parsed = Url::parse(value).with_context(|| format!("{name} must be an absolute URL"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        anyhow::bail!("{name} must be an absolute HTTP(S) URL");
    }
    Ok(parsed)
}

fn validate_server_mode(config: &BackendConfig, database_url: &Url) -> anyhow::Result<()> {
    let server = &config.server;
    match server.mode {
        RunMode::ReverseProxy => {
            if !server.api_bind_addr.ip().is_loopback()
                || !server.admin_bind_addr.ip().is_loopback()
            {
                anyhow::bail!("reverse_proxy mode requires API and admin listeners on loopback");
            }
            if !server.public_api_url.starts_with("https://")
                || !server.public_admin_url.starts_with("https://")
            {
                anyhow::bail!("reverse_proxy mode requires HTTPS public URLs");
            }
            if server.web_login_enabled {
                anyhow::bail!("Web login must be disabled in reverse_proxy mode");
            }
            if config.database.auto_migrate {
                anyhow::bail!(
                    "reverse_proxy mode requires database.auto_migrate = false; use the one-time migrate command"
                );
            }
            if config.security.turnstile.site_key.is_empty()
                || config.security.turnstile.secret_key.is_empty()
            {
                anyhow::bail!("reverse_proxy mode requires Cloudflare Turnstile");
            }
            validate_production_database_url(database_url)?;
            const TAURI_ORIGINS: [&str; 2] = ["tauri://localhost", "https://tauri.localhost"];
            if config
                .security
                .network
                .cors_allowed_origins
                .iter()
                .any(|origin| !TAURI_ORIGINS.contains(&origin.as_str()))
            {
                anyhow::bail!("reverse_proxy mode permits only built-in Tauri CORS origins");
            }
        }
        RunMode::Development => {
            let lan_listener = !server.api_bind_addr.ip().is_loopback()
                || !server.admin_bind_addr.ip().is_loopback();
            if lan_listener && !server.allow_insecure_lan {
                anyhow::bail!("development LAN HTTP requires server.allow_insecure_lan = true");
            }
            if server.web_login_enabled
                && (!server.public_api_url.starts_with("https://")
                    || !server.public_admin_url.starts_with("https://"))
            {
                anyhow::bail!("development Web login requires local HTTPS public URLs");
            }
        }
    }
    Ok(())
}

fn validate_production_database_url(database_url: &Url) -> anyhow::Result<()> {
    reject_known_database_password(database_url)?;
    let database_host = database_url.host_str().unwrap_or_default();
    if !is_loopback_host(database_host)
        && database_url
            .query_pairs()
            .find(|(key, _)| key == "sslmode")
            .is_none_or(|(_, value)| value != "verify-full")
    {
        anyhow::bail!("remote PostgreSQL requires sslmode=verify-full in reverse_proxy mode");
    }
    Ok(())
}

fn reject_known_database_password(database_url: &Url) -> anyhow::Result<()> {
    let password = database_url.password().unwrap_or_default();
    if password.is_empty()
        || matches!(
            password,
            "cloudledger" | "postgres" | "change-this-password"
        )
    {
        anyhow::bail!("reverse_proxy mode requires a non-example database password");
    }
    Ok(())
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn random_key() -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn decode_32_byte_key(name: &str, value: &str) -> anyhow::Result<[u8; 32]> {
    let decoded = URL_SAFE_NO_PAD
        .decode(value)
        .with_context(|| format!("{name} must use unpadded base64url"))?;
    decoded
        .try_into()
        .map_err(|_| anyhow::anyhow!("{name} must decode to exactly 32 bytes"))
}

fn validate_admin_path(value: &str) -> anyhow::Result<()> {
    let valid = (16..=128).contains(&value.len())
        && value != "admin"
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
    if !valid {
        anyhow::bail!(
            "admin.path must be one 16-128 character path segment using letters, numbers, '-' or '_', and cannot be 'admin'"
        );
    }
    Ok(())
}

fn read_legacy_value(path: &Path) -> anyhow::Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    restrict_private_file_permissions(path)?;
    let value = fs::read_to_string(path)?;
    let value = value.trim().to_string();
    Ok((!value.is_empty()).then_some(value))
}

fn write_private_config(path: &Path, config: &BackendConfig) -> anyhow::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let contents = toml::to_string_pretty(config)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        restrict_private_file_permissions(path)?;
    }
    #[cfg(not(unix))]
    fs::write(path, contents)?;

    Ok(())
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

    fn temp_config_path() -> PathBuf {
        std::env::temp_dir()
            .join(format!("cloudledger-config-{}", Uuid::new_v4()))
            .join("config.toml")
    }

    fn valid_reverse_proxy_config() -> BackendConfig {
        let mut config = BackendConfig::default();
        config.admin.path = "manage-0123456789abcdef0123456789abcdef".to_string();
        config.admin.token = "test-platform-token".to_string();
        config.complete_security_keys();
        config.server.mode = RunMode::ReverseProxy;
        config.server.public_api_url = "https://api.example.com".to_string();
        config.server.public_admin_url = "https://admin.example.com".to_string();
        config.database.url =
            "postgres://cloudledger:non-example-secret@127.0.0.1:5432/cloudledger".to_string();
        config.database.auto_migrate = false;
        config.security.turnstile.site_key = "site-key".to_string();
        config.security.turnstile.secret_key = "secret-key".to_string();
        config
    }

    #[test]
    fn creates_complete_private_config_and_reuses_credentials() {
        let root = temp_config_path().parent().unwrap().to_path_buf();
        let path = root.join("config.toml");
        let first =
            BackendConfig::load_or_create_for_data_dir(root.clone()).expect("create config");
        let second =
            BackendConfig::load_or_create_for_data_dir(root.clone()).expect("reload config");

        assert_eq!(first.admin.path, second.admin.path);
        assert_eq!(first.admin.token, second.admin.token);
        assert!(first.admin.path.starts_with("manage-"));
        assert!(path.exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }

        fs::remove_dir_all(path.parent().unwrap()).expect("remove temp config");
    }

    #[test]
    fn migrates_legacy_admin_credentials() {
        let path = temp_config_path();
        let root = path.parent().unwrap().to_path_buf();
        fs::create_dir_all(&root).expect("create temp data dir");
        fs::write(
            root.join(LEGACY_ADMIN_PATH_FILE),
            "manage-existing-1234567890",
        )
        .unwrap();
        fs::write(
            root.join(LEGACY_ADMIN_TOKEN_FILE),
            "existing-platform-token",
        )
        .unwrap();
        let config = BackendConfig::load_or_create_for_data_dir(root.clone()).unwrap();

        assert_eq!(config.admin.path, "manage-existing-1234567890");
        assert_eq!(config.admin.token, "existing-platform-token");

        fs::remove_dir_all(root).expect("remove temp data dir");
    }

    #[test]
    fn rejects_public_admin_bind_and_incomplete_turnstile() {
        let mut config = BackendConfig::default();
        config.admin.path = "manage-0123456789abcdef0123456789abcdef".to_string();
        config.admin.token = "test-platform-token".to_string();
        config.complete_security_keys();
        config.server.admin_bind_addr = "8.8.8.8:8788".parse().unwrap();
        assert!(config.validate().is_err());

        config.server.admin_bind_addr = "192.168.1.20:8788".parse().unwrap();
        assert!(config.validate().is_err());
        config.server.allow_insecure_lan = true;
        assert!(config.validate().is_ok());
        config.security.turnstile.site_key = "site-key".to_string();
        assert!(config.validate().is_err());
        config.security.turnstile.secret_key = "secret-key".to_string();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn reverse_proxy_rejects_missing_or_example_database_credentials() {
        let mut config = valid_reverse_proxy_config();
        config.database.url.clear();
        assert!(config.validate().is_err());

        for password in ["cloudledger", "postgres", "change-this-password"] {
            config.database.url =
                format!("postgres://cloudledger:{password}@127.0.0.1:5432/cloudledger");
            assert!(
                config.validate().is_err(),
                "accepted example password {password}"
            );
        }
    }

    #[test]
    fn reverse_proxy_requires_verify_full_for_remote_postgres() {
        let mut config = valid_reverse_proxy_config();
        config.database.url =
            "postgres://cloudledger:non-example-secret@db.example.com:5432/cloudledger?sslmode=require"
                .to_string();
        assert!(config.validate().is_err());

        config.database.url =
            "postgres://cloudledger:non-example-secret@db.example.com:5432/cloudledger?sslmode=verify-full"
                .to_string();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn rejects_wildcard_cors_origins() {
        let mut config = valid_reverse_proxy_config();
        config.security.network.cors_allowed_origins = vec!["*".to_string()];
        assert!(config.validate().is_err());

        config.security.network.cors_allowed_origins = vec!["https://*.example.com".to_string()];
        assert!(config.validate().is_err());
    }

    #[test]
    fn reverse_proxy_rejects_web_cors_origins() {
        let mut config = valid_reverse_proxy_config();
        config
            .security
            .network
            .cors_allowed_origins
            .push("https://localhost:1420".to_string());
        assert!(config.validate().is_err());
    }

    #[test]
    fn migration_database_url_uses_production_database_policy() {
        let config = valid_reverse_proxy_config();
        assert!(config
            .validate_migration_database_url(
                "postgres://migration:strong-secret@db.example.com/cloudledger?sslmode=require"
            )
            .is_err());
        assert!(config
            .validate_migration_database_url(
                "postgres://migration:strong-secret@db.example.com/cloudledger?sslmode=verify-full"
            )
            .is_ok());
    }
}
