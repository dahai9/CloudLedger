use std::collections::BTreeMap;

use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AuthError {
    #[error("email is already registered")]
    EmailAlreadyRegistered,
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("password hashing failed")]
    PasswordHashFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub user_id: Uuid,
    pub access_token: String,
    pub refresh_token: String,
}

#[derive(Debug, Clone)]
struct StoredUser {
    id: Uuid,
    password_hash: String,
}

#[derive(Debug, Default)]
pub struct AuthService {
    users_by_email: BTreeMap<String, StoredUser>,
}

impl AuthService {
    pub fn register(&mut self, email: &str, password: &str) -> Result<Session, AuthError> {
        let normalized = normalize_email(email);
        if self.users_by_email.contains_key(&normalized) {
            return Err(AuthError::EmailAlreadyRegistered);
        }

        let user = StoredUser {
            id: Uuid::new_v4(),
            password_hash: hash_password(password)?,
        };
        let session = new_session(user.id);
        self.users_by_email.insert(normalized, user);
        Ok(session)
    }

    pub fn login(&self, email: &str, password: &str) -> Result<Session, AuthError> {
        let normalized = normalize_email(email);
        let user = self
            .users_by_email
            .get(&normalized)
            .ok_or(AuthError::InvalidCredentials)?;

        verify_password(password, &user.password_hash)?;
        Ok(new_session(user.id))
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

fn new_session(user_id: Uuid) -> Session {
    Session {
        user_id,
        access_token: format!("access_{}", Uuid::new_v4()),
        refresh_token: format!("refresh_{}", Uuid::new_v4()),
    }
}

fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_login_rotates_tokens() {
        let mut auth = AuthService::default();
        let first = auth
            .register("Alice@Example.com", "correct horse battery staple")
            .unwrap();
        let second = auth
            .login("alice@example.com", "correct horse battery staple")
            .unwrap();

        assert_eq!(first.user_id, second.user_id);
        assert_ne!(first.access_token, second.access_token);
        assert_ne!(first.refresh_token, second.refresh_token);
    }

    #[test]
    fn rejects_bad_password() {
        let mut auth = AuthService::default();
        auth.register("alice@example.com", "correct").unwrap();

        assert_eq!(
            auth.login("alice@example.com", "wrong").unwrap_err(),
            AuthError::InvalidCredentials
        );
    }
}
