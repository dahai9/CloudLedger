use std::net::{IpAddr, SocketAddr};

use axum::{
    extract::{ConnectInfo, Path, Request, State},
    http::{
        header::{AUTHORIZATION, RETRY_AFTER},
        HeaderMap, HeaderValue, StatusCode,
    },
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, patch, post},
    Json, Router,
};
use cloudledger_core::MembershipRole;
use cloudledger_service::{
    AppAddOrganizationMemberInput, AppCreateOrganizationInput, AppLedgerService, AppServiceError,
    AppUpdateOrganizationMemberRoleInput, MembershipDto, OrganizationDto,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    auth::{
        AccountKind, AdminAuthenticatedSession, AdminCreateUserInput, AdminLoginInput,
        AdminSession, AuthError, AuthUserDto,
    },
    login_protection::LoginSurface,
    platform_auth::{platform_token_matches, PlatformSessions},
    turnstile::TurnstileError,
    ServerState,
};

const ADMIN_AUTHORIZATION_IDENTIFIER: &str = "admin-authorization";

pub fn router(state: ServerState) -> Router {
    let protection_state = state.clone();
    let base_path = format!("/{}", state.admin_path);
    let api_path = format!("{base_path}/api");
    Router::new()
        .route(&base_path, get(admin_page))
        .route(&format!("{base_path}/"), get(admin_page))
        .route(&format!("{api_path}/security"), get(admin_security))
        .route(&format!("{api_path}/login"), post(admin_login))
        .route(&format!("{api_path}/platform-login"), post(platform_login))
        .route(&format!("{api_path}/logout"), post(admin_logout))
        .route(&format!("{api_path}/me"), get(admin_me))
        .route(
            &format!("{api_path}/organizations"),
            get(list_organizations).post(create_organization),
        )
        .route(
            &format!("{api_path}/organizations/:organization_id/members"),
            get(list_members).post(add_member),
        )
        .route(
            &format!("{api_path}/organizations/:organization_id/members/:membership_id"),
            patch(update_member_role).delete(remove_member),
        )
        .route(
            &format!("{api_path}/organizations/:organization_id/members/:membership_id/password"),
            patch(reset_member_password),
        )
        .with_state(state)
        .layer(middleware::from_fn_with_state(
            protection_state,
            protect_admin_authorization,
        ))
}

async fn admin_page() -> Html<&'static str> {
    Html(include_str!("admin_v2.html"))
}

async fn protect_admin_authorization(
    State(state): State<ServerState>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    let api_path = format!("/{}/api", state.admin_path);
    if !path.starts_with(&format!("{api_path}/"))
        || matches!(
            path.strip_prefix(&api_path),
            Some("/security" | "/login" | "/platform-login")
        )
    {
        return next.run(request).await;
    }
    let Some(peer_ip) = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|connect_info| connect_info.0.ip())
    else {
        return AdminApiError::internal("client address is unavailable").into_response();
    };
    if let Err(error) = check_login_attempt(
        &state,
        peer_ip,
        LoginSurface::AdminAuthorization,
        ADMIN_AUTHORIZATION_IDENTIFIER,
    ) {
        return error.into_response();
    }

    let response = next.run(request).await;
    if response.status() == StatusCode::UNAUTHORIZED {
        return match record_login_failure(
            &state,
            peer_ip,
            LoginSurface::AdminAuthorization,
            ADMIN_AUTHORIZATION_IDENTIFIER,
        ) {
            Ok(Some(error)) => AdminApiError::from_auth(error).into_response(),
            Ok(None) => response,
            Err(error) => error.into_response(),
        };
    }
    if let Err(error) = record_login_success(
        &state,
        peer_ip,
        LoginSurface::AdminAuthorization,
        ADMIN_AUTHORIZATION_IDENTIFIER,
    ) {
        return error.into_response();
    }
    response
}

async fn admin_login(
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    State(state): State<ServerState>,
    Json(request): Json<AdminLoginRequest>,
) -> Result<Json<AdminSession>, AdminApiError> {
    let identifier = request.identifier.trim().to_lowercase();
    check_login_attempt(
        &state,
        peer_addr.ip(),
        LoginSurface::OrganizationAdmin,
        &identifier,
    )?;
    state
        .turnstile
        .verify(&request.turnstile_token, peer_addr.ip())
        .await
        .map_err(AdminApiError::from_turnstile)?;
    let (email, phone) = if identifier.contains('@') {
        (Some(identifier.clone()), None)
    } else {
        (None, Some(identifier.clone()))
    };
    let _write_guard = state.write_gate.lock().await;
    let (login_result, staged_auth) = {
        let auth = lock_auth(&state)?;
        let mut staged_auth = auth.clone();
        let login_result = staged_auth.admin_login(AdminLoginInput {
            email,
            phone,
            password: request.password,
        });
        (login_result, staged_auth)
    };
    if login_result.is_ok() {
        state
            .storage
            .save_auth(staged_auth.snapshot())
            .await
            .map_err(AdminApiError::from_storage)?;
        *lock_auth(&state)? = staged_auth;
    }
    let session = match login_result {
        Ok(session) => {
            record_login_success(
                &state,
                peer_addr.ip(),
                LoginSurface::OrganizationAdmin,
                &identifier,
            )?;
            session
        }
        Err(AuthError::InvalidCredentials) => {
            if let Some(error) = record_login_failure(
                &state,
                peer_addr.ip(),
                LoginSurface::OrganizationAdmin,
                &identifier,
            )? {
                return Err(AdminApiError::from_auth(error));
            }
            return Err(AdminApiError::from_auth(AuthError::InvalidCredentials));
        }
        Err(error) => {
            record_login_success(
                &state,
                peer_addr.ip(),
                LoginSurface::OrganizationAdmin,
                &identifier,
            )?;
            return Err(AdminApiError::from_auth(error));
        }
    };
    Ok(Json(session))
}

async fn admin_security(State(state): State<ServerState>) -> Json<AdminSecurityResponse> {
    Json(AdminSecurityResponse {
        turnstile_enabled: state.turnstile.is_enabled(),
        turnstile_site_key: state.turnstile.site_key().map(str::to_string),
    })
}

async fn platform_login(
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    State(state): State<ServerState>,
    Json(request): Json<PlatformLoginRequest>,
) -> Result<Json<PlatformLoginResponse>, AdminApiError> {
    check_login_attempt(
        &state,
        peer_addr.ip(),
        LoginSurface::Platform,
        "platform-token",
    )?;
    state
        .turnstile
        .verify(&request.turnstile_token, peer_addr.ip())
        .await
        .map_err(AdminApiError::from_turnstile)?;
    if !platform_token_matches(&state.admin_token, &request.platform_token) {
        if let Some(error) = record_login_failure(
            &state,
            peer_addr.ip(),
            LoginSurface::Platform,
            "platform-token",
        )? {
            return Err(AdminApiError::from_auth(error));
        }
        return Err(AdminApiError::from_auth(AuthError::InvalidCredentials));
    }

    record_login_success(
        &state,
        peer_addr.ip(),
        LoginSurface::Platform,
        "platform-token",
    )?;
    let access_token = lock_platform_sessions(&state)?.issue();
    Ok(Json(PlatformLoginResponse { access_token }))
}

async fn admin_logout(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<StatusCode, AdminApiError> {
    let token = bearer_token(&headers)?;
    let _write_guard = state.write_gate.lock().await;
    let principal = authenticate_principal(&headers, &state)?;
    match principal {
        AdminPrincipal::Platform => lock_platform_sessions(&state)?.revoke(token),
        AdminPrincipal::Organization(_) => {
            let staged_auth = {
                let auth = lock_auth(&state)?;
                let mut staged_auth = auth.clone();
                staged_auth
                    .logout(token)
                    .map_err(AdminApiError::from_auth)?;
                staged_auth
            };
            state
                .storage
                .save_auth(staged_auth.snapshot())
                .await
                .map_err(AdminApiError::from_storage)?;
            *lock_auth(&state)? = staged_auth;
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn admin_me(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<AdminMeResponse>, AdminApiError> {
    let principal = authenticate_principal(&headers, &state)?;
    Ok(Json(match principal {
        AdminPrincipal::Platform => AdminMeResponse {
            status: "ok",
            scope: "platform",
            user: None,
            organization_id: None,
        },
        AdminPrincipal::Organization(session) => AdminMeResponse {
            status: "ok",
            scope: "organization",
            user: Some(session.user),
            organization_id: Some(session.organization_id),
        },
    }))
}

async fn list_organizations(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<Vec<OrganizationWithMembers>>, AdminApiError> {
    let principal = authenticate_principal(&headers, &state)?;
    let service = lock_service(&state)?;
    let organization_filter = principal.organization_id();
    let organizations = service
        .organizations()
        .into_iter()
        .filter(|organization| {
            organization_filter.is_none_or(|organization_id| {
                Uuid::parse_str(&organization.id).ok() == Some(organization_id)
            })
        })
        .map(|organization| {
            let organization_id = Uuid::parse_str(&organization.id)
                .map_err(|_| AdminApiError::internal("invalid organization id"))?;
            organization_with_members(&service, organization_id)
        })
        .collect::<Result<Vec<_>, AdminApiError>>()?;
    Ok(Json(organizations))
}

async fn create_organization(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(request): Json<CreateOrganizationRequest>,
) -> Result<Json<OrganizationWithMembers>, AdminApiError> {
    let _write_guard = state.write_gate.lock().await;
    require_platform(authenticate_principal(&headers, &state)?)?;
    let CreateOrganizationRequest {
        organization_name,
        admin_display_name,
        admin_email,
        admin_phone,
        admin_password,
    } = request;

    let (mut staged_auth, mut staged_service) = {
        let auth = lock_auth(&state)?;
        let service = lock_service(&state)?;
        (auth.clone(), service.clone())
    };
    let admin_user_id = Uuid::new_v4();
    let admin_member = staged_service
        .create_organization(AppCreateOrganizationInput {
            organization_name,
            admin_user_id,
            admin_display_name: admin_display_name.clone(),
            admin_email: admin_email.clone(),
            admin_phone: admin_phone.clone(),
        })
        .map_err(AdminApiError::from_service)?;
    let organization_id = Uuid::parse_str(&admin_member.organization_id)
        .map_err(|_| AdminApiError::internal("invalid organization id"))?;
    staged_auth
        .create_or_update_admin_user(AdminCreateUserInput {
            user_id: admin_user_id,
            display_name: admin_display_name,
            email: admin_email,
            phone: admin_phone,
            password: Some(admin_password),
            account_kind: AccountKind::OrganizationAdmin,
            organization_id: Some(organization_id),
        })
        .map_err(AdminApiError::from_auth)?;
    let response = organization_with_members(&staged_service, organization_id)?;

    persist_staged_state(&state, staged_auth, staged_service).await?;
    Ok(Json(response))
}

async fn list_members(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(organization_id): Path<Uuid>,
) -> Result<Json<Vec<MembershipDto>>, AdminApiError> {
    require_organization_admin(authenticate_principal(&headers, &state)?, organization_id)?;
    let service = lock_service(&state)?;
    Ok(Json(
        service
            .organization_members(organization_id)
            .map_err(AdminApiError::from_service)?,
    ))
}

async fn add_member(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(organization_id): Path<Uuid>,
    Json(request): Json<AddMemberRequest>,
) -> Result<Json<MembershipDto>, AdminApiError> {
    let _write_guard = state.write_gate.lock().await;
    require_organization_admin(authenticate_principal(&headers, &state)?, organization_id)?;
    require_employee_role(request.role)?;
    let AddMemberRequest {
        display_name,
        email,
        phone,
        password,
        role,
    } = request;

    let (mut staged_auth, mut staged_service) = {
        let auth = lock_auth(&state)?;
        let service = lock_service(&state)?;
        (auth.clone(), service.clone())
    };
    let member = staged_service
        .add_organization_member(AppAddOrganizationMemberInput {
            organization_id,
            display_name,
            email,
            phone,
            role,
        })
        .map_err(AdminApiError::from_service)?;
    sync_business_auth_user(&mut staged_auth, &member, password)?;
    persist_staged_state(&state, staged_auth, staged_service).await?;
    Ok(Json(member))
}

async fn update_member_role(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path((organization_id, membership_id)): Path<(Uuid, Uuid)>,
    Json(request): Json<UpdateMemberRoleRequest>,
) -> Result<Json<MembershipDto>, AdminApiError> {
    let _write_guard = state.write_gate.lock().await;
    require_organization_admin(authenticate_principal(&headers, &state)?, organization_id)?;
    require_employee_role(request.role)?;
    let (member, staged_service) = {
        let auth = lock_auth(&state)?;
        let service = lock_service(&state)?;
        let mut staged_service = service.clone();
        let member = find_member(&staged_service, organization_id, membership_id)?;
        require_business_member(&auth, &member)?;
        let member = staged_service
            .update_organization_member_role(AppUpdateOrganizationMemberRoleInput {
                organization_id,
                membership_id,
                role: request.role,
            })
            .map_err(AdminApiError::from_service)?;
        (member, staged_service)
    };
    persist_service(&state, staged_service).await?;
    Ok(Json(member))
}

async fn remove_member(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path((organization_id, membership_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AdminApiError> {
    let _write_guard = state.write_gate.lock().await;
    require_organization_admin(authenticate_principal(&headers, &state)?, organization_id)?;
    let staged_service = {
        let auth = lock_auth(&state)?;
        let service = lock_service(&state)?;
        let mut staged_service = service.clone();
        let member = find_member(&staged_service, organization_id, membership_id)?;
        require_business_member(&auth, &member)?;
        staged_service
            .remove_organization_member(organization_id, membership_id)
            .map_err(AdminApiError::from_service)?;
        staged_service
    };
    persist_service(&state, staged_service).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn reset_member_password(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path((organization_id, membership_id)): Path<(Uuid, Uuid)>,
    Json(request): Json<ResetPasswordRequest>,
) -> Result<Json<MembershipDto>, AdminApiError> {
    let _write_guard = state.write_gate.lock().await;
    require_organization_admin(authenticate_principal(&headers, &state)?, organization_id)?;
    let (member, staged_auth) = {
        let auth = lock_auth(&state)?;
        let service = lock_service(&state)?;
        let member = find_member(&service, organization_id, membership_id)?;
        require_business_member(&auth, &member)?;
        let mut staged_auth = auth.clone();
        sync_business_auth_user(&mut staged_auth, &member, Some(request.password))?;
        (member, staged_auth)
    };
    state
        .storage
        .save_auth(staged_auth.snapshot())
        .await
        .map_err(AdminApiError::from_storage)?;
    let mut auth = lock_auth(&state)?;
    *auth = staged_auth;
    Ok(Json(member))
}

fn organization_with_members(
    service: &AppLedgerService,
    organization_id: Uuid,
) -> Result<OrganizationWithMembers, AdminApiError> {
    let organization = service
        .organizations()
        .into_iter()
        .find(|organization| Uuid::parse_str(&organization.id).ok() == Some(organization_id))
        .ok_or_else(|| AdminApiError::from_service(AppServiceError::OrganizationNotFound))?;
    let members = service
        .organization_members(organization_id)
        .map_err(AdminApiError::from_service)?;
    Ok(OrganizationWithMembers {
        organization,
        members,
    })
}

fn find_member(
    service: &AppLedgerService,
    organization_id: Uuid,
    membership_id: Uuid,
) -> Result<MembershipDto, AdminApiError> {
    service
        .organization_members(organization_id)
        .map_err(AdminApiError::from_service)?
        .into_iter()
        .find(|member| Uuid::parse_str(&member.id).ok() == Some(membership_id))
        .ok_or_else(|| AdminApiError::from_service(AppServiceError::MembershipNotFound))
}

fn sync_business_auth_user(
    auth: &mut crate::auth::AuthService,
    member: &MembershipDto,
    password: Option<String>,
) -> Result<(), AdminApiError> {
    auth.create_or_update_admin_user(AdminCreateUserInput {
        user_id: Uuid::parse_str(&member.user_id)
            .map_err(|_| AdminApiError::internal("invalid member user id"))?,
        display_name: member.display_name.clone(),
        email: member.email.clone(),
        phone: member.phone.clone(),
        password,
        account_kind: AccountKind::Business,
        organization_id: None,
    })
    .map_err(AdminApiError::from_auth)?;
    Ok(())
}

fn require_employee_role(role: MembershipRole) -> Result<(), AdminApiError> {
    if matches!(
        role,
        MembershipRole::BusinessOwner | MembershipRole::Employee
    ) {
        Ok(())
    } else {
        Err(AdminApiError::bad_request(
            "business role must be business_owner or employee",
        ))
    }
}

fn require_business_member(
    auth: &crate::auth::AuthService,
    member: &MembershipDto,
) -> Result<(), AdminApiError> {
    let user_id = Uuid::parse_str(&member.user_id)
        .map_err(|_| AdminApiError::internal("invalid member user id"))?;
    if matches!(member.role.as_str(), "owner" | "admin")
        || auth.account_kind(user_id) == Some(AccountKind::OrganizationAdmin)
    {
        Err(AdminApiError::forbidden(
            "organization admin accounts cannot be managed as employees",
        ))
    } else {
        Ok(())
    }
}

#[derive(Debug)]
enum AdminPrincipal {
    Platform,
    Organization(AdminAuthenticatedSession),
}

impl AdminPrincipal {
    fn organization_id(&self) -> Option<Uuid> {
        match self {
            Self::Platform => None,
            Self::Organization(session) => Some(session.organization_id),
        }
    }
}

fn authenticate_principal(
    headers: &HeaderMap,
    state: &ServerState,
) -> Result<AdminPrincipal, AdminApiError> {
    let token = bearer_token(headers)?;
    if lock_platform_sessions(state)?.authenticate(token) {
        return Ok(AdminPrincipal::Platform);
    }
    let auth = lock_auth(state)?;
    auth.authenticate_admin_access_token(token)
        .map(AdminPrincipal::Organization)
        .map_err(AdminApiError::from_auth)
}

fn require_platform(principal: AdminPrincipal) -> Result<(), AdminApiError> {
    if matches!(principal, AdminPrincipal::Platform) {
        Ok(())
    } else {
        Err(AdminApiError::forbidden("platform admin token is required"))
    }
}

fn require_organization_admin(
    principal: AdminPrincipal,
    organization_id: Uuid,
) -> Result<(), AdminApiError> {
    match principal {
        AdminPrincipal::Organization(session) if session.organization_id == organization_id => {
            Ok(())
        }
        AdminPrincipal::Organization(_) => Err(AdminApiError::forbidden(
            "organization admin cannot manage another organization",
        )),
        AdminPrincipal::Platform => Err(AdminApiError::forbidden(
            "platform admin cannot manage organization employees",
        )),
    }
}

fn bearer_token(headers: &HeaderMap) -> Result<&str, AdminApiError> {
    headers
        .get(AUTHORIZATION)
        .and_then(|header| header.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer ").or(Some(value)))
        .filter(|value| !value.trim().is_empty())
        .map(str::trim)
        .ok_or_else(|| AdminApiError::unauthorized("admin authorization required"))
}

fn lock_auth(
    state: &ServerState,
) -> Result<std::sync::MutexGuard<'_, crate::auth::AuthService>, AdminApiError> {
    state
        .auth_service
        .lock()
        .map_err(|_| AdminApiError::internal("auth service lock poisoned"))
}

fn lock_service(
    state: &ServerState,
) -> Result<std::sync::MutexGuard<'_, AppLedgerService>, AdminApiError> {
    state
        .ledger_service
        .lock()
        .map_err(|_| AdminApiError::internal("ledger service lock poisoned"))
}

fn lock_platform_sessions(
    state: &ServerState,
) -> Result<std::sync::MutexGuard<'_, PlatformSessions>, AdminApiError> {
    state
        .platform_sessions
        .lock()
        .map_err(|_| AdminApiError::internal("platform session lock poisoned"))
}

fn check_login_attempt(
    state: &ServerState,
    ip: IpAddr,
    surface: LoginSurface,
    identifier: &str,
) -> Result<(), AdminApiError> {
    state
        .login_protection
        .lock()
        .map_err(|_| AdminApiError::internal("login protection lock poisoned"))?
        .check(ip, surface, identifier)
        .map_err(AdminApiError::from_auth)
}

fn record_login_failure(
    state: &ServerState,
    ip: IpAddr,
    surface: LoginSurface,
    identifier: &str,
) -> Result<Option<AuthError>, AdminApiError> {
    Ok(state
        .login_protection
        .lock()
        .map_err(|_| AdminApiError::internal("login protection lock poisoned"))?
        .record_failure(ip, surface, identifier))
}

fn record_login_success(
    state: &ServerState,
    ip: IpAddr,
    surface: LoginSurface,
    identifier: &str,
) -> Result<(), AdminApiError> {
    state
        .login_protection
        .lock()
        .map_err(|_| AdminApiError::internal("login protection lock poisoned"))?
        .record_success(ip, surface, identifier);
    Ok(())
}

async fn persist_service(
    state: &ServerState,
    staged_service: AppLedgerService,
) -> Result<(), AdminApiError> {
    state
        .storage
        .save_ledger(staged_service.snapshot())
        .await
        .map_err(AdminApiError::from_storage)?;
    *lock_service(state)? = staged_service;
    Ok(())
}

async fn persist_staged_state(
    state: &ServerState,
    staged_auth: crate::auth::AuthService,
    staged_service: AppLedgerService,
) -> Result<(), AdminApiError> {
    state
        .storage
        .save_all(staged_service.snapshot(), staged_auth.snapshot())
        .await
        .map_err(AdminApiError::from_storage)?;
    let mut auth = lock_auth(state)?;
    let mut service = lock_service(state)?;
    *auth = staged_auth;
    *service = staged_service;
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminMeResponse {
    status: &'static str,
    scope: &'static str,
    user: Option<AuthUserDto>,
    organization_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OrganizationWithMembers {
    organization: OrganizationDto,
    members: Vec<MembershipDto>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdminLoginRequest {
    identifier: String,
    password: String,
    #[serde(default)]
    turnstile_token: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlatformLoginRequest {
    platform_token: String,
    #[serde(default)]
    turnstile_token: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PlatformLoginResponse {
    access_token: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminSecurityResponse {
    turnstile_enabled: bool,
    turnstile_site_key: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateOrganizationRequest {
    organization_name: String,
    admin_display_name: String,
    admin_email: Option<String>,
    admin_phone: Option<String>,
    admin_password: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddMemberRequest {
    display_name: String,
    email: Option<String>,
    phone: Option<String>,
    password: Option<String>,
    role: MembershipRole,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateMemberRoleRequest {
    role: MembershipRole,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResetPasswordRequest {
    password: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorResponse {
    error: String,
}

#[derive(Debug)]
struct AdminApiError {
    status: StatusCode,
    message: String,
    retry_after_seconds: Option<u64>,
}

impl AdminApiError {
    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: message.into(),
            retry_after_seconds: None,
        }
    }

    fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: message.into(),
            retry_after_seconds: None,
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
            retry_after_seconds: None,
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
            retry_after_seconds: None,
        }
    }

    fn from_service(error: AppServiceError) -> Self {
        let status = match error {
            AppServiceError::OrganizationNotFound | AppServiceError::MembershipNotFound => {
                StatusCode::NOT_FOUND
            }
            AppServiceError::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
            AppServiceError::Unauthorized => StatusCode::FORBIDDEN,
            _ => StatusCode::BAD_REQUEST,
        };
        Self {
            status,
            message: error.to_string(),
            retry_after_seconds: None,
        }
    }

    fn from_auth(error: AuthError) -> Self {
        let retry_after_seconds = match &error {
            AuthError::LoginRateLimited {
                retry_after_seconds,
            } => Some(*retry_after_seconds),
            _ => None,
        };
        let status = match &error {
            AuthError::InvalidCredentials | AuthError::SessionNotFound => StatusCode::UNAUTHORIZED,
            AuthError::LoginRateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            AuthError::AdminAccessDenied | AuthError::BusinessAppAccessDenied => {
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

    fn from_storage(error: anyhow::Error) -> Self {
        Self::internal(format!("storage error: {error}"))
    }

    fn from_turnstile(error: TurnstileError) -> Self {
        let status = match error {
            TurnstileError::TokenRequired => StatusCode::BAD_REQUEST,
            TurnstileError::Rejected => StatusCode::FORBIDDEN,
            TurnstileError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        };
        Self {
            status,
            message: error.to_string(),
            retry_after_seconds: None,
        }
    }
}

impl IntoResponse for AdminApiError {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::LoginInput;
    use axum::http::HeaderValue;

    fn test_state() -> ServerState {
        let data_dir = std::env::temp_dir().join(format!("cloudledger-admin-{}", Uuid::new_v4()));
        ServerState::load(data_dir).expect("server state")
    }

    fn peer_addr() -> ConnectInfo<SocketAddr> {
        ConnectInfo("127.0.0.1:42000".parse().expect("peer address"))
    }

    fn authorization_headers(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).expect("token header"),
        );
        headers
    }

    fn platform_headers(state: &ServerState) -> HeaderMap {
        let token = state
            .platform_sessions
            .lock()
            .expect("platform sessions")
            .issue();
        authorization_headers(&token)
    }

    fn organization_request(name: &str, email: &str) -> CreateOrganizationRequest {
        CreateOrganizationRequest {
            organization_name: name.to_string(),
            admin_display_name: format!("{name} Admin"),
            admin_email: Some(email.to_string()),
            admin_phone: None,
            admin_password: "admin-password".to_string(),
        }
    }

    async fn create_test_organization(
        state: &ServerState,
        name: &str,
        email: &str,
    ) -> OrganizationWithMembers {
        create_organization(
            State(state.clone()),
            platform_headers(state),
            Json(organization_request(name, email)),
        )
        .await
        .expect("create organization")
        .0
    }

    async fn organization_admin_headers(state: &ServerState, email: &str) -> HeaderMap {
        let session = admin_login(
            peer_addr(),
            State(state.clone()),
            Json(AdminLoginRequest {
                identifier: email.to_string(),
                password: "admin-password".to_string(),
                turnstile_token: String::new(),
            }),
        )
        .await
        .expect("admin login")
        .0;
        authorization_headers(&session.access_token)
    }

    #[tokio::test]
    async fn platform_creates_multiple_scoped_organization_admins() {
        let state = test_state();
        let first = create_test_organization(&state, "First", "first-admin@example.com").await;
        let second = create_test_organization(&state, "Second", "second-admin@example.com").await;
        let organizations = list_organizations(State(state.clone()), platform_headers(&state))
            .await
            .expect("list organizations")
            .0;

        assert_eq!(organizations.len(), 2);
        assert_ne!(first.organization.id, second.organization.id);

        let mut auth = state.auth_service.lock().expect("auth lock");
        let app_login = auth.login(LoginInput {
            email: Some("first-admin@example.com".to_string()),
            phone: None,
            password: "admin-password".to_string(),
            installation_id: "admin-phone".to_string(),
        });
        assert_eq!(app_login.unwrap_err(), AuthError::BusinessAppAccessDenied);
    }

    #[tokio::test]
    async fn platform_token_must_be_exchanged_for_a_session() {
        let state = test_state();
        let raw_token_error = list_organizations(
            State(state.clone()),
            authorization_headers(&state.admin_token),
        )
        .await
        .expect_err("raw platform token is not an API session");
        assert_eq!(raw_token_error.status, StatusCode::UNAUTHORIZED);

        let session = platform_login(
            peer_addr(),
            State(state.clone()),
            Json(PlatformLoginRequest {
                platform_token: state.admin_token.to_string(),
                turnstile_token: String::new(),
            }),
        )
        .await
        .expect("exchange platform token")
        .0;
        let organizations =
            list_organizations(State(state), authorization_headers(&session.access_token))
                .await
                .expect("platform session is authorized")
                .0;
        assert!(organizations.is_empty());
    }

    #[tokio::test]
    async fn organization_admin_manages_only_own_employees() {
        let state = test_state();
        let first = create_test_organization(&state, "First", "first-admin@example.com").await;
        let second = create_test_organization(&state, "Second", "second-admin@example.com").await;
        let first_id = Uuid::parse_str(&first.organization.id).expect("first organization id");
        let second_id = Uuid::parse_str(&second.organization.id).expect("second organization id");
        let headers = organization_admin_headers(&state, "first-admin@example.com").await;

        let member = add_member(
            State(state.clone()),
            headers.clone(),
            Path(first_id),
            Json(AddMemberRequest {
                display_name: "Employee".to_string(),
                email: Some("employee@example.com".to_string()),
                phone: None,
                password: Some("employee-password".to_string()),
                role: MembershipRole::Employee,
            }),
        )
        .await
        .expect("add employee")
        .0;
        assert_eq!(member.organization_id, first.organization.id);

        let cross_organization = add_member(
            State(state.clone()),
            headers,
            Path(second_id),
            Json(AddMemberRequest {
                display_name: "Forbidden".to_string(),
                email: Some("forbidden@example.com".to_string()),
                phone: None,
                password: Some("employee-password".to_string()),
                role: MembershipRole::Employee,
            }),
        )
        .await
        .expect_err("cross organization access rejected");
        assert_eq!(cross_organization.status, StatusCode::FORBIDDEN);

        let second_headers = organization_admin_headers(&state, "second-admin@example.com").await;
        let shared_employee = add_member(
            State(state.clone()),
            second_headers,
            Path(second_id),
            Json(AddMemberRequest {
                display_name: "Shared Employee".to_string(),
                email: Some("employee@example.com".to_string()),
                phone: None,
                password: Some("another-password".to_string()),
                role: MembershipRole::Employee,
            }),
        )
        .await
        .expect_err("employee cannot be shared across organizations");
        assert_eq!(shared_employee.status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn business_employee_cannot_log_into_admin_backend() {
        let state = test_state();
        let organization =
            create_test_organization(&state, "First", "first-admin@example.com").await;
        let organization_id =
            Uuid::parse_str(&organization.organization.id).expect("organization id");
        let headers = organization_admin_headers(&state, "first-admin@example.com").await;
        let _ = add_member(
            State(state.clone()),
            headers,
            Path(organization_id),
            Json(AddMemberRequest {
                display_name: "Employee".to_string(),
                email: Some("employee@example.com".to_string()),
                phone: None,
                password: Some("employee-password".to_string()),
                role: MembershipRole::Employee,
            }),
        )
        .await
        .expect("add employee");

        let error = admin_login(
            peer_addr(),
            State(state),
            Json(AdminLoginRequest {
                identifier: "employee@example.com".to_string(),
                password: "employee-password".to_string(),
                turnstile_token: String::new(),
            }),
        )
        .await
        .expect_err("business login rejected");
        assert_eq!(error.status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn repeated_bad_passwords_lock_admin_login() {
        let state = test_state();
        create_test_organization(&state, "First", "first-admin@example.com").await;

        for _ in 0..4 {
            let error = admin_login(
                peer_addr(),
                State(state.clone()),
                Json(AdminLoginRequest {
                    identifier: "first-admin@example.com".to_string(),
                    password: "incorrect-password".to_string(),
                    turnstile_token: String::new(),
                }),
            )
            .await
            .expect_err("bad password rejected");
            assert_eq!(error.status, StatusCode::UNAUTHORIZED);
        }

        let locked = admin_login(
            peer_addr(),
            State(state.clone()),
            Json(AdminLoginRequest {
                identifier: "first-admin@example.com".to_string(),
                password: "incorrect-password".to_string(),
                turnstile_token: String::new(),
            }),
        )
        .await
        .expect_err("fifth failure locks login");
        assert_eq!(locked.status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(locked.retry_after_seconds, Some(15 * 60));

        let still_locked = admin_login(
            peer_addr(),
            State(state),
            Json(AdminLoginRequest {
                identifier: "first-admin@example.com".to_string(),
                password: "admin-password".to_string(),
                turnstile_token: String::new(),
            }),
        )
        .await
        .expect_err("correct password waits for lockout expiry");
        assert_eq!(still_locked.status, StatusCode::TOO_MANY_REQUESTS);
    }
}
