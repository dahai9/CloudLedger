use std::net::{IpAddr, SocketAddr};

use axum::{
    extract::{ConnectInfo, State},
    http::{
        header::{AUTHORIZATION, RETRY_AFTER},
        HeaderMap, HeaderValue, StatusCode,
    },
    response::{IntoResponse, Response},
    Json,
};
use cloudledger_service::{AppEnsureUserIdentityInput, AppServiceError};
use serde::{Deserialize, Serialize};

use crate::{
    auth::{
        AuthError, AuthService, AuthenticatedSession, LoginInput, RefreshInput, Session,
        UpdateProfileInput,
    },
    login_protection::LoginSurface,
    ServerState,
};

pub async fn login(
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    State(state): State<ServerState>,
    Json(request): Json<LoginRequest>,
) -> Result<Json<Session>, ApiError> {
    let identifier = login_identifier(request.email.as_deref(), request.phone.as_deref());
    check_login_attempt(&state, peer_addr.ip(), LoginSurface::Business, &identifier)?;
    let _write_guard = state.write_gate.lock().await;

    let (login_result, staged_auth) = {
        let auth = state
            .auth_service
            .lock()
            .map_err(|_| ApiError::internal("auth service lock poisoned"))?;
        let mut staged_auth = auth.clone();
        let result = staged_auth.login(LoginInput {
            email: request.email,
            phone: request.phone,
            password: request.password,
            installation_id: request.installation_id,
        });
        (result, staged_auth)
    };
    let session = match login_result {
        Ok(session) => {
            record_login_success(&state, peer_addr.ip(), LoginSurface::Business, &identifier)?;
            session
        }
        Err(AuthError::InvalidCredentials) => {
            if let Some(error) =
                record_login_failure(&state, peer_addr.ip(), LoginSurface::Business, &identifier)?
            {
                return Err(ApiError::from_auth(error));
            }
            return Err(ApiError::from_auth(AuthError::InvalidCredentials));
        }
        Err(error) => {
            record_login_success(&state, peer_addr.ip(), LoginSurface::Business, &identifier)?;
            return Err(ApiError::from_auth(error));
        }
    };

    persist_auth_and_ledger_identity(
        &state,
        staged_auth,
        &AuthenticatedSession {
            user: session.user.clone(),
            installation_id: session.installation_id.clone(),
        },
    )
    .await?;
    Ok(Json(session))
}

pub async fn refresh(
    State(state): State<ServerState>,
    Json(request): Json<RefreshRequest>,
) -> Result<Json<Session>, ApiError> {
    let _write_guard = state.write_gate.lock().await;
    let (session, staged_auth) = {
        let auth = state
            .auth_service
            .lock()
            .map_err(|_| ApiError::internal("auth service lock poisoned"))?;
        let mut staged_auth = auth.clone();
        let session = staged_auth
            .refresh(RefreshInput {
                refresh_token: request.refresh_token,
                installation_id: request.installation_id,
            })
            .map_err(ApiError::from_auth)?;
        (session, staged_auth)
    };
    persist_auth_and_ledger_identity(
        &state,
        staged_auth,
        &AuthenticatedSession {
            user: session.user.clone(),
            installation_id: session.installation_id.clone(),
        },
    )
    .await?;
    Ok(Json(session))
}

pub async fn me(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<MeResponse>, ApiError> {
    let session = authenticate(&state, &headers)?;
    Ok(Json(MeResponse {
        user: session.user,
        installation_id: session.installation_id,
    }))
}

pub async fn update_me(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(request): Json<UpdateProfileRequest>,
) -> Result<Json<MeResponse>, ApiError> {
    let token = bearer_token(&headers)?;
    let _write_guard = state.write_gate.lock().await;
    let (session, staged_auth) = {
        let auth = state
            .auth_service
            .lock()
            .map_err(|_| ApiError::internal("auth service lock poisoned"))?;
        let mut staged_auth = auth.clone();
        let session = staged_auth
            .update_profile(
                token,
                UpdateProfileInput {
                    display_name: request.display_name,
                },
            )
            .map_err(ApiError::from_auth)?;
        (session, staged_auth)
    };
    persist_auth_and_ledger_identity(&state, staged_auth, &session).await?;
    Ok(Json(MeResponse {
        user: session.user,
        installation_id: session.installation_id,
    }))
}

pub async fn logout(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let token = bearer_token(&headers)?;
    let _write_guard = state.write_gate.lock().await;
    let staged_auth = {
        let auth = state
            .auth_service
            .lock()
            .map_err(|_| ApiError::internal("auth service lock poisoned"))?;
        let mut staged_auth = auth.clone();
        staged_auth.logout(token).map_err(ApiError::from_auth)?;
        staged_auth
    };
    state
        .storage
        .save_auth(staged_auth.snapshot())
        .await
        .map_err(ApiError::from_storage)?;
    *state
        .auth_service
        .lock()
        .map_err(|_| ApiError::internal("auth service lock poisoned"))? = staged_auth;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) fn authenticate(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<crate::auth::AuthenticatedSession, ApiError> {
    let token = bearer_token(headers)?;
    let auth = state
        .auth_service
        .lock()
        .map_err(|_| ApiError::internal("auth service lock poisoned"))?;
    auth.authenticate_access_token(token)
        .map_err(ApiError::from_auth)
}

async fn persist_auth_and_ledger_identity(
    state: &ServerState,
    staged_auth: AuthService,
    session: &AuthenticatedSession,
) -> Result<(), ApiError> {
    let mut staged_ledger = {
        let service = state
            .ledger_service
            .lock()
            .map_err(|_| ApiError::internal("ledger service lock poisoned"))?;
        service.clone()
    };
    staged_ledger
        .ensure_user_identity(AppEnsureUserIdentityInput {
            user_id: session.user.id,
            display_name: session.user.display_name.clone(),
            email: session.user.email.clone(),
            phone: session.user.phone.clone(),
        })
        .map_err(ApiError::from_service)?;
    state
        .storage
        .save_all(staged_ledger.snapshot(), staged_auth.snapshot())
        .await
        .map_err(ApiError::from_storage)?;
    let mut auth = state
        .auth_service
        .lock()
        .map_err(|_| ApiError::internal("auth service lock poisoned"))?;
    let mut ledger = state
        .ledger_service
        .lock()
        .map_err(|_| ApiError::internal("ledger service lock poisoned"))?;
    *auth = staged_auth;
    *ledger = staged_ledger;
    Ok(())
}

fn bearer_token(headers: &HeaderMap) -> Result<&str, ApiError> {
    headers
        .get(AUTHORIZATION)
        .and_then(|header| header.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ApiError::unauthorized("access token required"))
}

fn login_identifier(email: Option<&str>, phone: Option<&str>) -> String {
    email
        .or(phone)
        .map(|value| value.trim().to_lowercase())
        .unwrap_or_default()
}

fn check_login_attempt(
    state: &ServerState,
    ip: IpAddr,
    surface: LoginSurface,
    identifier: &str,
) -> Result<(), ApiError> {
    state
        .login_protection
        .lock()
        .map_err(|_| ApiError::internal("login protection lock poisoned"))?
        .check(ip, surface, identifier)
        .map_err(ApiError::from_auth)
}

fn record_login_failure(
    state: &ServerState,
    ip: IpAddr,
    surface: LoginSurface,
    identifier: &str,
) -> Result<Option<AuthError>, ApiError> {
    Ok(state
        .login_protection
        .lock()
        .map_err(|_| ApiError::internal("login protection lock poisoned"))?
        .record_failure(ip, surface, identifier))
}

fn record_login_success(
    state: &ServerState,
    ip: IpAddr,
    surface: LoginSurface,
    identifier: &str,
) -> Result<(), ApiError> {
    state
        .login_protection
        .lock()
        .map_err(|_| ApiError::internal("login protection lock poisoned"))?
        .record_success(ip, surface, identifier);
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginRequest {
    email: Option<String>,
    phone: Option<String>,
    password: String,
    installation_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshRequest {
    refresh_token: String,
    installation_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProfileRequest {
    display_name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeResponse {
    user: crate::auth::AuthUserDto,
    installation_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorResponse {
    error: String,
}

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
    retry_after_seconds: Option<u64>,
}

impl ApiError {
    pub(crate) fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: message.into(),
            retry_after_seconds: None,
        }
    }

    pub(crate) fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
            retry_after_seconds: None,
        }
    }

    pub(crate) fn from_auth(error: AuthError) -> Self {
        let retry_after_seconds = match &error {
            AuthError::LoginRateLimited {
                retry_after_seconds,
            } => Some(*retry_after_seconds),
            _ => None,
        };
        let status = match &error {
            AuthError::InvalidCredentials | AuthError::SessionNotFound => StatusCode::UNAUTHORIZED,
            AuthError::LoginRateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            AuthError::BusinessAppAccessDenied | AuthError::AdminAccessDenied => {
                StatusCode::FORBIDDEN
            }
            AuthError::EmailAlreadyRegistered
            | AuthError::PhoneAlreadyRegistered
            | AuthError::InstallationAlreadyBound => StatusCode::CONFLICT,
            AuthError::Storage(_)
            | AuthError::PasswordHashFailed
            | AuthError::AdminOrganizationRequired => StatusCode::INTERNAL_SERVER_ERROR,
            AuthError::LoginIdentifierRequired
            | AuthError::DisplayNameRequired
            | AuthError::PasswordRequired
            | AuthError::PasswordPolicyViolation
            | AuthError::InstallationIdRequired => StatusCode::BAD_REQUEST,
        };
        Self {
            status,
            message: error.to_string(),
            retry_after_seconds,
        }
    }

    pub(crate) fn from_service(error: AppServiceError) -> Self {
        let status = match error {
            AppServiceError::UserNotFound
            | AppServiceError::LedgerNotFound
            | AppServiceError::AccountNotFound
            | AppServiceError::CategoryNotFound
            | AppServiceError::TransactionNotFound => StatusCode::NOT_FOUND,
            AppServiceError::Unauthorized => StatusCode::FORBIDDEN,
            AppServiceError::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::BAD_REQUEST,
        };
        Self {
            status,
            message: error.to_string(),
            retry_after_seconds: None,
        }
    }

    pub(crate) fn from_storage(error: anyhow::Error) -> Self {
        Self::internal(format!("storage error: {error}"))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (
            self.status,
            Json(ErrorResponse {
                error: self.message,
            }),
        )
            .into_response();
        if let Some(retry_after_seconds) = self.retry_after_seconds {
            if let Ok(value) = HeaderValue::from_str(&retry_after_seconds.to_string()) {
                response.headers_mut().insert(RETRY_AFTER, value);
            }
        }
        response
    }
}
