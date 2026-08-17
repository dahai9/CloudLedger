use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

pub const MIN_PASSWORD_LENGTH: usize = 12;
pub const MAX_PASSWORD_LENGTH: usize = 128;
const APP_ACCESS_TOKEN_TTL: Duration = Duration::minutes(15);
const APP_REFRESH_TOKEN_TTL: Duration = Duration::days(30);
const ADMIN_SESSION_TTL: Duration = Duration::hours(8);

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AuthError {
    #[error("email is already registered")]
    EmailAlreadyRegistered,
    #[error("phone is already registered")]
    PhoneAlreadyRegistered,
    #[error("email or phone is required")]
    LoginIdentifierRequired,
    #[error("display name is required")]
    DisplayNameRequired,
    #[error("password is required")]
    PasswordRequired,
    #[error("password must be between 12 and 128 characters")]
    PasswordPolicyViolation,
    #[error("installation id is required")]
    InstallationIdRequired,
    #[error("installation is already bound to another account")]
    InstallationAlreadyBound,
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("too many login attempts; try again in {retry_after_seconds} seconds")]
    LoginRateLimited { retry_after_seconds: u64 },
    #[error("turnstile_required")]
    TurnstileRequired,
    #[error("organization admin accounts cannot use the business app")]
    BusinessAppAccessDenied,
    #[error("business accounts cannot use the organization admin backend")]
    AdminAccessDenied,
    #[error("organization admin account is missing its organization")]
    AdminOrganizationRequired,
    #[error("session was not found")]
    SessionNotFound,
    #[error("refresh token replay detected; session family revoked")]
    RefreshReplayDetected,
    #[error("password hashing failed")]
    PasswordHashFailed,
    #[error("storage error: {0}")]
    Storage(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthUserDto {
    pub id: Uuid,
    pub display_name: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub account_kind: AccountKind,
    pub organization_id: Option<Uuid>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind {
    #[default]
    Business,
    OrganizationAdmin,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub user: AuthUserDto,
    pub access_token: String,
    pub refresh_token: String,
    pub installation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthenticatedSession {
    pub user: AuthUserDto,
    pub installation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminSession {
    pub user: AuthUserDto,
    pub access_token: String,
    pub organization_id: Uuid,
}

#[derive(Debug, Clone)]
pub struct AdminAuthenticatedSession {
    pub user: AuthUserDto,
    pub organization_id: Uuid,
}

#[derive(Debug, Clone)]
pub struct RegisterInput {
    pub user_id: Option<Uuid>,
    pub display_name: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub password: String,
    pub installation_id: String,
}

#[derive(Debug, Clone)]
pub struct AdminCreateUserInput {
    pub user_id: Uuid,
    pub display_name: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub password: Option<String>,
    pub account_kind: AccountKind,
    pub organization_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct LoginInput {
    pub email: Option<String>,
    pub phone: Option<String>,
    pub password: String,
    pub installation_id: String,
}

#[derive(Debug, Clone)]
pub struct AdminLoginInput {
    pub email: Option<String>,
    pub phone: Option<String>,
    pub password: String,
}

#[derive(Debug, Clone)]
pub struct RefreshInput {
    pub refresh_token: String,
    pub installation_id: String,
}

#[derive(Debug, Clone)]
pub struct UpdateProfileInput {
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StoredUser {
    pub(crate) id: Uuid,
    pub(crate) display_name: String,
    pub(crate) email: Option<String>,
    pub(crate) phone: Option<String>,
    pub(crate) password_hash: String,
    #[serde(default)]
    pub(crate) account_kind: AccountKind,
    #[serde(default)]
    pub(crate) organization_id: Option<Uuid>,
    pub(crate) created_at: OffsetDateTime,
    #[serde(default = "unix_epoch")]
    pub(crate) updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SessionKind {
    #[default]
    #[serde(alias = "app")]
    Tauri,
    Web,
    Admin,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StoredSession {
    #[serde(default)]
    pub(crate) id: Uuid,
    #[serde(default)]
    pub(crate) family_id: Uuid,
    pub(crate) user_id: Uuid,
    pub(crate) installation_id: String,
    /// Hex-encoded SHA-256 digest. Raw bearer material never enters snapshots.
    pub(crate) access_token: String,
    pub(crate) refresh_token: String,
    #[serde(default)]
    pub(crate) kind: SessionKind,
    pub(crate) created_at: OffsetDateTime,
    #[serde(default = "unix_epoch")]
    pub(crate) access_expires_at: OffsetDateTime,
    #[serde(default)]
    pub(crate) refresh_expires_at: Option<OffsetDateTime>,
    #[serde(default)]
    pub(crate) rotated_at: Option<OffsetDateTime>,
    #[serde(default)]
    pub(crate) revoked_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthService {
    #[serde(default)]
    users_by_id: BTreeMap<Uuid, StoredUser>,
    #[serde(default)]
    user_ids_by_email: BTreeMap<String, Uuid>,
    #[serde(default)]
    user_ids_by_phone: BTreeMap<String, Uuid>,
    #[serde(default)]
    installations_by_id: BTreeMap<String, Uuid>,
    #[serde(default)]
    sessions_by_access_token: BTreeMap<String, StoredSession>,
    #[serde(default)]
    access_tokens_by_refresh_token: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct AuthSnapshot {
    pub(crate) users: Vec<StoredUser>,
    pub(crate) installations: Vec<(String, Uuid)>,
    pub(crate) sessions: Vec<StoredSession>,
}

impl AuthService {
    pub(crate) fn snapshot(&self) -> AuthSnapshot {
        AuthSnapshot {
            users: self.users_by_id.values().cloned().collect(),
            installations: self
                .installations_by_id
                .iter()
                .map(|(installation_id, user_id)| (installation_id.clone(), *user_id))
                .collect(),
            sessions: self.sessions_by_access_token.values().cloned().collect(),
        }
    }

    pub(crate) fn from_snapshot(snapshot: AuthSnapshot) -> Self {
        let users_by_id: BTreeMap<_, _> = snapshot
            .users
            .into_iter()
            .map(|user| (user.id, user))
            .collect();
        let sessions_by_access_token: BTreeMap<_, _> = snapshot
            .sessions
            .into_iter()
            .map(|session| (session.access_token.clone(), session))
            .collect();
        let mut service = Self {
            user_ids_by_email: users_by_id
                .values()
                .filter_map(|user| user.email.clone().map(|email| (email, user.id)))
                .collect(),
            user_ids_by_phone: users_by_id
                .values()
                .filter_map(|user| user.phone.clone().map(|phone| (phone, user.id)))
                .collect(),
            installations_by_id: snapshot.installations.into_iter().collect(),
            access_tokens_by_refresh_token: sessions_by_access_token
                .values()
                .filter(|session| !session.refresh_token.is_empty())
                .map(|session| (session.refresh_token.clone(), session.access_token.clone()))
                .collect(),
            users_by_id,
            sessions_by_access_token,
        };
        service.prune_expired_sessions();
        service
    }

    pub fn load_or_default(path: impl AsRef<Path>) -> Result<Self, AuthError> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::default());
        }

        let document = fs::read_to_string(path)
            .map_err(|err| AuthError::Storage(format!("read {}: {err}", path.display())))?;
        let mut service: Self = serde_json::from_str(&document)
            .map_err(|err| AuthError::Storage(format!("parse {}: {err}", path.display())))?;
        service.prune_expired_sessions();
        Ok(service)
    }

    pub fn save_to_path(&self, path: impl AsRef<Path>) -> Result<(), AuthError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| AuthError::Storage(format!("create {}: {err}", parent.display())))?;
        }

        let document = serde_json::to_string_pretty(self)
            .map_err(|err| AuthError::Storage(format!("serialize auth state: {err}")))?;
        let temporary_path = sidecar_path(path, ".tmp");
        {
            let mut file = fs::File::create(&temporary_path).map_err(|err| {
                AuthError::Storage(format!("create {}: {err}", temporary_path.display()))
            })?;
            file.write_all(document.as_bytes()).map_err(|err| {
                AuthError::Storage(format!("write {}: {err}", temporary_path.display()))
            })?;
            file.sync_all().map_err(|err| {
                AuthError::Storage(format!("sync {}: {err}", temporary_path.display()))
            })?;
        }
        fs::rename(&temporary_path, path).map_err(|err| {
            AuthError::Storage(format!(
                "replace {} with {}: {err}",
                path.display(),
                temporary_path.display()
            ))
        })?;
        Ok(())
    }

    pub fn register(&mut self, input: RegisterInput) -> Result<Session, AuthError> {
        let display_name = normalize_display_name(&input.display_name)?;
        let email = input.email.and_then(normalize_optional_email);
        let phone = input.phone.and_then(normalize_optional_phone);
        let password = validate_new_password(&input.password)?;
        require_login_identifier(&email, &phone)?;
        let installation_id = normalize_installation_id(&input.installation_id)?;

        if email
            .as_deref()
            .is_some_and(|email| self.user_ids_by_email.contains_key(email))
        {
            return Err(AuthError::EmailAlreadyRegistered);
        }
        if phone
            .as_deref()
            .is_some_and(|phone| self.user_ids_by_phone.contains_key(phone))
        {
            return Err(AuthError::PhoneAlreadyRegistered);
        }

        let now = OffsetDateTime::now_utc();
        let user = StoredUser {
            id: input.user_id.unwrap_or_else(Uuid::new_v4),
            display_name,
            email,
            phone,
            password_hash: hash_password(&password)?,
            account_kind: AccountKind::Business,
            organization_id: None,
            created_at: now,
            updated_at: now,
        };
        self.bind_installation(&installation_id, user.id)?;
        if let Some(email) = &user.email {
            self.user_ids_by_email.insert(email.clone(), user.id);
        }
        if let Some(phone) = &user.phone {
            self.user_ids_by_phone.insert(phone.clone(), user.id);
        }
        let user_id = user.id;
        self.users_by_id.insert(user_id, user);
        self.issue_session(user_id, installation_id, Uuid::new_v4(), SessionKind::Tauri)
    }

    pub fn create_or_update_admin_user(
        &mut self,
        input: AdminCreateUserInput,
    ) -> Result<AuthUserDto, AuthError> {
        let display_name = normalize_display_name(&input.display_name)?;
        let email = input.email.and_then(normalize_optional_email);
        let phone = input.phone.and_then(normalize_optional_phone);
        let password = input
            .password
            .as_deref()
            .map(validate_new_password)
            .transpose()?;
        let password_was_updated = password.is_some();
        require_login_identifier(&email, &phone)?;
        self.ensure_identifier_available(input.user_id, email.as_deref(), phone.as_deref())?;

        let existing_user = self.users_by_id.get(&input.user_id).cloned();
        let password_hash = match (existing_user.as_ref(), password) {
            (Some(existing), None) => existing.password_hash.clone(),
            (_, Some(password)) => hash_password(&password)?,
            (None, None) => return Err(AuthError::PasswordRequired),
        };
        let created_at = existing_user
            .as_ref()
            .map(|user| user.created_at)
            .unwrap_or_else(OffsetDateTime::now_utc);

        if let Some(existing) = &existing_user {
            if let Some(email) = &existing.email {
                if self.user_ids_by_email.get(email) == Some(&input.user_id) {
                    self.user_ids_by_email.remove(email);
                }
            }
            if let Some(phone) = &existing.phone {
                if self.user_ids_by_phone.get(phone) == Some(&input.user_id) {
                    self.user_ids_by_phone.remove(phone);
                }
            }
        }

        let user = StoredUser {
            id: input.user_id,
            display_name,
            email,
            phone,
            password_hash,
            account_kind: input.account_kind,
            organization_id: input.organization_id,
            created_at,
            updated_at: OffsetDateTime::now_utc(),
        };
        if let Some(email) = &user.email {
            self.user_ids_by_email.insert(email.clone(), user.id);
        }
        if let Some(phone) = &user.phone {
            self.user_ids_by_phone.insert(phone.clone(), user.id);
        }
        let user_id = user.id;
        self.users_by_id.insert(user_id, user);
        if password_was_updated
            || existing_user.as_ref().is_some_and(|existing| {
                existing.account_kind != input.account_kind
                    || existing.organization_id != input.organization_id
            })
        {
            self.revoke_user_sessions(user_id);
        }
        self.users_by_id
            .get(&user_id)
            .map(auth_user_dto)
            .ok_or(AuthError::InvalidCredentials)
    }

    pub fn login(&mut self, input: LoginInput) -> Result<Session, AuthError> {
        self.login_for_client(input, SessionKind::Tauri)
    }

    pub(crate) fn login_for_client(
        &mut self,
        input: LoginInput,
        client_kind: SessionKind,
    ) -> Result<Session, AuthError> {
        debug_assert!(client_kind != SessionKind::Admin);
        let email = input.email.and_then(normalize_optional_email);
        let phone = input.phone.and_then(normalize_optional_phone);
        require_login_identifier(&email, &phone)?;
        let installation_id = normalize_installation_id(&input.installation_id)?;
        let Some(user_id) = self.find_user_id(email.as_deref(), phone.as_deref()) else {
            consume_unknown_user_password_work(&input.password)?;
            return Err(AuthError::InvalidCredentials);
        };
        let user = self
            .users_by_id
            .get(&user_id)
            .ok_or(AuthError::InvalidCredentials)?;
        verify_password(&input.password, &user.password_hash)?;
        if user.account_kind != AccountKind::Business {
            return Err(AuthError::BusinessAppAccessDenied);
        }
        self.bind_installation(&installation_id, user_id)?;
        self.issue_session(user_id, installation_id, Uuid::new_v4(), client_kind)
    }

    pub fn admin_login(&mut self, input: AdminLoginInput) -> Result<AdminSession, AuthError> {
        let email = input.email.and_then(normalize_optional_email);
        let phone = input.phone.and_then(normalize_optional_phone);
        require_login_identifier(&email, &phone)?;
        let Some(user_id) = self.find_user_id(email.as_deref(), phone.as_deref()) else {
            consume_unknown_user_password_work(&input.password)?;
            return Err(AuthError::InvalidCredentials);
        };
        let user = self
            .users_by_id
            .get(&user_id)
            .ok_or(AuthError::InvalidCredentials)?;
        verify_password(&input.password, &user.password_hash)?;
        if user.account_kind != AccountKind::OrganizationAdmin {
            return Err(AuthError::AdminAccessDenied);
        }
        self.issue_admin_session(user_id)
    }

    pub fn change_admin_password(
        &mut self,
        access_token: &str,
        current_password: &str,
        new_password: &str,
    ) -> Result<AdminAuthenticatedSession, AuthError> {
        let access_token = normalize_bearer_token(access_token);
        let access_token = token_digest(access_token);
        let session = self
            .sessions_by_access_token
            .get(&access_token)
            .cloned()
            .ok_or(AuthError::InvalidCredentials)?;
        if session.kind != SessionKind::Admin || session.revoked_at.is_some() {
            return Err(AuthError::InvalidCredentials);
        }
        if OffsetDateTime::now_utc() >= session.access_expires_at {
            return Err(AuthError::InvalidCredentials);
        }

        let user = self
            .users_by_id
            .get(&session.user_id)
            .cloned()
            .ok_or(AuthError::InvalidCredentials)?;
        if user.account_kind != AccountKind::OrganizationAdmin {
            return Err(AuthError::AdminAccessDenied);
        }
        let organization_id = user
            .organization_id
            .ok_or(AuthError::AdminOrganizationRequired)?;
        verify_password(current_password, &user.password_hash)?;
        let new_password = validate_new_password(new_password)?;
        let password_hash = hash_password(&new_password)?;

        let updated_user = self
            .users_by_id
            .get_mut(&user.id)
            .ok_or(AuthError::InvalidCredentials)?;
        updated_user.password_hash = password_hash;
        updated_user.updated_at = OffsetDateTime::now_utc();
        self.revoke_user_sessions(user.id);

        let updated_user = self
            .users_by_id
            .get(&user.id)
            .ok_or(AuthError::InvalidCredentials)?;
        Ok(AdminAuthenticatedSession {
            user: auth_user_dto(updated_user),
            organization_id,
        })
    }

    pub fn reset_organization_admin_password(
        &mut self,
        user_id: Uuid,
        new_password: &str,
    ) -> Result<AuthUserDto, AuthError> {
        let new_password = validate_new_password(new_password)?;
        let user = self
            .users_by_id
            .get(&user_id)
            .cloned()
            .ok_or(AuthError::InvalidCredentials)?;
        if user.account_kind != AccountKind::OrganizationAdmin {
            return Err(AuthError::AdminAccessDenied);
        }
        if user.organization_id.is_none() {
            return Err(AuthError::AdminOrganizationRequired);
        }
        let password_hash = hash_password(&new_password)?;
        let updated_user = self
            .users_by_id
            .get_mut(&user_id)
            .ok_or(AuthError::InvalidCredentials)?;
        updated_user.password_hash = password_hash;
        updated_user.updated_at = OffsetDateTime::now_utc();
        self.revoke_user_sessions(user_id);
        self.users_by_id
            .get(&user_id)
            .map(auth_user_dto)
            .ok_or(AuthError::InvalidCredentials)
    }

    pub fn refresh(&mut self, input: RefreshInput) -> Result<Session, AuthError> {
        self.refresh_for_client(input, SessionKind::Tauri)
    }

    pub(crate) fn refresh_for_client(
        &mut self,
        input: RefreshInput,
        client_kind: SessionKind,
    ) -> Result<Session, AuthError> {
        let installation_id = normalize_installation_id(&input.installation_id)?;
        let refresh_hash = token_digest(input.refresh_token.trim());
        let access_token = self
            .access_tokens_by_refresh_token
            .get(&refresh_hash)
            .cloned()
            .ok_or(AuthError::SessionNotFound)?;
        let session = self
            .sessions_by_access_token
            .get(&access_token)
            .cloned()
            .ok_or(AuthError::SessionNotFound)?;
        if session.kind != client_kind {
            return Err(AuthError::SessionNotFound);
        }
        if session.rotated_at.is_some() || session.revoked_at.is_some() {
            self.revoke_family(session.family_id);
            return Err(AuthError::RefreshReplayDetected);
        }
        if session
            .refresh_expires_at
            .is_none_or(|expires_at| OffsetDateTime::now_utc() >= expires_at)
        {
            return Err(AuthError::SessionNotFound);
        }
        if session.installation_id != installation_id {
            return Err(AuthError::InstallationAlreadyBound);
        }
        if let Some(stored) = self.sessions_by_access_token.get_mut(&access_token) {
            stored.rotated_at = Some(OffsetDateTime::now_utc());
        }
        self.issue_session(
            session.user_id,
            installation_id,
            session.family_id,
            client_kind,
        )
    }

    pub fn logout(&mut self, access_token: &str) -> Result<(), AuthError> {
        let access_token = normalize_bearer_token(access_token);
        let access_token = token_digest(access_token);
        let session = self
            .sessions_by_access_token
            .get(&access_token)
            .cloned()
            .ok_or(AuthError::SessionNotFound)?;
        self.revoke_installation_sessions(&session);
        Ok(())
    }

    pub fn logout_refresh(&mut self, refresh_token: &str) -> Result<Uuid, AuthError> {
        let refresh_hash = token_digest(refresh_token);
        let access_hash = self
            .access_tokens_by_refresh_token
            .get(&refresh_hash)
            .ok_or(AuthError::SessionNotFound)?;
        let session = self
            .sessions_by_access_token
            .get(access_hash)
            .cloned()
            .ok_or(AuthError::SessionNotFound)?;
        self.revoke_installation_sessions(&session);
        Ok(session.user_id)
    }

    pub fn authenticate_access_token(
        &self,
        access_token: &str,
    ) -> Result<AuthenticatedSession, AuthError> {
        let access_token = normalize_bearer_token(access_token);
        let access_token = token_digest(access_token);
        let session = self
            .sessions_by_access_token
            .get(&access_token)
            .ok_or(AuthError::InvalidCredentials)?;
        if !matches!(session.kind, SessionKind::Tauri | SessionKind::Web)
            || session.rotated_at.is_some()
            || session.revoked_at.is_some()
        {
            return Err(AuthError::InvalidCredentials);
        }
        if OffsetDateTime::now_utc() >= session.access_expires_at {
            return Err(AuthError::InvalidCredentials);
        }
        let user = self
            .users_by_id
            .get(&session.user_id)
            .ok_or(AuthError::InvalidCredentials)?;
        if user.account_kind != AccountKind::Business {
            return Err(AuthError::BusinessAppAccessDenied);
        }
        Ok(AuthenticatedSession {
            user: auth_user_dto(user),
            installation_id: session.installation_id.clone(),
        })
    }

    pub fn authenticate_admin_access_token(
        &self,
        access_token: &str,
    ) -> Result<AdminAuthenticatedSession, AuthError> {
        let access_token = normalize_bearer_token(access_token);
        let access_token = token_digest(access_token);
        let session = self
            .sessions_by_access_token
            .get(&access_token)
            .ok_or(AuthError::InvalidCredentials)?;
        if session.kind != SessionKind::Admin || session.revoked_at.is_some() {
            return Err(AuthError::InvalidCredentials);
        }
        if OffsetDateTime::now_utc() >= session.access_expires_at {
            return Err(AuthError::InvalidCredentials);
        }
        let user = self
            .users_by_id
            .get(&session.user_id)
            .ok_or(AuthError::InvalidCredentials)?;
        if user.account_kind != AccountKind::OrganizationAdmin {
            return Err(AuthError::AdminAccessDenied);
        }
        let organization_id = user
            .organization_id
            .ok_or(AuthError::AdminOrganizationRequired)?;
        Ok(AdminAuthenticatedSession {
            user: auth_user_dto(user),
            organization_id,
        })
    }

    pub fn mark_organization_admin(&mut self, user_id: Uuid, organization_id: Uuid) -> bool {
        let Some(user) = self.users_by_id.get_mut(&user_id) else {
            return false;
        };
        if user.account_kind == AccountKind::OrganizationAdmin
            && user.organization_id == Some(organization_id)
        {
            return false;
        }

        user.account_kind = AccountKind::OrganizationAdmin;
        user.organization_id = Some(organization_id);
        user.updated_at = OffsetDateTime::now_utc();
        self.revoke_user_sessions(user_id);
        true
    }

    pub fn account_kind(&self, user_id: Uuid) -> Option<AccountKind> {
        self.users_by_id.get(&user_id).map(|user| user.account_kind)
    }

    pub fn organization_id(&self, user_id: Uuid) -> Option<Uuid> {
        self.users_by_id
            .get(&user_id)
            .and_then(|user| user.organization_id)
    }

    pub fn update_profile(
        &mut self,
        access_token: &str,
        input: UpdateProfileInput,
    ) -> Result<AuthenticatedSession, AuthError> {
        let access_token = normalize_bearer_token(access_token);
        let access_token = token_digest(access_token);
        let session = self
            .sessions_by_access_token
            .get(&access_token)
            .ok_or(AuthError::InvalidCredentials)?
            .clone();
        if !matches!(session.kind, SessionKind::Tauri | SessionKind::Web)
            || session.rotated_at.is_some()
            || session.revoked_at.is_some()
        {
            return Err(AuthError::InvalidCredentials);
        }
        if OffsetDateTime::now_utc() >= session.access_expires_at {
            return Err(AuthError::InvalidCredentials);
        }
        let display_name = normalize_display_name(&input.display_name)?;
        let user = self
            .users_by_id
            .get_mut(&session.user_id)
            .ok_or(AuthError::InvalidCredentials)?;
        if user.account_kind != AccountKind::Business {
            return Err(AuthError::BusinessAppAccessDenied);
        }
        user.display_name = display_name;
        user.updated_at = OffsetDateTime::now_utc();
        Ok(AuthenticatedSession {
            user: auth_user_dto(user),
            installation_id: session.installation_id,
        })
    }

    fn find_user_id(&self, email: Option<&str>, phone: Option<&str>) -> Option<Uuid> {
        email
            .and_then(|email| self.user_ids_by_email.get(email).copied())
            .or_else(|| phone.and_then(|phone| self.user_ids_by_phone.get(phone).copied()))
    }

    fn ensure_identifier_available(
        &self,
        user_id: Uuid,
        email: Option<&str>,
        phone: Option<&str>,
    ) -> Result<(), AuthError> {
        if email
            .and_then(|email| self.user_ids_by_email.get(email))
            .is_some_and(|existing| *existing != user_id)
        {
            return Err(AuthError::EmailAlreadyRegistered);
        }
        if phone
            .and_then(|phone| self.user_ids_by_phone.get(phone))
            .is_some_and(|existing| *existing != user_id)
        {
            return Err(AuthError::PhoneAlreadyRegistered);
        }
        Ok(())
    }

    fn bind_installation(&mut self, installation_id: &str, user_id: Uuid) -> Result<(), AuthError> {
        match self.installations_by_id.get(installation_id) {
            Some(existing) if *existing != user_id => Err(AuthError::InstallationAlreadyBound),
            Some(_) => Ok(()),
            None => {
                self.installations_by_id
                    .insert(installation_id.to_string(), user_id);
                Ok(())
            }
        }
    }

    fn issue_session(
        &mut self,
        user_id: Uuid,
        installation_id: String,
        family_id: Uuid,
        kind: SessionKind,
    ) -> Result<Session, AuthError> {
        self.prune_expired_sessions();
        let user = self
            .users_by_id
            .get(&user_id)
            .ok_or(AuthError::InvalidCredentials)?;
        let now = OffsetDateTime::now_utc();
        let access_token = random_token();
        let refresh_token = random_token();
        let access_token_hash = token_digest(&access_token);
        let refresh_token_hash = token_digest(&refresh_token);
        let stored = StoredSession {
            id: Uuid::new_v4(),
            family_id,
            user_id,
            installation_id: installation_id.clone(),
            access_token: access_token_hash.clone(),
            refresh_token: refresh_token_hash.clone(),
            kind,
            created_at: now,
            access_expires_at: now + APP_ACCESS_TOKEN_TTL,
            refresh_expires_at: Some(now + APP_REFRESH_TOKEN_TTL),
            rotated_at: None,
            revoked_at: None,
        };
        self.access_tokens_by_refresh_token
            .insert(refresh_token_hash, access_token_hash.clone());
        self.sessions_by_access_token
            .insert(access_token_hash, stored);
        Ok(Session {
            user: auth_user_dto(user),
            access_token,
            refresh_token,
            installation_id,
        })
    }

    fn issue_admin_session(&mut self, user_id: Uuid) -> Result<AdminSession, AuthError> {
        self.prune_expired_sessions();
        let user = self
            .users_by_id
            .get(&user_id)
            .ok_or(AuthError::InvalidCredentials)?;
        let organization_id = user
            .organization_id
            .ok_or(AuthError::AdminOrganizationRequired)?;
        let now = OffsetDateTime::now_utc();
        let access_token = random_token();
        let access_token_hash = token_digest(&access_token);
        self.sessions_by_access_token.insert(
            access_token_hash.clone(),
            StoredSession {
                id: Uuid::new_v4(),
                family_id: Uuid::new_v4(),
                user_id,
                installation_id: String::new(),
                access_token: access_token_hash,
                refresh_token: String::new(),
                kind: SessionKind::Admin,
                created_at: now,
                access_expires_at: now + ADMIN_SESSION_TTL,
                refresh_expires_at: None,
                rotated_at: None,
                revoked_at: None,
            },
        );
        Ok(AdminSession {
            user: auth_user_dto(user),
            access_token,
            organization_id,
        })
    }

    fn revoke_user_sessions(&mut self, user_id: Uuid) {
        let now = OffsetDateTime::now_utc();
        for session in self.sessions_by_access_token.values_mut() {
            if session.user_id == user_id {
                session.revoked_at = Some(now);
            }
        }
        self.installations_by_id
            .retain(|_, installed_user_id| *installed_user_id != user_id);
    }

    fn revoke_installation_sessions(&mut self, session: &StoredSession) {
        let now = OffsetDateTime::now_utc();
        for stored in self.sessions_by_access_token.values_mut() {
            if stored.user_id == session.user_id
                && stored.installation_id == session.installation_id
                && stored.kind == session.kind
            {
                stored.revoked_at = Some(now);
            }
        }
        self.installations_by_id.remove(&session.installation_id);
    }

    fn prune_expired_sessions(&mut self) {
        let expired_access_tokens = self
            .sessions_by_access_token
            .iter()
            .filter_map(|(access_token, session)| {
                let expired = session
                    .refresh_expires_at
                    .unwrap_or(session.access_expires_at)
                    <= OffsetDateTime::now_utc();
                expired.then_some(access_token.clone())
            })
            .collect::<Vec<_>>();
        for access_token in expired_access_tokens {
            if let Some(session) = self.sessions_by_access_token.remove(&access_token) {
                self.access_tokens_by_refresh_token
                    .remove(&session.refresh_token);
            }
        }
    }

    fn revoke_family(&mut self, family_id: Uuid) {
        let now = OffsetDateTime::now_utc();
        for session in self.sessions_by_access_token.values_mut() {
            if session.family_id == family_id {
                session.revoked_at = Some(now);
            }
        }
    }
}

fn random_token() -> String {
    let mut token = [0_u8; 32];
    OsRng.fill_bytes(&mut token);
    URL_SAFE_NO_PAD.encode(token)
}

pub(crate) fn token_digest(token: &str) -> String {
    hex::encode(Sha256::digest(token.trim().as_bytes()))
}

fn unix_epoch() -> OffsetDateTime {
    OffsetDateTime::UNIX_EPOCH
}

fn hash_password(password: &str) -> Result<String, AuthError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| AuthError::PasswordHashFailed)
}

fn verify_password(password: &str, password_hash: &str) -> Result<(), AuthError> {
    let parsed = PasswordHash::new(password_hash).map_err(|_| AuthError::InvalidCredentials)?;
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .map_err(|_| AuthError::InvalidCredentials)
}

fn consume_unknown_user_password_work(password: &str) -> Result<(), AuthError> {
    hash_password(password).map(|_| ())
}

fn auth_user_dto(user: &StoredUser) -> AuthUserDto {
    AuthUserDto {
        id: user.id,
        display_name: user.display_name.clone(),
        email: user.email.clone(),
        phone: user.phone.clone(),
        account_kind: user.account_kind,
        organization_id: user.organization_id,
    }
}

fn normalize_display_name(value: &str) -> Result<String, AuthError> {
    let value = value.trim();
    if value.is_empty() {
        Err(AuthError::DisplayNameRequired)
    } else {
        Ok(value.to_string())
    }
}

fn validate_new_password(value: &str) -> Result<String, AuthError> {
    if value.trim().is_empty() {
        Err(AuthError::PasswordRequired)
    } else if !(MIN_PASSWORD_LENGTH..=MAX_PASSWORD_LENGTH).contains(&value.chars().count()) {
        Err(AuthError::PasswordPolicyViolation)
    } else {
        Ok(value.to_string())
    }
}

fn normalize_optional_email(email: String) -> Option<String> {
    let normalized = email.trim().to_lowercase();
    (!normalized.is_empty()).then_some(normalized)
}

fn normalize_optional_phone(phone: String) -> Option<String> {
    let normalized = phone.trim().to_string();
    (!normalized.is_empty()).then_some(normalized)
}

fn normalize_installation_id(value: &str) -> Result<String, AuthError> {
    let value = value.trim();
    if value.is_empty() {
        Err(AuthError::InstallationIdRequired)
    } else {
        Ok(value.to_string())
    }
}

fn normalize_bearer_token(value: &str) -> &str {
    value.trim().strip_prefix("Bearer ").unwrap_or(value.trim())
}

fn require_login_identifier(
    email: &Option<String>,
    phone: &Option<String>,
) -> Result<(), AuthError> {
    if email.is_none() && phone.is_none() {
        Err(AuthError::LoginIdentifierRequired)
    } else {
        Ok(())
    }
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "auth-state.json".into());
    path.with_file_name(format!("{file_name}{suffix}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_login_rotates_tokens() {
        let mut auth = AuthService::default();
        let first = auth
            .register(RegisterInput {
                user_id: None,
                display_name: "Alice".to_string(),
                email: Some("Alice@Example.com".to_string()),
                phone: None,
                password: "correct horse battery staple".to_string(),
                installation_id: "phone-1".to_string(),
            })
            .unwrap();
        let second = auth
            .login(LoginInput {
                email: Some("alice@example.com".to_string()),
                phone: None,
                password: "correct horse battery staple".to_string(),
                installation_id: "phone-1".to_string(),
            })
            .unwrap();

        assert_eq!(first.user.id, second.user.id);
        assert_ne!(first.access_token, second.access_token);
        assert_ne!(first.refresh_token, second.refresh_token);
    }

    #[test]
    fn admin_created_user_can_login_without_self_registration() {
        let mut auth = AuthService::default();
        let user_id = Uuid::new_v4();
        let created = auth
            .create_or_update_admin_user(AdminCreateUserInput {
                user_id,
                display_name: "Alice".to_string(),
                email: Some("Alice@Example.com".to_string()),
                phone: None,
                password: Some("correct horse battery staple".to_string()),
                account_kind: AccountKind::Business,
                organization_id: None,
            })
            .unwrap();
        let session = auth
            .login(LoginInput {
                email: Some("alice@example.com".to_string()),
                phone: None,
                password: "correct horse battery staple".to_string(),
                installation_id: "phone-1".to_string(),
            })
            .unwrap();

        assert_eq!(created.id, user_id);
        assert_eq!(session.user.id, user_id);
    }

    #[test]
    fn organization_admin_uses_only_admin_sessions() {
        let mut auth = AuthService::default();
        let organization_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        auth.create_or_update_admin_user(AdminCreateUserInput {
            user_id,
            display_name: "Organization Admin".to_string(),
            email: Some("admin@example.com".to_string()),
            phone: None,
            password: Some("correct horse battery staple".to_string()),
            account_kind: AccountKind::OrganizationAdmin,
            organization_id: Some(organization_id),
        })
        .unwrap();

        let app_login = auth.login(LoginInput {
            email: Some("admin@example.com".to_string()),
            phone: None,
            password: "correct horse battery staple".to_string(),
            installation_id: "phone-admin".to_string(),
        });
        assert_eq!(app_login.unwrap_err(), AuthError::BusinessAppAccessDenied);

        let admin_session = auth
            .admin_login(AdminLoginInput {
                email: Some("admin@example.com".to_string()),
                phone: None,
                password: "correct horse battery staple".to_string(),
            })
            .unwrap();
        let authenticated = auth
            .authenticate_admin_access_token(&admin_session.access_token)
            .unwrap();
        assert_eq!(authenticated.user.id, user_id);
        assert_eq!(authenticated.organization_id, organization_id);
        assert!(auth
            .authenticate_access_token(&admin_session.access_token)
            .is_err());

        auth.sessions_by_access_token
            .get_mut(&token_digest(&admin_session.access_token))
            .expect("stored admin session")
            .access_expires_at = OffsetDateTime::now_utc() - Duration::seconds(1);
        assert!(auth
            .authenticate_admin_access_token(&admin_session.access_token)
            .is_err());
    }

    #[test]
    fn organization_admin_can_change_password_and_old_sessions_are_revoked() {
        let mut auth = AuthService::default();
        let user_id = Uuid::new_v4();
        auth.create_or_update_admin_user(AdminCreateUserInput {
            user_id,
            display_name: "Organization Admin".to_string(),
            email: Some("admin@example.com".to_string()),
            phone: None,
            password: Some("old-password".to_string()),
            account_kind: AccountKind::OrganizationAdmin,
            organization_id: Some(Uuid::new_v4()),
        })
        .unwrap();
        let session = auth
            .admin_login(AdminLoginInput {
                email: Some("admin@example.com".to_string()),
                phone: None,
                password: "old-password".to_string(),
            })
            .unwrap();

        let changed = auth
            .change_admin_password(&session.access_token, "old-password", "new-password")
            .unwrap();
        assert_eq!(changed.user.id, user_id);
        assert!(auth
            .authenticate_admin_access_token(&session.access_token)
            .is_err());
        assert_eq!(
            auth.admin_login(AdminLoginInput {
                email: Some("admin@example.com".to_string()),
                phone: None,
                password: "old-password".to_string(),
            })
            .unwrap_err(),
            AuthError::InvalidCredentials
        );
        assert_eq!(
            auth.admin_login(AdminLoginInput {
                email: Some("admin@example.com".to_string()),
                phone: None,
                password: "new-password".to_string(),
            })
            .unwrap()
            .user
            .id,
            user_id
        );
    }

    #[test]
    fn platform_reset_only_accepts_organization_admin_accounts() {
        let mut auth = AuthService::default();
        let organization_id = Uuid::new_v4();
        let admin_id = Uuid::new_v4();
        auth.create_or_update_admin_user(AdminCreateUserInput {
            user_id: admin_id,
            display_name: "Organization Admin".to_string(),
            email: Some("admin@example.com".to_string()),
            phone: None,
            password: Some("old-password".to_string()),
            account_kind: AccountKind::OrganizationAdmin,
            organization_id: Some(organization_id),
        })
        .unwrap();
        let session = auth
            .admin_login(AdminLoginInput {
                email: Some("admin@example.com".to_string()),
                phone: None,
                password: "old-password".to_string(),
            })
            .unwrap();

        auth.reset_organization_admin_password(admin_id, "reset-password")
            .unwrap();
        assert!(auth
            .authenticate_admin_access_token(&session.access_token)
            .is_err());
        assert_eq!(
            auth.admin_login(AdminLoginInput {
                email: Some("admin@example.com".to_string()),
                phone: None,
                password: "reset-password".to_string(),
            })
            .unwrap()
            .user
            .id,
            admin_id
        );

        let business_id = Uuid::new_v4();
        auth.create_or_update_admin_user(AdminCreateUserInput {
            user_id: business_id,
            display_name: "Business User".to_string(),
            email: Some("business@example.com".to_string()),
            phone: None,
            password: Some("business-password".to_string()),
            account_kind: AccountKind::Business,
            organization_id: None,
        })
        .unwrap();
        assert_eq!(
            auth.reset_organization_admin_password(business_id, "reset-password")
                .unwrap_err(),
            AuthError::AdminAccessDenied
        );
    }

    #[test]
    fn rejects_bad_password() {
        let mut auth = AuthService::default();
        auth.register(RegisterInput {
            user_id: None,
            display_name: "Alice".to_string(),
            email: Some("alice@example.com".to_string()),
            phone: None,
            password: "correct-password".to_string(),
            installation_id: "phone-1".to_string(),
        })
        .unwrap();

        assert_eq!(
            auth.login(LoginInput {
                email: Some("alice@example.com".to_string()),
                phone: None,
                password: "wrong-password".to_string(),
                installation_id: "phone-1".to_string(),
            })
            .unwrap_err(),
            AuthError::InvalidCredentials
        );
    }

    #[test]
    fn rejects_weak_new_passwords() {
        let mut auth = AuthService::default();

        assert_eq!(
            auth.register(RegisterInput {
                user_id: None,
                display_name: "Alice".to_string(),
                email: Some("alice@example.com".to_string()),
                phone: None,
                password: "too-short".to_string(),
                installation_id: "phone-1".to_string(),
            })
            .unwrap_err(),
            AuthError::PasswordPolicyViolation
        );

        let user_id = Uuid::new_v4();
        auth.create_or_update_admin_user(AdminCreateUserInput {
            user_id,
            display_name: "Alice".to_string(),
            email: Some("alice@example.com".to_string()),
            phone: None,
            password: Some("correct-password".to_string()),
            account_kind: AccountKind::Business,
            organization_id: None,
        })
        .unwrap();
        assert_eq!(
            auth.create_or_update_admin_user(AdminCreateUserInput {
                user_id,
                display_name: "Alice".to_string(),
                email: Some("alice@example.com".to_string()),
                phone: None,
                password: Some(String::new()),
                account_kind: AccountKind::Business,
                organization_id: None,
            })
            .unwrap_err(),
            AuthError::PasswordRequired
        );
    }

    #[test]
    fn installation_cannot_login_as_another_user() {
        let mut auth = AuthService::default();
        auth.register(RegisterInput {
            user_id: None,
            display_name: "Alice".to_string(),
            email: Some("alice@example.com".to_string()),
            phone: None,
            password: "correct-password".to_string(),
            installation_id: "phone-1".to_string(),
        })
        .unwrap();
        auth.register(RegisterInput {
            user_id: None,
            display_name: "Bob".to_string(),
            email: Some("bob@example.com".to_string()),
            phone: None,
            password: "correct-password".to_string(),
            installation_id: "phone-2".to_string(),
        })
        .unwrap();

        assert_eq!(
            auth.login(LoginInput {
                email: Some("bob@example.com".to_string()),
                phone: None,
                password: "correct-password".to_string(),
                installation_id: "phone-1".to_string(),
            })
            .unwrap_err(),
            AuthError::InstallationAlreadyBound
        );
    }

    #[test]
    fn logout_releases_installation_for_another_user() {
        let mut auth = AuthService::default();
        let alice = auth
            .register(RegisterInput {
                user_id: None,
                display_name: "Alice".to_string(),
                email: Some("alice@example.com".to_string()),
                phone: None,
                password: "correct-password".to_string(),
                installation_id: "phone-1".to_string(),
            })
            .unwrap();
        let bob = auth
            .register(RegisterInput {
                user_id: None,
                display_name: "Bob".to_string(),
                email: Some("bob@example.com".to_string()),
                phone: None,
                password: "correct-password".to_string(),
                installation_id: "phone-2".to_string(),
            })
            .unwrap();

        auth.logout(&alice.access_token).unwrap();

        assert!(auth.authenticate_access_token(&alice.access_token).is_err());
        assert_eq!(
            auth.refresh(RefreshInput {
                refresh_token: alice.refresh_token,
                installation_id: "phone-1".to_string(),
            })
            .unwrap_err(),
            AuthError::RefreshReplayDetected
        );
        let switched = auth
            .login(LoginInput {
                email: Some("bob@example.com".to_string()),
                phone: None,
                password: "correct-password".to_string(),
                installation_id: "phone-1".to_string(),
            })
            .unwrap();
        assert_eq!(switched.user.id, bob.user.id);
    }

    #[test]
    fn refresh_rotates_tokens_and_authenticates_access_token() {
        let mut auth = AuthService::default();
        let first = auth
            .register(RegisterInput {
                user_id: None,
                display_name: "Alice".to_string(),
                email: Some("alice@example.com".to_string()),
                phone: None,
                password: "correct-password".to_string(),
                installation_id: "phone-1".to_string(),
            })
            .unwrap();
        let refreshed = auth
            .refresh(RefreshInput {
                refresh_token: first.refresh_token.clone(),
                installation_id: "phone-1".to_string(),
            })
            .unwrap();

        assert_ne!(first.access_token, refreshed.access_token);
        assert!(auth.authenticate_access_token(&first.access_token).is_err());
        assert_eq!(
            auth.authenticate_access_token(&refreshed.access_token)
                .unwrap()
                .user
                .id,
            refreshed.user.id
        );
        assert_eq!(
            auth.refresh(RefreshInput {
                refresh_token: first.refresh_token,
                installation_id: "phone-1".to_string(),
            })
            .unwrap_err(),
            AuthError::RefreshReplayDetected
        );
        assert!(auth
            .authenticate_access_token(&refreshed.access_token)
            .is_err());
    }

    #[test]
    fn raw_tokens_are_32_random_bytes_and_never_enter_snapshots() {
        let mut auth = AuthService::default();
        let session = auth
            .register(RegisterInput {
                user_id: None,
                display_name: "Alice".to_string(),
                email: Some("alice@example.com".to_string()),
                phone: None,
                password: "correct-password".to_string(),
                installation_id: "phone-1".to_string(),
            })
            .unwrap();
        assert_eq!(
            URL_SAFE_NO_PAD.decode(&session.access_token).unwrap().len(),
            32
        );
        assert_eq!(
            URL_SAFE_NO_PAD
                .decode(&session.refresh_token)
                .unwrap()
                .len(),
            32
        );
        let snapshot = serde_json::to_string(&auth.snapshot().sessions).unwrap();
        assert!(!snapshot.contains(&session.access_token));
        assert!(!snapshot.contains(&session.refresh_token));
    }

    #[test]
    fn expired_access_token_can_refresh_but_expired_refresh_token_cannot() {
        let mut auth = AuthService::default();
        let first = auth
            .register(RegisterInput {
                user_id: None,
                display_name: "Alice".to_string(),
                email: Some("alice@example.com".to_string()),
                phone: None,
                password: "correct-password".to_string(),
                installation_id: "phone-1".to_string(),
            })
            .unwrap();
        auth.sessions_by_access_token
            .get_mut(&token_digest(&first.access_token))
            .expect("stored app session")
            .access_expires_at = OffsetDateTime::now_utc() - Duration::seconds(1);
        assert!(auth.authenticate_access_token(&first.access_token).is_err());

        let refreshed = auth
            .refresh(RefreshInput {
                refresh_token: first.refresh_token,
                installation_id: "phone-1".to_string(),
            })
            .expect("refresh within refresh-token lifetime");
        auth.sessions_by_access_token
            .get_mut(&token_digest(&refreshed.access_token))
            .expect("stored refreshed session")
            .refresh_expires_at = Some(OffsetDateTime::now_utc() - Duration::seconds(1));
        assert_eq!(
            auth.refresh(RefreshInput {
                refresh_token: refreshed.refresh_token,
                installation_id: "phone-1".to_string(),
            })
            .unwrap_err(),
            AuthError::SessionNotFound
        );
    }
}
