use axum::{
    extract::State,
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use cloudledger_service::{AppEnsureUserIdentityInput, AppServiceError};
use serde::{Deserialize, Serialize};

use crate::{
    auth::{
        AuthError, AuthenticatedSession, LoginInput, RefreshInput, Session, UpdateProfileInput,
    },
    ServerState,
};

pub async fn login(
    State(state): State<ServerState>,
    Json(request): Json<LoginRequest>,
) -> Result<Json<Session>, ApiError> {
    let session = {
        let mut auth = state
            .auth_service
            .lock()
            .map_err(|_| ApiError::internal("auth service lock poisoned"))?;
        let session = auth
            .login(LoginInput {
                email: request.email,
                phone: request.phone,
                password: request.password,
                installation_id: request.installation_id,
            })
            .map_err(ApiError::from_auth)?;
        auth.save_to_path(&state.auth_state_path)
            .map_err(ApiError::from_auth)?;
        session
    };

    ensure_ledger_identity(&state, &session)?;
    Ok(Json(session))
}

pub async fn refresh(
    State(state): State<ServerState>,
    Json(request): Json<RefreshRequest>,
) -> Result<Json<Session>, ApiError> {
    let session = {
        let mut auth = state
            .auth_service
            .lock()
            .map_err(|_| ApiError::internal("auth service lock poisoned"))?;
        let session = auth
            .refresh(RefreshInput {
                refresh_token: request.refresh_token,
                installation_id: request.installation_id,
            })
            .map_err(ApiError::from_auth)?;
        auth.save_to_path(&state.auth_state_path)
            .map_err(ApiError::from_auth)?;
        session
    };

    ensure_ledger_identity(&state, &session)?;
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
    let session = {
        let mut auth = state
            .auth_service
            .lock()
            .map_err(|_| ApiError::internal("auth service lock poisoned"))?;
        let session = auth
            .update_profile(
                token,
                UpdateProfileInput {
                    display_name: request.display_name,
                },
            )
            .map_err(ApiError::from_auth)?;
        auth.save_to_path(&state.auth_state_path)
            .map_err(ApiError::from_auth)?;
        session
    };

    ensure_ledger_identity_from_auth(&state, &session)?;
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
    let mut auth = state
        .auth_service
        .lock()
        .map_err(|_| ApiError::internal("auth service lock poisoned"))?;
    auth.logout(token).map_err(ApiError::from_auth)?;
    auth.save_to_path(&state.auth_state_path)
        .map_err(ApiError::from_auth)?;
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

fn ensure_ledger_identity(state: &ServerState, session: &Session) -> Result<(), ApiError> {
    ensure_ledger_identity_from_auth(
        state,
        &AuthenticatedSession {
            user: session.user.clone(),
            installation_id: session.installation_id.clone(),
        },
    )
}

fn ensure_ledger_identity_from_auth(
    state: &ServerState,
    session: &AuthenticatedSession,
) -> Result<(), ApiError> {
    let mut service = state
        .ledger_service
        .lock()
        .map_err(|_| ApiError::internal("ledger service lock poisoned"))?;
    service
        .ensure_user_identity(AppEnsureUserIdentityInput {
            user_id: session.user.id,
            display_name: session.user.display_name.clone(),
            email: session.user.email.clone(),
            phone: session.user.phone.clone(),
        })
        .map_err(ApiError::from_service)?;
    service
        .save_to_path(&state.ledger_state_path)
        .map_err(ApiError::from_service)
}

fn bearer_token(headers: &HeaderMap) -> Result<&str, ApiError> {
    headers
        .get(AUTHORIZATION)
        .and_then(|header| header.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ApiError::unauthorized("access token required"))
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
}

impl ApiError {
    pub(crate) fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: message.into(),
        }
    }

    pub(crate) fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
        }
    }

    pub(crate) fn from_auth(error: AuthError) -> Self {
        let status = match error {
            AuthError::InvalidCredentials | AuthError::SessionNotFound => StatusCode::UNAUTHORIZED,
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
            | AuthError::InstallationIdRequired => StatusCode::BAD_REQUEST,
        };
        Self {
            status,
            message: error.to_string(),
        }
    }

    pub(crate) fn from_service(error: AppServiceError) -> Self {
        let status = match error {
            AppServiceError::UserNotFound
            | AppServiceError::LedgerNotFound
            | AppServiceError::AccountNotFound
            | AppServiceError::TransactionNotFound => StatusCode::NOT_FOUND,
            AppServiceError::Unauthorized => StatusCode::FORBIDDEN,
            AppServiceError::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::BAD_REQUEST,
        };
        Self {
            status,
            message: error.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorResponse {
                error: self.message,
            }),
        )
            .into_response()
    }
}
