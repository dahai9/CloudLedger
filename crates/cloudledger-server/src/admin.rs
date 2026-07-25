use axum::{
    extract::{Path, State},
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
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
    ServerState,
};

pub fn router(state: ServerState) -> Router {
    Router::new()
        .route("/admin", get(admin_page))
        .route("/admin/", get(admin_page))
        .route("/admin/api/login", post(admin_login))
        .route("/admin/api/logout", post(admin_logout))
        .route("/admin/api/me", get(admin_me))
        .route(
            "/admin/api/organizations",
            get(list_organizations).post(create_organization),
        )
        .route(
            "/admin/api/organizations/:organization_id/members",
            get(list_members).post(add_member),
        )
        .route(
            "/admin/api/organizations/:organization_id/members/:membership_id",
            patch(update_member_role).delete(remove_member),
        )
        .route(
            "/admin/api/organizations/:organization_id/members/:membership_id/password",
            patch(reset_member_password),
        )
        .with_state(state)
}

async fn admin_page() -> Html<&'static str> {
    Html(include_str!("admin_v2.html"))
}

async fn admin_login(
    State(state): State<ServerState>,
    Json(request): Json<AdminLoginRequest>,
) -> Result<Json<AdminSession>, AdminApiError> {
    let identifier = request.identifier.trim();
    let (email, phone) = if identifier.contains('@') {
        (Some(identifier.to_string()), None)
    } else {
        (None, Some(identifier.to_string()))
    };
    let mut auth = lock_auth(&state)?;
    let session = auth
        .admin_login(AdminLoginInput {
            email,
            phone,
            password: request.password,
        })
        .map_err(AdminApiError::from_auth)?;
    auth.save_to_path(&state.auth_state_path)
        .map_err(AdminApiError::from_auth)?;
    Ok(Json(session))
}

async fn admin_logout(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<StatusCode, AdminApiError> {
    let principal = authenticate_principal(&headers, &state)?;
    if matches!(principal, AdminPrincipal::Organization(_)) {
        let token = bearer_token(&headers)?;
        let mut auth = lock_auth(&state)?;
        auth.logout(token).map_err(AdminApiError::from_auth)?;
        auth.save_to_path(&state.auth_state_path)
            .map_err(AdminApiError::from_auth)?;
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
    require_platform(authenticate_principal(&headers, &state)?)?;
    let CreateOrganizationRequest {
        organization_name,
        admin_display_name,
        admin_email,
        admin_phone,
        admin_password,
    } = request;

    let mut auth = lock_auth(&state)?;
    let mut service = lock_service(&state)?;
    let original_service = service.clone();
    let mut staged_auth = auth.clone();
    let mut staged_service = original_service.clone();
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

    persist_staged_state(
        &state,
        &mut auth,
        &mut service,
        staged_auth,
        staged_service,
        original_service,
    )?;
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
    require_organization_admin(authenticate_principal(&headers, &state)?, organization_id)?;
    require_employee_role(request.role)?;
    let AddMemberRequest {
        display_name,
        email,
        phone,
        password,
        role,
    } = request;

    let mut auth = lock_auth(&state)?;
    let mut service = lock_service(&state)?;
    let original_service = service.clone();
    let mut staged_auth = auth.clone();
    let mut staged_service = original_service.clone();
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
    persist_staged_state(
        &state,
        &mut auth,
        &mut service,
        staged_auth,
        staged_service,
        original_service,
    )?;
    Ok(Json(member))
}

async fn update_member_role(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path((organization_id, membership_id)): Path<(Uuid, Uuid)>,
    Json(request): Json<UpdateMemberRoleRequest>,
) -> Result<Json<MembershipDto>, AdminApiError> {
    require_organization_admin(authenticate_principal(&headers, &state)?, organization_id)?;
    require_employee_role(request.role)?;
    let auth = lock_auth(&state)?;
    let mut service = lock_service(&state)?;
    let member = find_member(&service, organization_id, membership_id)?;
    require_business_member(&auth, &member)?;
    let member = service
        .update_organization_member_role(AppUpdateOrganizationMemberRoleInput {
            organization_id,
            membership_id,
            role: request.role,
        })
        .map_err(AdminApiError::from_service)?;
    persist_service(&state, &service)?;
    Ok(Json(member))
}

async fn remove_member(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path((organization_id, membership_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AdminApiError> {
    require_organization_admin(authenticate_principal(&headers, &state)?, organization_id)?;
    let auth = lock_auth(&state)?;
    let mut service = lock_service(&state)?;
    let member = find_member(&service, organization_id, membership_id)?;
    require_business_member(&auth, &member)?;
    service
        .remove_organization_member(organization_id, membership_id)
        .map_err(AdminApiError::from_service)?;
    persist_service(&state, &service)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn reset_member_password(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path((organization_id, membership_id)): Path<(Uuid, Uuid)>,
    Json(request): Json<ResetPasswordRequest>,
) -> Result<Json<MembershipDto>, AdminApiError> {
    require_organization_admin(authenticate_principal(&headers, &state)?, organization_id)?;
    let mut auth = lock_auth(&state)?;
    let service = lock_service(&state)?;
    let member = find_member(&service, organization_id, membership_id)?;
    require_business_member(&auth, &member)?;
    let mut staged_auth = auth.clone();
    sync_business_auth_user(&mut staged_auth, &member, Some(request.password))?;
    staged_auth
        .save_to_path(&state.auth_state_path)
        .map_err(AdminApiError::from_auth)?;
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
        MembershipRole::Accountant
            | MembershipRole::Approver
            | MembershipRole::Member
            | MembershipRole::Viewer
    ) {
        Ok(())
    } else {
        Err(AdminApiError::bad_request(
            "employee role must be accountant, approver, member, or viewer",
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
    if token == state.admin_token.as_str() {
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

fn persist_service(state: &ServerState, service: &AppLedgerService) -> Result<(), AdminApiError> {
    service
        .save_to_path(&state.ledger_state_path)
        .map_err(AdminApiError::from_service)
}

fn persist_staged_state(
    state: &ServerState,
    auth: &mut crate::auth::AuthService,
    service: &mut AppLedgerService,
    staged_auth: crate::auth::AuthService,
    staged_service: AppLedgerService,
    original_service: AppLedgerService,
) -> Result<(), AdminApiError> {
    staged_service
        .save_to_path(&state.ledger_state_path)
        .map_err(AdminApiError::from_service)?;
    if let Err(error) = staged_auth.save_to_path(&state.auth_state_path) {
        let _ = original_service.save_to_path(&state.ledger_state_path);
        return Err(AdminApiError::from_auth(error));
    }
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
}

impl AdminApiError {
    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: message.into(),
        }
    }

    fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: message.into(),
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
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
        }
    }

    fn from_auth(error: AuthError) -> Self {
        let status = match error {
            AuthError::InvalidCredentials | AuthError::SessionNotFound => StatusCode::UNAUTHORIZED,
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
            | AuthError::InstallationIdRequired => StatusCode::BAD_REQUEST,
        };
        Self {
            status,
            message: error.to_string(),
        }
    }
}

impl IntoResponse for AdminApiError {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::LoginInput;
    use axum::http::HeaderValue;

    fn test_state() -> ServerState {
        let data_dir = std::env::temp_dir().join(format!("cloudledger-admin-{}", Uuid::new_v4()));
        ServerState::load(data_dir).expect("server state")
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
        authorization_headers(&state.admin_token)
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
            State(state.clone()),
            Json(AdminLoginRequest {
                identifier: email.to_string(),
                password: "admin-password".to_string(),
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
                role: MembershipRole::Member,
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
                role: MembershipRole::Viewer,
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
                role: MembershipRole::Viewer,
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
                role: MembershipRole::Accountant,
            }),
        )
        .await
        .expect("add employee");

        let error = admin_login(
            State(state),
            Json(AdminLoginRequest {
                identifier: "employee@example.com".to_string(),
                password: "employee-password".to_string(),
            }),
        )
        .await
        .expect_err("business login rejected");
        assert_eq!(error.status, StatusCode::FORBIDDEN);
    }
}
