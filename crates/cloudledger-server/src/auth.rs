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
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

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
    #[error("installation id is required")]
    InstallationIdRequired,
    #[error("installation is already bound to another account")]
    InstallationAlreadyBound,
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("session was not found")]
    SessionNotFound,
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
}

#[derive(Debug, Clone)]
pub struct LoginInput {
    pub email: Option<String>,
    pub phone: Option<String>,
    pub password: String,
    pub installation_id: String,
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
struct StoredUser {
    id: Uuid,
    display_name: String,
    email: Option<String>,
    phone: Option<String>,
    password_hash: String,
    created_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredSession {
    user_id: Uuid,
    installation_id: String,
    access_token: String,
    refresh_token: String,
    created_at: OffsetDateTime,
    refreshed_at: OffsetDateTime,
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

impl AuthService {
    pub fn load_or_default(path: impl AsRef<Path>) -> Result<Self, AuthError> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::default());
        }

        let document = fs::read_to_string(path)
            .map_err(|err| AuthError::Storage(format!("read {}: {err}", path.display())))?;
        serde_json::from_str(&document)
            .map_err(|err| AuthError::Storage(format!("parse {}: {err}", path.display())))
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
        let password = normalize_password(&input.password)?;
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

        let user = StoredUser {
            id: input.user_id.unwrap_or_else(Uuid::new_v4),
            display_name,
            email,
            phone,
            password_hash: hash_password(&password)?,
            created_at: OffsetDateTime::now_utc(),
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
        self.issue_session(user_id, installation_id)
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
            .map(str::trim)
            .filter(|password| !password.is_empty())
            .map(str::to_string);
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
            created_at,
        };
        if let Some(email) = &user.email {
            self.user_ids_by_email.insert(email.clone(), user.id);
        }
        if let Some(phone) = &user.phone {
            self.user_ids_by_phone.insert(phone.clone(), user.id);
        }
        let user_id = user.id;
        self.users_by_id.insert(user_id, user);
        self.users_by_id
            .get(&user_id)
            .map(auth_user_dto)
            .ok_or(AuthError::InvalidCredentials)
    }

    pub fn login(&mut self, input: LoginInput) -> Result<Session, AuthError> {
        let email = input.email.and_then(normalize_optional_email);
        let phone = input.phone.and_then(normalize_optional_phone);
        require_login_identifier(&email, &phone)?;
        let installation_id = normalize_installation_id(&input.installation_id)?;
        let user_id = self
            .find_user_id(email.as_deref(), phone.as_deref())
            .ok_or(AuthError::InvalidCredentials)?;
        let user = self
            .users_by_id
            .get(&user_id)
            .ok_or(AuthError::InvalidCredentials)?;
        verify_password(&input.password, &user.password_hash)?;
        self.bind_installation(&installation_id, user_id)?;
        self.issue_session(user_id, installation_id)
    }

    pub fn refresh(&mut self, input: RefreshInput) -> Result<Session, AuthError> {
        let installation_id = normalize_installation_id(&input.installation_id)?;
        let access_token = self
            .access_tokens_by_refresh_token
            .remove(input.refresh_token.trim())
            .ok_or(AuthError::SessionNotFound)?;
        let session = self
            .sessions_by_access_token
            .remove(&access_token)
            .ok_or(AuthError::SessionNotFound)?;
        if session.installation_id != installation_id {
            return Err(AuthError::InstallationAlreadyBound);
        }
        self.issue_session(session.user_id, installation_id)
    }

    pub fn logout(&mut self, access_token: &str) -> Result<(), AuthError> {
        let access_token = normalize_bearer_token(access_token);
        let session = self
            .sessions_by_access_token
            .remove(access_token)
            .ok_or(AuthError::SessionNotFound)?;
        self.access_tokens_by_refresh_token
            .remove(&session.refresh_token);
        Ok(())
    }

    pub fn authenticate_access_token(
        &self,
        access_token: &str,
    ) -> Result<AuthenticatedSession, AuthError> {
        let access_token = normalize_bearer_token(access_token);
        let session = self
            .sessions_by_access_token
            .get(access_token)
            .ok_or(AuthError::InvalidCredentials)?;
        let user = self
            .users_by_id
            .get(&session.user_id)
            .ok_or(AuthError::InvalidCredentials)?;
        Ok(AuthenticatedSession {
            user: auth_user_dto(user),
            installation_id: session.installation_id.clone(),
        })
    }

    pub fn update_profile(
        &mut self,
        access_token: &str,
        input: UpdateProfileInput,
    ) -> Result<AuthenticatedSession, AuthError> {
        let access_token = normalize_bearer_token(access_token);
        let session = self
            .sessions_by_access_token
            .get(access_token)
            .ok_or(AuthError::InvalidCredentials)?
            .clone();
        let display_name = normalize_display_name(&input.display_name)?;
        let user = self
            .users_by_id
            .get_mut(&session.user_id)
            .ok_or(AuthError::InvalidCredentials)?;
        user.display_name = display_name;
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
    ) -> Result<Session, AuthError> {
        let user = self
            .users_by_id
            .get(&user_id)
            .ok_or(AuthError::InvalidCredentials)?;
        let now = OffsetDateTime::now_utc();
        let access_token = format!("access_{}", Uuid::new_v4());
        let refresh_token = format!("refresh_{}", Uuid::new_v4());
        let stored = StoredSession {
            user_id,
            installation_id: installation_id.clone(),
            access_token: access_token.clone(),
            refresh_token: refresh_token.clone(),
            created_at: now,
            refreshed_at: now,
        };
        self.access_tokens_by_refresh_token
            .insert(refresh_token.clone(), access_token.clone());
        self.sessions_by_access_token
            .insert(access_token.clone(), stored);
        Ok(Session {
            user: auth_user_dto(user),
            access_token,
            refresh_token,
            installation_id,
        })
    }
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

fn auth_user_dto(user: &StoredUser) -> AuthUserDto {
    AuthUserDto {
        id: user.id,
        display_name: user.display_name.clone(),
        email: user.email.clone(),
        phone: user.phone.clone(),
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

fn normalize_password(value: &str) -> Result<String, AuthError> {
    let value = value.trim();
    if value.is_empty() {
        Err(AuthError::PasswordRequired)
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
    fn rejects_bad_password() {
        let mut auth = AuthService::default();
        auth.register(RegisterInput {
            user_id: None,
            display_name: "Alice".to_string(),
            email: Some("alice@example.com".to_string()),
            phone: None,
            password: "correct".to_string(),
            installation_id: "phone-1".to_string(),
        })
        .unwrap();

        assert_eq!(
            auth.login(LoginInput {
                email: Some("alice@example.com".to_string()),
                phone: None,
                password: "wrong".to_string(),
                installation_id: "phone-1".to_string(),
            })
            .unwrap_err(),
            AuthError::InvalidCredentials
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
            password: "correct".to_string(),
            installation_id: "phone-1".to_string(),
        })
        .unwrap();
        auth.register(RegisterInput {
            user_id: None,
            display_name: "Bob".to_string(),
            email: Some("bob@example.com".to_string()),
            phone: None,
            password: "correct".to_string(),
            installation_id: "phone-2".to_string(),
        })
        .unwrap();

        assert_eq!(
            auth.login(LoginInput {
                email: Some("bob@example.com".to_string()),
                phone: None,
                password: "correct".to_string(),
                installation_id: "phone-1".to_string(),
            })
            .unwrap_err(),
            AuthError::InstallationAlreadyBound
        );
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
                password: "correct".to_string(),
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
    }
}
