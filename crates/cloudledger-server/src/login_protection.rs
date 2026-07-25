use std::{
    collections::HashMap,
    net::IpAddr,
    time::{Duration, Instant},
};

use crate::auth::AuthError;

const DEFAULT_MAX_FAILURES_PER_LOGIN: u32 = 5;
const DEFAULT_MAX_FAILURES_PER_IP: u32 = 20;
const DEFAULT_WINDOW_SECONDS: u64 = 15 * 60;
const DEFAULT_LOCKOUT_SECONDS: u64 = 15 * 60;
const MAX_TRACKED_KEYS: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LoginSurface {
    Business,
    OrganizationAdmin,
    Platform,
    AdminAuthorization,
}

#[derive(Debug, Clone)]
pub struct LoginProtectionConfig {
    max_failures_per_login: u32,
    max_failures_per_ip: u32,
    window: Duration,
    lockout: Duration,
}

impl Default for LoginProtectionConfig {
    fn default() -> Self {
        Self {
            max_failures_per_login: DEFAULT_MAX_FAILURES_PER_LOGIN,
            max_failures_per_ip: DEFAULT_MAX_FAILURES_PER_IP,
            window: Duration::from_secs(DEFAULT_WINDOW_SECONDS),
            lockout: Duration::from_secs(DEFAULT_LOCKOUT_SECONDS),
        }
    }
}

impl LoginProtectionConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let config = Self {
            max_failures_per_login: read_u32_env(
                "CLOUDLEDGER_LOGIN_MAX_FAILURES",
                DEFAULT_MAX_FAILURES_PER_LOGIN,
            )?,
            max_failures_per_ip: read_u32_env(
                "CLOUDLEDGER_LOGIN_MAX_FAILURES_PER_IP",
                DEFAULT_MAX_FAILURES_PER_IP,
            )?,
            window: Duration::from_secs(read_u64_env(
                "CLOUDLEDGER_LOGIN_WINDOW_SECONDS",
                DEFAULT_WINDOW_SECONDS,
            )?),
            lockout: Duration::from_secs(read_u64_env(
                "CLOUDLEDGER_LOGIN_LOCKOUT_SECONDS",
                DEFAULT_LOCKOUT_SECONDS,
            )?),
        };
        if config.max_failures_per_ip < config.max_failures_per_login {
            anyhow::bail!(
                "CLOUDLEDGER_LOGIN_MAX_FAILURES_PER_IP must be greater than or equal to CLOUDLEDGER_LOGIN_MAX_FAILURES"
            );
        }
        Ok(config)
    }
}

#[derive(Debug)]
pub struct LoginProtection {
    config: LoginProtectionConfig,
    failures_by_login: HashMap<LoginKey, FailureState>,
    failures_by_ip: HashMap<IpAddr, FailureState>,
}

impl Default for LoginProtection {
    fn default() -> Self {
        Self::new(LoginProtectionConfig::default())
    }
}

impl LoginProtection {
    pub fn new(config: LoginProtectionConfig) -> Self {
        Self {
            config,
            failures_by_login: HashMap::new(),
            failures_by_ip: HashMap::new(),
        }
    }

    pub fn check(
        &mut self,
        ip: IpAddr,
        surface: LoginSurface,
        identifier: &str,
    ) -> Result<(), AuthError> {
        self.check_at(ip, surface, identifier, Instant::now())
    }

    pub fn record_failure(
        &mut self,
        ip: IpAddr,
        surface: LoginSurface,
        identifier: &str,
    ) -> Option<AuthError> {
        self.record_failure_at(ip, surface, identifier, Instant::now())
            .map(rate_limit_error)
    }

    pub fn record_success(&mut self, ip: IpAddr, surface: LoginSurface, identifier: &str) {
        self.failures_by_login
            .remove(&LoginKey::new(ip, surface, identifier));
    }

    fn check_at(
        &mut self,
        ip: IpAddr,
        surface: LoginSurface,
        identifier: &str,
        now: Instant,
    ) -> Result<(), AuthError> {
        self.prune(now);
        let login_retry = self
            .failures_by_login
            .get_mut(&LoginKey::new(ip, surface, identifier))
            .and_then(|state| active_retry_after(state, now, self.config.window));
        let ip_retry = self
            .failures_by_ip
            .get_mut(&ip)
            .and_then(|state| active_retry_after(state, now, self.config.window));
        match max_duration(login_retry, ip_retry) {
            Some(retry_after) => Err(rate_limit_error(retry_after)),
            None => Ok(()),
        }
    }

    fn record_failure_at(
        &mut self,
        ip: IpAddr,
        surface: LoginSurface,
        identifier: &str,
        now: Instant,
    ) -> Option<Duration> {
        self.prune(now);
        make_room(&mut self.failures_by_login);
        make_room(&mut self.failures_by_ip);

        let login_retry = record_failure_state(
            self.failures_by_login
                .entry(LoginKey::new(ip, surface, identifier))
                .or_insert_with(|| FailureState::new(now)),
            now,
            self.config.window,
            self.config.lockout,
            self.config.max_failures_per_login,
        );
        let ip_retry = record_failure_state(
            self.failures_by_ip
                .entry(ip)
                .or_insert_with(|| FailureState::new(now)),
            now,
            self.config.window,
            self.config.lockout,
            self.config.max_failures_per_ip,
        );
        max_duration(login_retry, ip_retry)
    }

    fn prune(&mut self, now: Instant) {
        let retention = self.config.window.saturating_add(self.config.lockout);
        self.failures_by_login.retain(|_, state| {
            state
                .blocked_until
                .is_some_and(|blocked_until| blocked_until > now)
                || now.saturating_duration_since(state.last_seen) <= retention
        });
        self.failures_by_ip.retain(|_, state| {
            state
                .blocked_until
                .is_some_and(|blocked_until| blocked_until > now)
                || now.saturating_duration_since(state.last_seen) <= retention
        });
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct LoginKey {
    ip: IpAddr,
    surface: LoginSurface,
    identifier: String,
}

impl LoginKey {
    fn new(ip: IpAddr, surface: LoginSurface, identifier: &str) -> Self {
        Self {
            ip,
            surface,
            identifier: identifier.trim().to_lowercase(),
        }
    }
}

#[derive(Debug)]
struct FailureState {
    failures: u32,
    window_started: Instant,
    blocked_until: Option<Instant>,
    last_seen: Instant,
}

impl FailureState {
    fn new(now: Instant) -> Self {
        Self {
            failures: 0,
            window_started: now,
            blocked_until: None,
            last_seen: now,
        }
    }

    fn reset(&mut self, now: Instant) {
        self.failures = 0;
        self.window_started = now;
        self.blocked_until = None;
        self.last_seen = now;
    }
}

fn active_retry_after(
    state: &mut FailureState,
    now: Instant,
    window: Duration,
) -> Option<Duration> {
    state.last_seen = now;
    if let Some(blocked_until) = state.blocked_until {
        if blocked_until > now {
            return Some(blocked_until.saturating_duration_since(now));
        }
        state.reset(now);
        return None;
    }
    if now.saturating_duration_since(state.window_started) >= window {
        state.reset(now);
    }
    None
}

fn record_failure_state(
    state: &mut FailureState,
    now: Instant,
    window: Duration,
    lockout: Duration,
    limit: u32,
) -> Option<Duration> {
    if let Some(retry_after) = active_retry_after(state, now, window) {
        return Some(retry_after);
    }
    state.failures = state.failures.saturating_add(1);
    state.last_seen = now;
    if state.failures >= limit {
        state.blocked_until = now.checked_add(lockout);
        Some(lockout)
    } else {
        None
    }
}

fn max_duration(left: Option<Duration>, right: Option<Duration>) -> Option<Duration> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn rate_limit_error(retry_after: Duration) -> AuthError {
    AuthError::LoginRateLimited {
        retry_after_seconds: retry_after.as_secs().max(1),
    }
}

fn make_room<K: std::hash::Hash + Eq + Clone>(states: &mut HashMap<K, FailureState>) {
    if states.len() < MAX_TRACKED_KEYS {
        return;
    }
    if let Some(oldest_key) = states
        .iter()
        .min_by_key(|(_, state)| state.last_seen)
        .map(|(key, _)| key.clone())
    {
        states.remove(&oldest_key);
    }
}

fn read_u32_env(name: &str, default: u32) -> anyhow::Result<u32> {
    match std::env::var(name) {
        Ok(value) => {
            let parsed = value
                .parse::<u32>()
                .map_err(|_| anyhow::anyhow!("{name} must be a positive integer"))?;
            if parsed == 0 {
                anyhow::bail!("{name} must be greater than zero");
            }
            Ok(parsed)
        }
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(anyhow::anyhow!("read {name}: {error}")),
    }
}

fn read_u64_env(name: &str, default: u64) -> anyhow::Result<u64> {
    match std::env::var(name) {
        Ok(value) => {
            let parsed = value
                .parse::<u64>()
                .map_err(|_| anyhow::anyhow!("{name} must be a positive integer"))?;
            if parsed == 0 {
                anyhow::bail!("{name} must be greater than zero");
            }
            Ok(parsed)
        }
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(anyhow::anyhow!("read {name}: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn test_config() -> LoginProtectionConfig {
        LoginProtectionConfig {
            max_failures_per_login: 3,
            max_failures_per_ip: 5,
            window: Duration::from_secs(60),
            lockout: Duration::from_secs(120),
        }
    }

    #[test]
    fn repeated_failures_lock_one_login() {
        let mut protection = LoginProtection::new(test_config());
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let now = Instant::now();

        assert!(protection
            .record_failure_at(
                ip,
                LoginSurface::OrganizationAdmin,
                "admin@example.com",
                now
            )
            .is_none());
        assert!(protection
            .record_failure_at(
                ip,
                LoginSurface::OrganizationAdmin,
                "admin@example.com",
                now
            )
            .is_none());
        assert_eq!(
            protection
                .record_failure_at(
                    ip,
                    LoginSurface::OrganizationAdmin,
                    "admin@example.com",
                    now
                )
                .map(|duration| duration.as_secs()),
            Some(120)
        );
        assert!(matches!(
            protection.check_at(
                ip,
                LoginSurface::OrganizationAdmin,
                "admin@example.com",
                now
            ),
            Err(AuthError::LoginRateLimited { .. })
        ));
    }

    #[test]
    fn ip_limit_blocks_identifier_rotation() {
        let mut protection = LoginProtection::new(test_config());
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let now = Instant::now();

        for attempt in 0..5 {
            protection.record_failure_at(
                ip,
                LoginSurface::OrganizationAdmin,
                &format!("admin-{attempt}@example.com"),
                now,
            );
        }
        assert!(matches!(
            protection.check_at(
                ip,
                LoginSurface::OrganizationAdmin,
                "new-admin@example.com",
                now
            ),
            Err(AuthError::LoginRateLimited { .. })
        ));
    }

    #[test]
    fn success_clears_login_failures_and_lockout_expires() {
        let mut protection = LoginProtection::new(test_config());
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let now = Instant::now();

        protection.record_failure_at(ip, LoginSurface::Business, "user@example.com", now);
        protection.record_success(ip, LoginSurface::Business, "user@example.com");
        assert!(protection
            .check_at(ip, LoginSurface::Business, "user@example.com", now)
            .is_ok());

        for _ in 0..3 {
            protection.record_failure_at(ip, LoginSurface::Business, "user@example.com", now);
        }
        assert!(protection
            .check_at(
                ip,
                LoginSurface::Business,
                "user@example.com",
                now + Duration::from_secs(121)
            )
            .is_ok());
    }
}
