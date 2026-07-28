use std::{
    fs,
    io::Write,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::{Path, PathBuf},
};

use anyhow::Context;
use serde::{Deserialize, Serialize};
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
    pub max_connections: u32,
    pub connect_timeout_seconds: u64,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: "postgres://cloudledger:cloudledger@127.0.0.1:5432/cloudledger".to_string(),
            max_connections: 10,
            connect_timeout_seconds: 10,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    pub api_bind_addr: SocketAddr,
    pub admin_bind_addr: SocketAddr,
    pub data_dir: PathBuf,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            api_bind_addr: "0.0.0.0:8787".parse().expect("valid default API address"),
            admin_bind_addr: "127.0.0.1:8788"
                .parse()
                .expect("valid default admin address"),
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
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoginSecurityConfig {
    pub max_failures_per_login: u32,
    pub max_failures_per_ip: u32,
    pub window_seconds: u64,
    pub lockout_seconds: u64,
}

impl Default for LoginSecurityConfig {
    fn default() -> Self {
        Self {
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
        config.normalize();
        config.validate()?;

        if !existed || credentials_added {
            write_private_config(path, &config)?;
        }
        Ok(config)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if self.server.data_dir.as_os_str().is_empty() {
            anyhow::bail!("server.data_dir cannot be empty");
        }
        if !self.database.url.starts_with("postgres://")
            && !self.database.url.starts_with("postgresql://")
        {
            anyhow::bail!("database.url must be a PostgreSQL connection URL");
        }
        if self.database.max_connections == 0 {
            anyhow::bail!("database.max_connections must be greater than zero");
        }
        if self.database.connect_timeout_seconds == 0 {
            anyhow::bail!("database.connect_timeout_seconds must be greater than zero");
        }
        validate_admin_bind_addr(&self.server.admin_bind_addr)?;
        validate_admin_path(&self.admin.path)?;
        if self.admin.token.is_empty() {
            anyhow::bail!("admin.token cannot be empty");
        }

        let login = &self.security.login;
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
        if !self.server.admin_bind_addr.ip().is_loopback() && turnstile.site_key.is_empty() {
            anyhow::bail!(
                "Cloudflare Turnstile keys are required when server.admin_bind_addr is not loopback"
            );
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

    fn normalize(&mut self) {
        self.admin.path = self.admin.path.trim().trim_matches('/').to_string();
        self.admin.token = self.admin.token.trim().to_string();
        self.database.url = self.database.url.trim().to_string();
        self.security.turnstile.site_key = self.security.turnstile.site_key.trim().to_string();
        self.security.turnstile.secret_key = self.security.turnstile.secret_key.trim().to_string();
        self.security.turnstile.verify_url = self.security.turnstile.verify_url.trim().to_string();
    }
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

fn validate_admin_bind_addr(addr: &SocketAddr) -> anyhow::Result<()> {
    if is_private_or_loopback(addr.ip()) {
        Ok(())
    } else {
        anyhow::bail!("server.admin_bind_addr must be loopback or private LAN, got {addr}")
    }
}

fn is_private_or_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_loopback() || ip.is_private() || is_ipv4_link_local(ip),
        IpAddr::V6(ip) => ip.is_loopback() || is_ipv6_unique_local(ip) || is_ipv6_link_local(ip),
    }
}

fn is_ipv4_link_local(ip: Ipv4Addr) -> bool {
    let [first, second, _, _] = ip.octets();
    first == 169 && second == 254
}

fn is_ipv6_unique_local(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xfe00) == 0xfc00
}

fn is_ipv6_link_local(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfe80
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
        config.server.admin_bind_addr = "8.8.8.8:8788".parse().unwrap();
        assert!(config.validate().is_err());

        config.server.admin_bind_addr = "192.168.1.20:8788".parse().unwrap();
        assert!(config.validate().is_err());
        config.security.turnstile.site_key = "site-key".to_string();
        assert!(config.validate().is_err());
        config.security.turnstile.secret_key = "secret-key".to_string();
        assert!(config.validate().is_ok());
    }
}
