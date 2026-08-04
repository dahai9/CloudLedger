use std::{
    collections::HashMap,
    net::IpAddr,
    sync::Mutex,
    time::{Duration, Instant},
};

use hmac::{Hmac, Mac};
use sha2::Sha256;
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;

use crate::{auth::AuthError, config::LoginSecurityConfig};

const MAX_TRACKED_KEYS: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LoginSurface {
    Business,
    OrganizationAdmin,
    Platform,
    AdminAuthorization,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecurityRateKind {
    Refresh,
    InvalidBearer,
    AnonymousProbe,
}

#[derive(Debug, Clone)]
pub struct LoginProtectionConfig {
    turnstile_after_failures: u32,
    max_failures_per_login: u32,
    max_failures_per_ip: u32,
    window: Duration,
    lockout: Duration,
}

impl Default for LoginProtectionConfig {
    fn default() -> Self {
        Self::from_settings(&LoginSecurityConfig::default())
    }
}

impl LoginProtectionConfig {
    pub fn from_settings(settings: &LoginSecurityConfig) -> Self {
        Self {
            turnstile_after_failures: settings.turnstile_after_failures,
            max_failures_per_login: settings.max_failures_per_login,
            max_failures_per_ip: settings.max_failures_per_ip,
            window: Duration::from_secs(settings.window_seconds),
            lockout: Duration::from_secs(settings.lockout_seconds),
        }
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

    fn challenge_required(&mut self, ip: IpAddr, surface: LoginSurface, identifier: &str) -> bool {
        self.prune(Instant::now());
        self.failures_by_login
            .get(&LoginKey::new(ip, surface, identifier))
            .is_some_and(|state| state.failures >= self.config.turnstile_after_failures)
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

#[derive(Debug)]
pub struct SharedLoginProtection {
    backend: SharedBackend,
    config: LoginProtectionConfig,
    identifier_hmac_key: [u8; 32],
    memory_security_limits: Mutex<HashMap<(SecurityRateKind, IpAddr), FailureState>>,
}

#[derive(Debug)]
enum SharedBackend {
    Postgres(PgPool),
    Memory(Mutex<LoginProtection>),
}

impl SharedLoginProtection {
    pub fn postgres(
        pool: PgPool,
        config: LoginProtectionConfig,
        identifier_hmac_key: [u8; 32],
    ) -> Self {
        Self {
            backend: SharedBackend::Postgres(pool),
            config,
            identifier_hmac_key,
            memory_security_limits: Mutex::new(HashMap::new()),
        }
    }

    pub fn memory(config: LoginProtectionConfig, identifier_hmac_key: [u8; 32]) -> Self {
        Self {
            backend: SharedBackend::Memory(Mutex::new(LoginProtection::new(config.clone()))),
            config,
            identifier_hmac_key,
            memory_security_limits: Mutex::new(HashMap::new()),
        }
    }

    pub async fn check(
        &self,
        ip: IpAddr,
        surface: LoginSurface,
        identifier: &str,
    ) -> anyhow::Result<Result<(), AuthError>> {
        match &self.backend {
            SharedBackend::Memory(protection) => Ok(protection
                .lock()
                .map_err(|_| anyhow::anyhow!("login protection lock poisoned"))?
                .check(ip, surface, identifier)),
            SharedBackend::Postgres(pool) => {
                let identifier_hmac = self.identifier_hmac(surface, identifier);
                let now = OffsetDateTime::now_utc();
                let blocked_until: Option<OffsetDateTime> = sqlx::query_scalar(
                    "SELECT MAX(blocked_until) FROM login_failure_buckets WHERE ((surface = $1 AND bucket_kind = 'login' AND client_ip = $2::inet AND identifier_hmac = $3) OR (surface = 'all' AND bucket_kind = 'ip' AND client_ip = $2::inet AND identifier_hmac = ''::bytea)) AND blocked_until > $4",
                )
                .bind(surface.as_str())
                .bind(ip.to_string())
                .bind(identifier_hmac)
                .bind(now)
                .fetch_one(pool)
                .await?;
                Ok(match blocked_until {
                    Some(until) => Err(retry_after_error(until, now)),
                    None => Ok(()),
                })
            }
        }
    }

    pub async fn challenge_required(
        &self,
        ip: IpAddr,
        surface: LoginSurface,
        identifier: &str,
    ) -> anyhow::Result<bool> {
        match &self.backend {
            SharedBackend::Memory(protection) => Ok(protection
                .lock()
                .map_err(|_| anyhow::anyhow!("login protection lock poisoned"))?
                .challenge_required(ip, surface, identifier)),
            SharedBackend::Postgres(pool) => {
                let failures: Option<i32> = sqlx::query_scalar(
                    "SELECT failure_count FROM login_failure_buckets WHERE surface = $1 AND bucket_kind = 'login' AND client_ip = $2::inet AND identifier_hmac = $3 AND window_started_at > $4",
                )
                .bind(surface.as_str())
                .bind(ip.to_string())
                .bind(self.identifier_hmac(surface, identifier))
                .bind(OffsetDateTime::now_utc() - duration_to_time(self.config.window))
                .fetch_optional(pool)
                .await?;
                Ok(failures.is_some_and(|failures| {
                    failures >= self.config.turnstile_after_failures as i32
                }))
            }
        }
    }

    pub async fn record_failure(
        &self,
        ip: IpAddr,
        surface: LoginSurface,
        identifier: &str,
    ) -> anyhow::Result<Option<AuthError>> {
        match &self.backend {
            SharedBackend::Memory(protection) => Ok(protection
                .lock()
                .map_err(|_| anyhow::anyhow!("login protection lock poisoned"))?
                .record_failure(ip, surface, identifier)),
            SharedBackend::Postgres(pool) => {
                let now = OffsetDateTime::now_utc();
                let mut transaction = pool.begin().await?;
                let login_blocked = upsert_failure_bucket(
                    &mut transaction,
                    FailureBucket {
                        surface: surface.as_str(),
                        kind: "login",
                        ip,
                        identifier_hmac: self.identifier_hmac(surface, identifier),
                        limit: self.config.max_failures_per_login,
                    },
                    &self.config,
                    now,
                )
                .await?;
                let ip_blocked = upsert_failure_bucket(
                    &mut transaction,
                    FailureBucket {
                        surface: "all",
                        kind: "ip",
                        ip,
                        identifier_hmac: Vec::new(),
                        limit: self.config.max_failures_per_ip,
                    },
                    &self.config,
                    now,
                )
                .await?;
                cleanup_expired(&mut transaction, &self.config, now).await?;
                transaction.commit().await?;
                Ok(max_timestamp(login_blocked, ip_blocked)
                    .map(|until| retry_after_error(until, now)))
            }
        }
    }

    pub async fn record_success(
        &self,
        ip: IpAddr,
        surface: LoginSurface,
        identifier: &str,
    ) -> anyhow::Result<()> {
        match &self.backend {
            SharedBackend::Memory(protection) => {
                protection
                    .lock()
                    .map_err(|_| anyhow::anyhow!("login protection lock poisoned"))?
                    .record_success(ip, surface, identifier);
            }
            SharedBackend::Postgres(pool) => {
                sqlx::query(
                    "DELETE FROM login_failure_buckets WHERE surface = $1 AND bucket_kind = 'login' AND client_ip = $2::inet AND identifier_hmac = $3",
                )
                .bind(surface.as_str())
                .bind(ip.to_string())
                .bind(self.identifier_hmac(surface, identifier))
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub fn identifier_hmac(&self, surface: LoginSurface, identifier: &str) -> Vec<u8> {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.identifier_hmac_key)
            .expect("HMAC accepts a 32-byte key");
        mac.update(surface.as_str().as_bytes());
        mac.update(b"\0");
        mac.update(identifier.trim().to_lowercase().as_bytes());
        mac.finalize().into_bytes().to_vec()
    }

    pub async fn check_security_request(
        &self,
        kind: SecurityRateKind,
        ip: IpAddr,
    ) -> anyhow::Result<Result<(), AuthError>> {
        let (limit, window, lockout) = security_rate_policy(kind);
        match &self.backend {
            SharedBackend::Memory(_) => {
                let mut limits = self
                    .memory_security_limits
                    .lock()
                    .map_err(|_| anyhow::anyhow!("security rate-limit lock poisoned"))?;
                make_room(&mut limits);
                let now = Instant::now();
                let retry = record_failure_state(
                    limits
                        .entry((kind, ip))
                        .or_insert_with(|| FailureState::new(now)),
                    now,
                    window,
                    lockout,
                    limit,
                );
                Ok(match retry {
                    Some(retry) => Err(rate_limit_error(retry)),
                    None => Ok(()),
                })
            }
            SharedBackend::Postgres(pool) => {
                let now = OffsetDateTime::now_utc();
                let cutoff = now - duration_to_time(window);
                let blocked_until = now + duration_to_time(lockout);
                let result: Option<OffsetDateTime> = sqlx::query_scalar(
                    "INSERT INTO security_rate_limits (bucket_kind, client_ip, request_count, window_started_at, blocked_until, last_seen_at) VALUES ($1, $2::inet, 1, $3, NULL, $3) ON CONFLICT (bucket_kind, client_ip) DO UPDATE SET request_count = CASE WHEN security_rate_limits.blocked_until > $3 THEN security_rate_limits.request_count WHEN security_rate_limits.window_started_at <= $4 THEN 1 ELSE security_rate_limits.request_count + 1 END, window_started_at = CASE WHEN security_rate_limits.window_started_at <= $4 AND (security_rate_limits.blocked_until IS NULL OR security_rate_limits.blocked_until <= $3) THEN $3 ELSE security_rate_limits.window_started_at END, blocked_until = CASE WHEN security_rate_limits.blocked_until > $3 THEN security_rate_limits.blocked_until WHEN security_rate_limits.window_started_at <= $4 THEN NULL WHEN security_rate_limits.request_count + 1 > $5 THEN $6 ELSE NULL END, last_seen_at = $3 RETURNING blocked_until",
                )
                .bind(kind.as_str())
                .bind(ip.to_string())
                .bind(now)
                .bind(cutoff)
                .bind(limit as i32)
                .bind(blocked_until)
                .fetch_one(pool)
                .await?;
                sqlx::query(
                    "DELETE FROM security_rate_limits WHERE last_seen_at < $1 AND (blocked_until IS NULL OR blocked_until < $2)",
                )
                .bind(now - duration_to_time(window + lockout))
                .bind(now)
                .execute(pool)
                .await?;
                Ok(match result.filter(|until| *until > now) {
                    Some(until) => Err(retry_after_error(until, now)),
                    None => Ok(()),
                })
            }
        }
    }
}

impl LoginSurface {
    fn as_str(self) -> &'static str {
        match self {
            Self::Business => "business",
            Self::OrganizationAdmin => "organization_admin",
            Self::Platform => "platform",
            Self::AdminAuthorization => "admin_authorization",
        }
    }
}

impl SecurityRateKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Refresh => "refresh",
            Self::InvalidBearer => "invalid_bearer",
            Self::AnonymousProbe => "anonymous_probe",
        }
    }
}

fn security_rate_policy(kind: SecurityRateKind) -> (u32, Duration, Duration) {
    match kind {
        SecurityRateKind::Refresh => (30, Duration::from_secs(60), Duration::from_secs(5 * 60)),
        SecurityRateKind::InvalidBearer => {
            (60, Duration::from_secs(60), Duration::from_secs(10 * 60))
        }
        SecurityRateKind::AnonymousProbe => {
            (120, Duration::from_secs(60), Duration::from_secs(10 * 60))
        }
    }
}

struct FailureBucket<'a> {
    surface: &'a str,
    kind: &'a str,
    ip: IpAddr,
    identifier_hmac: Vec<u8>,
    limit: u32,
}

async fn upsert_failure_bucket(
    transaction: &mut Transaction<'_, Postgres>,
    bucket: FailureBucket<'_>,
    config: &LoginProtectionConfig,
    now: OffsetDateTime,
) -> anyhow::Result<Option<OffsetDateTime>> {
    let window_cutoff = now - duration_to_time(config.window);
    let blocked_until = now + duration_to_time(config.lockout);
    let result: Option<OffsetDateTime> = sqlx::query_scalar(
        "INSERT INTO login_failure_buckets (surface, bucket_kind, client_ip, identifier_hmac, failure_count, window_started_at, blocked_until, last_seen_at) VALUES ($1, $2, $3::inet, $4, 1, $5, CASE WHEN 1 >= $6 THEN $7 ELSE NULL END, $5) ON CONFLICT (surface, bucket_kind, client_ip, identifier_hmac) DO UPDATE SET failure_count = CASE WHEN login_failure_buckets.blocked_until > $5 THEN login_failure_buckets.failure_count WHEN login_failure_buckets.window_started_at <= $8 THEN 1 ELSE login_failure_buckets.failure_count + 1 END, window_started_at = CASE WHEN login_failure_buckets.blocked_until <= $5 OR login_failure_buckets.blocked_until IS NULL THEN CASE WHEN login_failure_buckets.window_started_at <= $8 THEN $5 ELSE login_failure_buckets.window_started_at END ELSE login_failure_buckets.window_started_at END, blocked_until = CASE WHEN login_failure_buckets.blocked_until > $5 THEN login_failure_buckets.blocked_until WHEN login_failure_buckets.window_started_at <= $8 THEN CASE WHEN 1 >= $6 THEN $7 ELSE NULL END WHEN login_failure_buckets.failure_count + 1 >= $6 THEN $7 ELSE NULL END, last_seen_at = $5 RETURNING blocked_until",
    )
    .bind(bucket.surface)
    .bind(bucket.kind)
    .bind(bucket.ip.to_string())
    .bind(bucket.identifier_hmac)
    .bind(now)
    .bind(bucket.limit as i32)
    .bind(blocked_until)
    .bind(window_cutoff)
    .fetch_one(&mut **transaction)
    .await?;
    Ok(result.filter(|until| *until > now))
}

async fn cleanup_expired(
    transaction: &mut Transaction<'_, Postgres>,
    config: &LoginProtectionConfig,
    now: OffsetDateTime,
) -> anyhow::Result<()> {
    let cutoff = now - duration_to_time(config.window + config.lockout);
    sqlx::query(
        "DELETE FROM login_failure_buckets WHERE last_seen_at < $1 AND (blocked_until IS NULL OR blocked_until < $2)",
    )
    .bind(cutoff)
    .bind(now)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn duration_to_time(duration: Duration) -> time::Duration {
    time::Duration::seconds(duration.as_secs().min(i64::MAX as u64) as i64)
}

fn retry_after_error(until: OffsetDateTime, now: OffsetDateTime) -> AuthError {
    AuthError::LoginRateLimited {
        retry_after_seconds: (until - now).whole_seconds().max(1) as u64,
    }
}

fn max_timestamp(
    left: Option<OffsetDateTime>,
    right: Option<OffsetDateTime>,
) -> Option<OffsetDateTime> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn test_config() -> LoginProtectionConfig {
        LoginProtectionConfig {
            turnstile_after_failures: 2,
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
