use std::net::IpAddr;

use axum::{
    extract::{Extension, State},
    http::{
        header::{AUTHORIZATION, COOKIE, RETRY_AFTER, SET_COOKIE},
        HeaderMap, HeaderValue, StatusCode,
    },
    response::{IntoResponse, Response},
    Json,
};
use cloudledger_service::{AppEnsureUserIdentityInput, AppServiceError};
use serde::{Deserialize, Serialize};

use crate::{
    audit::SecurityAuditEvent,
    auth::{
        AuthError, AuthService, AuthenticatedSession, LoginInput, RefreshInput, Session,
        SessionKind, UpdateProfileInput,
    },
    login_protection::{LoginSurface, SecurityRateKind},
    request_security::RequestContext,
    turnstile::TurnstileError,
    ServerState,
};

const WEB_REFRESH_COOKIE: &str = "__Host-cloudledger_refresh";

pub async fn auth_security(State(state): State<ServerState>) -> Json<AuthSecurityResponse> {
    Json(AuthSecurityResponse {
        turnstile_enabled: state.turnstile.is_enabled(),
        turnstile_site_key: state.turnstile.site_key().map(str::to_string),
    })
}

pub async fn tauri_login(
    Extension(context): Extension<RequestContext>,
    State(state): State<ServerState>,
    Json(request): Json<LoginRequest>,
) -> Result<Json<Session>, ApiError> {
    login_for_client(&state, context.client_ip, request, SessionKind::Tauri)
        .await
        .map(Json)
}

async fn login_for_client(
    state: &ServerState,
    client_ip: IpAddr,
    request: LoginRequest,
    client_kind: SessionKind,
) -> Result<Session, ApiError> {
    let identifier = login_identifier(request.email.as_deref(), request.phone.as_deref());
    check_login_attempt(state, client_ip, LoginSurface::Business, &identifier).await?;
    if state
        .login_protection
        .challenge_required(client_ip, LoginSurface::Business, &identifier)
        .await
        .map_err(ApiError::from_storage)?
    {
        if request.turnstile_token.trim().is_empty() {
            return Err(ApiError::from_auth(AuthError::TurnstileRequired));
        }
        state
            .turnstile
            .verify_for_action(&request.turnstile_token, client_ip, "business-login")
            .await
            .map_err(ApiError::from_turnstile)?;
    }
    let _write_guard = state.write_gate.lock().await;

    let (login_result, staged_auth) = {
        let auth = state
            .auth_service
            .lock()
            .map_err(|_| ApiError::internal("auth service lock poisoned"))?;
        let mut staged_auth = auth.clone();
        let result = staged_auth.login_for_client(
            LoginInput {
                email: request.email,
                phone: request.phone,
                password: request.password,
                installation_id: request.installation_id,
            },
            client_kind,
        );
        (result, staged_auth)
    };
    let session = match login_result {
        Ok(session) => {
            record_login_success(state, client_ip, LoginSurface::Business, &identifier).await?;
            session
        }
        Err(AuthError::InvalidCredentials) => {
            if let Some(error) =
                record_login_failure(state, client_ip, LoginSurface::Business, &identifier).await?
            {
                return Err(ApiError::from_auth(error));
            }
            return Err(ApiError::from_auth(AuthError::InvalidCredentials));
        }
        Err(error) => {
            record_login_success(state, client_ip, LoginSurface::Business, &identifier).await?;
            return Err(ApiError::from_auth(error));
        }
    };

    persist_auth_and_ledger_identity(
        state,
        staged_auth,
        &AuthenticatedSession {
            user: session.user.clone(),
            installation_id: session.installation_id.clone(),
        },
    )
    .await?;
    append_session_audit(state, &session, "session_issued", client_kind).await?;
    Ok(session)
}

pub async fn tauri_refresh(
    Extension(context): Extension<RequestContext>,
    State(state): State<ServerState>,
    Json(request): Json<RefreshRequest>,
) -> Result<Json<Session>, ApiError> {
    refresh_for_client(&state, context.client_ip, request, SessionKind::Tauri)
        .await
        .map(Json)
}

async fn refresh_for_client(
    state: &ServerState,
    client_ip: IpAddr,
    request: RefreshRequest,
    client_kind: SessionKind,
) -> Result<Session, ApiError> {
    state
        .login_protection
        .check_security_request(SecurityRateKind::Refresh, client_ip)
        .await
        .map_err(ApiError::from_storage)?
        .map_err(ApiError::from_auth)?;
    let _write_guard = state.write_gate.lock().await;
    let (session, staged_auth) = {
        let auth = state
            .auth_service
            .lock()
            .map_err(|_| ApiError::internal("auth service lock poisoned"))?;
        let mut staged_auth = auth.clone();
        let session = staged_auth
            .refresh_for_client(
                RefreshInput {
                    refresh_token: request.refresh_token,
                    installation_id: request.installation_id,
                },
                client_kind,
            )
            .map_err(ApiError::from_auth)?;
        (session, staged_auth)
    };
    persist_auth_and_ledger_identity(
        state,
        staged_auth,
        &AuthenticatedSession {
            user: session.user.clone(),
            installation_id: session.installation_id.clone(),
        },
    )
    .await?;
    append_session_audit(state, &session, "session_rotated", client_kind).await?;
    Ok(session)
}

pub async fn web_login(
    Extension(context): Extension<RequestContext>,
    State(state): State<ServerState>,
    Json(request): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    require_web_login(&state, context)?;
    let session = login_for_client(&state, context.client_ip, request, SessionKind::Web).await?;
    Ok(web_session_response(session, false))
}

pub async fn web_refresh(
    Extension(context): Extension<RequestContext>,
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(request): Json<WebRefreshRequest>,
) -> Result<Response, ApiError> {
    require_web_login(&state, context)?;
    let refresh_token = cookie_value(&headers, WEB_REFRESH_COOKIE)
        .ok_or_else(|| ApiError::unauthorized("refresh cookie required"))?;
    let session = refresh_for_client(
        &state,
        context.client_ip,
        RefreshRequest {
            refresh_token,
            installation_id: request.installation_id,
        },
        SessionKind::Web,
    )
    .await?;
    Ok(web_session_response(session, false))
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

pub async fn tauri_logout(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let token = bearer_token(&headers)?;
    let authenticated = authenticate(&state, &headers)?;
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
    state
        .storage
        .append_security_event(SecurityAuditEvent {
            scope_key: business_audit_scope(authenticated.user.organization_id),
            actor_type: "business_user".to_string(),
            actor_id: Some(authenticated.user.id),
            action: "session_revoked".to_string(),
            resource_type: "session".to_string(),
            resource_id: None,
            metadata: serde_json::json!({"client_kind": "tauri"}),
        })
        .await
        .map_err(ApiError::from_storage)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn append_session_audit(
    state: &ServerState,
    session: &Session,
    action: &str,
    client_kind: SessionKind,
) -> Result<(), ApiError> {
    let client_kind = match client_kind {
        SessionKind::Tauri => "tauri",
        SessionKind::Web => "web",
        SessionKind::Admin => "admin",
    };
    state
        .storage
        .append_security_event(SecurityAuditEvent {
            scope_key: business_audit_scope(session.user.organization_id),
            actor_type: "business_user".to_string(),
            actor_id: Some(session.user.id),
            action: action.to_string(),
            resource_type: "session".to_string(),
            resource_id: None,
            metadata: serde_json::json!({"client_kind": client_kind}),
        })
        .await
        .map_err(ApiError::from_storage)
}

pub async fn web_logout(
    Extension(context): Extension<RequestContext>,
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    require_web_login(&state, context)?;
    if let Some(refresh_token) = cookie_value(&headers, WEB_REFRESH_COOKIE) {
        let _write_guard = state.write_gate.lock().await;
        let (user_id, organization_id, staged_auth) = {
            let auth = state
                .auth_service
                .lock()
                .map_err(|_| ApiError::internal("auth service lock poisoned"))?;
            let mut staged_auth = auth.clone();
            let user_id = staged_auth
                .logout_refresh(&refresh_token)
                .map_err(ApiError::from_auth)?;
            let organization_id = staged_auth.organization_id(user_id);
            (user_id, organization_id, staged_auth)
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
        state
            .storage
            .append_security_event(SecurityAuditEvent {
                scope_key: business_audit_scope(organization_id),
                actor_type: "business_user".to_string(),
                actor_id: Some(user_id),
                action: "session_revoked".to_string(),
                resource_type: "session".to_string(),
                resource_id: None,
                metadata: serde_json::json!({"client_kind": "web"}),
            })
            .await
            .map_err(ApiError::from_storage)?;
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_static(
            "__Host-cloudledger_refresh=; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Strict",
        ),
    );
    Ok(response)
}

fn business_audit_scope(organization_id: Option<uuid::Uuid>) -> String {
    organization_id
        .map(|organization_id| format!("organization:{organization_id}"))
        .unwrap_or_else(|| "platform".to_string())
}

pub async fn legacy_upgrade() -> Response {
    (
        StatusCode::UPGRADE_REQUIRED,
        Json(ErrorResponse {
            error: "client_upgrade_required".to_string(),
        }),
    )
        .into_response()
}

fn require_web_login(state: &ServerState, context: RequestContext) -> Result<(), ApiError> {
    if !state.web_login_enabled {
        return Err(ApiError::not_found("web login is disabled"));
    }
    if !context.forwarded_https {
        return Err(ApiError::bad_request("web login requires HTTPS"));
    }
    Ok(())
}

fn web_session_response(session: Session, _clear_old_cookie: bool) -> Response {
    let cookie = format!(
        "{WEB_REFRESH_COOKIE}={}; Path=/; Max-Age=2592000; Secure; HttpOnly; SameSite=Strict",
        session.refresh_token
    );
    let body = WebSessionResponse {
        user: session.user,
        access_token: session.access_token,
        installation_id: session.installation_id,
    };
    let mut response = Json(body).into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_str(&cookie).expect("valid refresh cookie"),
    );
    response
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(COOKIE)
        .and_then(|header| header.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';').find_map(|cookie| {
                let (key, value) = cookie.trim().split_once('=')?;
                (key == name).then(|| value.to_string())
            })
        })
        .filter(|value| !value.trim().is_empty())
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

async fn check_login_attempt(
    state: &ServerState,
    ip: IpAddr,
    surface: LoginSurface,
    identifier: &str,
) -> Result<(), ApiError> {
    state
        .login_protection
        .check(ip, surface, identifier)
        .await
        .map_err(ApiError::from_storage)?
        .map_err(ApiError::from_auth)
}

async fn record_login_failure(
    state: &ServerState,
    ip: IpAddr,
    surface: LoginSurface,
    identifier: &str,
) -> Result<Option<AuthError>, ApiError> {
    let rate_limit = state
        .login_protection
        .record_failure(ip, surface, identifier)
        .await
        .map_err(ApiError::from_storage)?;
    if rate_limit.is_some() {
        return Ok(rate_limit);
    }
    if state
        .login_protection
        .challenge_required(ip, surface, identifier)
        .await
        .map_err(ApiError::from_storage)?
    {
        return Ok(Some(AuthError::TurnstileRequired));
    }
    Ok(None)
}

async fn record_login_success(
    state: &ServerState,
    ip: IpAddr,
    surface: LoginSurface,
    identifier: &str,
) -> Result<(), ApiError> {
    state
        .login_protection
        .record_success(ip, surface, identifier)
        .await
        .map_err(ApiError::from_storage)?;
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginRequest {
    email: Option<String>,
    phone: Option<String>,
    password: String,
    installation_id: String,
    #[serde(default)]
    turnstile_token: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshRequest {
    refresh_token: String,
    installation_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebRefreshRequest {
    installation_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthSecurityResponse {
    turnstile_enabled: bool,
    turnstile_site_key: Option<String>,
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
struct WebSessionResponse {
    user: crate::auth::AuthUserDto,
    access_token: String,
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

    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
            retry_after_seconds: None,
        }
    }

    pub(crate) fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
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
            AuthError::InvalidCredentials
            | AuthError::SessionNotFound
            | AuthError::RefreshReplayDetected => StatusCode::UNAUTHORIZED,
            AuthError::LoginRateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            AuthError::TurnstileRequired => StatusCode::PRECONDITION_REQUIRED,
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

    fn from_turnstile(error: TurnstileError) -> Self {
        match error {
            TurnstileError::TokenRequired => Self::from_auth(AuthError::TurnstileRequired),
            TurnstileError::Rejected => Self::unauthorized("turnstile verification rejected"),
            TurnstileError::Unavailable => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                message: "turnstile verification unavailable".to_string(),
                retry_after_seconds: None,
            },
        }
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
