use axum::{
    extract::{Path, State},
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, patch},
    Json, Router,
};
use cloudledger_core::MembershipRole;
use cloudledger_service::{
    AppAddOrganizationMemberInput, AppBootstrapOrganizationInput, AppLedgerService,
    AppServiceError, AppSetupStatus, AppUpdateOrganizationMemberRoleInput, MembershipDto,
    OrganizationDto,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    auth::{AdminCreateUserInput, AuthError},
    ServerState,
};

pub fn router(state: ServerState) -> Router {
    Router::new()
        .route("/admin", get(admin_page))
        .route("/admin/", get(admin_page))
        .route("/admin/api/me", get(admin_me))
        .route("/admin/api/setup", get(setup_status).post(setup))
        .route("/admin/api/organizations", get(list_organizations))
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
    Html(include_str!("admin.html"))
}

async fn admin_me(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<AdminMeResponse>, AdminApiError> {
    require_admin(&headers, &state)?;
    Ok(Json(AdminMeResponse {
        status: "ok",
        scope: "admin",
    }))
}

async fn setup_status(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<AppSetupStatus>, AdminApiError> {
    require_admin(&headers, &state)?;
    let service = lock_service(&state)?;
    Ok(Json(service.setup_status()))
}

async fn setup(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(request): Json<SetupRequest>,
) -> Result<Json<SetupCompleteResponse>, AdminApiError> {
    require_admin(&headers, &state)?;
    let SetupRequest {
        organization_name,
        owner_display_name,
        owner_email,
        owner_phone,
        owner_password,
    } = request;

    let mut auth = state
        .auth_service
        .lock()
        .map_err(|_| AdminApiError::internal("auth service lock poisoned"))?;
    let mut service = lock_service(&state)?;
    let original_service = service.clone();
    let mut staged_auth = auth.clone();
    let mut staged_service = original_service.clone();

    let member = staged_service
        .bootstrap_single_organization(AppBootstrapOrganizationInput {
            organization_name,
            owner_user_id: Uuid::new_v4(),
            owner_display_name,
            owner_email,
            owner_phone,
        })
        .map_err(AdminApiError::from_service)?;
    sync_admin_auth_user(&mut staged_auth, &member, Some(owner_password))?;
    let organization_id = Uuid::parse_str(&member.organization_id)
        .map_err(|_| AdminApiError::internal("invalid organization id"))?;
    let response = SetupCompleteResponse {
        setup: staged_service.setup_status(),
        organization: organization_with_members(&staged_service, organization_id)?,
    };

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

async fn list_organizations(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<Vec<OrganizationWithMembers>>, AdminApiError> {
    require_admin(&headers, &state)?;
    let service = lock_service(&state)?;
    let organizations = service
        .organizations()
        .into_iter()
        .map(|organization| {
            let organization_id = Uuid::parse_str(&organization.id)
                .map_err(|_| AdminApiError::internal("invalid organization id"))?;
            let members = service
                .organization_members(organization_id)
                .map_err(AdminApiError::from_service)?;
            Ok(OrganizationWithMembers {
                organization,
                members,
            })
        })
        .collect::<Result<Vec<_>, AdminApiError>>()?;
    Ok(Json(organizations))
}

fn organization_with_members(
    service: &AppLedgerService,
    organization_id: Uuid,
) -> Result<OrganizationWithMembers, AdminApiError> {
    let organization = service
        .organizations()
        .into_iter()
        .find(|organization| {
            Uuid::parse_str(&organization.id)
                .is_ok_and(|parsed_organization_id| parsed_organization_id == organization_id)
        })
        .ok_or_else(|| AdminApiError::from_service(AppServiceError::OrganizationNotFound))?;
    let members = service
        .organization_members(organization_id)
        .map_err(AdminApiError::from_service)?;
    Ok(OrganizationWithMembers {
        organization,
        members,
    })
}

async fn list_members(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(organization_id): Path<Uuid>,
) -> Result<Json<Vec<MembershipDto>>, AdminApiError> {
    require_admin(&headers, &state)?;
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
    require_admin(&headers, &state)?;
    let AddMemberRequest {
        display_name,
        email,
        phone,
        password,
        role,
    } = request;

    let mut auth = state
        .auth_service
        .lock()
        .map_err(|_| AdminApiError::internal("auth service lock poisoned"))?;
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
    sync_admin_auth_user(&mut staged_auth, &member, password)?;
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
    require_admin(&headers, &state)?;
    let mut service = lock_service(&state)?;
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
    require_admin(&headers, &state)?;
    let mut service = lock_service(&state)?;
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
    require_admin(&headers, &state)?;
    let mut auth = state
        .auth_service
        .lock()
        .map_err(|_| AdminApiError::internal("auth service lock poisoned"))?;
    let service = lock_service(&state)?;
    let mut staged_auth = auth.clone();
    let member = service
        .organization_members(organization_id)
        .map_err(AdminApiError::from_service)?
        .into_iter()
        .find(|member| {
            Uuid::parse_str(&member.id)
                .is_ok_and(|parsed_membership_id| parsed_membership_id == membership_id)
        })
        .ok_or_else(|| AdminApiError::from_service(AppServiceError::MembershipNotFound))?;
    staged_auth
        .create_or_update_admin_user(AdminCreateUserInput {
            user_id: Uuid::parse_str(&member.user_id)
                .map_err(|_| AdminApiError::internal("invalid member user id"))?,
            display_name: member.display_name.clone(),
            email: member.email.clone(),
            phone: member.phone.clone(),
            password: Some(request.password),
        })
        .map_err(AdminApiError::from_auth)?;
    staged_auth
        .save_to_path(&state.auth_state_path)
        .map_err(AdminApiError::from_auth)?;
    *auth = staged_auth;
    Ok(Json(member))
}

fn admin_create_user_input(
    member: &MembershipDto,
    password: Option<String>,
) -> Result<AdminCreateUserInput, AdminApiError> {
    Ok(AdminCreateUserInput {
        user_id: Uuid::parse_str(&member.user_id)
            .map_err(|_| AdminApiError::internal("invalid member user id"))?,
        display_name: member.display_name.clone(),
        email: member.email.clone(),
        phone: member.phone.clone(),
        password,
    })
}

fn require_admin(headers: &HeaderMap, state: &ServerState) -> Result<(), AdminApiError> {
    let token = headers
        .get(AUTHORIZATION)
        .and_then(|header| header.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer ").or(Some(value)))
        .unwrap_or_default();
    if token == state.admin_token.as_str() {
        Ok(())
    } else {
        Err(AdminApiError::unauthorized("admin token required"))
    }
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

fn sync_admin_auth_user(
    auth: &mut crate::auth::AuthService,
    member: &MembershipDto,
    password: Option<String>,
) -> Result<(), AdminApiError> {
    auth.create_or_update_admin_user(admin_create_user_input(member, password)?)
        .map_err(AdminApiError::from_auth)?;
    Ok(())
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
    drop(original_service);
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminMeResponse {
    status: &'static str,
    scope: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OrganizationWithMembers {
    organization: OrganizationDto,
    members: Vec<MembershipDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SetupCompleteResponse {
    setup: AppSetupStatus,
    organization: OrganizationWithMembers,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetupRequest {
    organization_name: String,
    owner_display_name: String,
    owner_email: Option<String>,
    owner_phone: Option<String>,
    owner_password: String,
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
            AppServiceError::AlreadyInitialized | AppServiceError::SingleOrganizationOnly => {
                StatusCode::CONFLICT
            }
            AppServiceError::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
            AppServiceError::LastOwnerDenied
            | AppServiceError::InvalidUserDisplayName
            | AppServiceError::InvalidOrganizationName
            | AppServiceError::SetupIncomplete => StatusCode::BAD_REQUEST,
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
            AuthError::EmailAlreadyRegistered
            | AuthError::PhoneAlreadyRegistered
            | AuthError::InstallationAlreadyBound => StatusCode::CONFLICT,
            AuthError::Storage(_) | AuthError::PasswordHashFailed => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
            AuthError::LoginIdentifierRequired
            | AuthError::DisplayNameRequired
            | AuthError::PasswordRequired
            | AuthError::InstallationIdRequired
            | AuthError::InvalidCredentials
            | AuthError::SessionNotFound => StatusCode::BAD_REQUEST,
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

    fn admin_headers(state: &ServerState) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", state.admin_token)).expect("token header"),
        );
        headers
    }

    fn test_setup_request() -> SetupRequest {
        SetupRequest {
            organization_name: "星河贸易".to_string(),
            owner_display_name: "Owner".to_string(),
            owner_email: Some("owner@example.com".to_string()),
            owner_phone: None,
            owner_password: "owner-password".to_string(),
        }
    }

    async fn setup_test_organization(state: &ServerState) -> Uuid {
        let Json(response) = setup(
            State(state.clone()),
            admin_headers(state),
            Json(test_setup_request()),
        )
        .await
        .expect("setup organization");

        assert!(response.setup.initialized);
        Uuid::parse_str(&response.organization.organization.id).expect("organization uuid")
    }

    #[tokio::test]
    async fn admin_setup_creates_owner_login_identity() {
        let state = test_state();
        let Json(before) = setup_status(State(state.clone()), admin_headers(&state))
            .await
            .expect("setup status before");
        assert!(!before.initialized);
        assert_eq!(before.reason.as_deref(), Some("missing_organization"));

        let organization_id = setup_test_organization(&state).await;
        let Json(after) = setup_status(State(state.clone()), admin_headers(&state))
            .await
            .expect("setup status after");
        assert!(after.initialized);
        assert_eq!(after.organization_count, 1);
        assert_eq!(after.owner_count, 1);

        let mut auth = state.auth_service.lock().expect("auth lock");
        let session = auth
            .login(LoginInput {
                email: Some("owner@example.com".to_string()),
                phone: None,
                password: "owner-password".to_string(),
                installation_id: "phone-owner".to_string(),
            })
            .expect("owner login");

        let service = state.ledger_service.lock().expect("service lock");
        let members = service
            .organization_members(organization_id)
            .expect("organization members");
        assert_eq!(
            Uuid::parse_str(&members.first().expect("owner member").user_id)
                .expect("owner user uuid"),
            session.user.id
        );
    }

    #[tokio::test]
    async fn admin_setup_rejects_repeat_initialization() {
        let state = test_state();
        setup_test_organization(&state).await;

        let error = setup(
            State(state.clone()),
            admin_headers(&state),
            Json(SetupRequest {
                organization_name: "第二组织".to_string(),
                owner_display_name: "Second Owner".to_string(),
                owner_email: Some("second@example.com".to_string()),
                owner_phone: None,
                owner_password: "second-password".to_string(),
            }),
        )
        .await
        .expect_err("repeat setup rejected");

        assert_eq!(error.status, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn admin_add_member_creates_login_identity() {
        let state = test_state();
        let organization_id = setup_test_organization(&state).await;
        let Json(member) = add_member(
            State(state.clone()),
            admin_headers(&state),
            Path(organization_id),
            Json(AddMemberRequest {
                display_name: "Dana".to_string(),
                email: Some("dana@example.com".to_string()),
                phone: None,
                password: Some("initial-password".to_string()),
                role: MembershipRole::Member,
            }),
        )
        .await
        .expect("add member");

        let mut auth = state.auth_service.lock().expect("auth lock");
        let session = auth
            .login(LoginInput {
                email: Some("dana@example.com".to_string()),
                phone: None,
                password: "initial-password".to_string(),
                installation_id: "phone-dana".to_string(),
            })
            .expect("login");

        assert_eq!(
            session.user.id,
            Uuid::parse_str(&member.user_id).expect("member user id")
        );
    }

    #[tokio::test]
    async fn admin_can_reset_member_password() {
        let state = test_state();
        let organization_id = setup_test_organization(&state).await;
        let Json(member) = add_member(
            State(state.clone()),
            admin_headers(&state),
            Path(organization_id),
            Json(AddMemberRequest {
                display_name: "Riley".to_string(),
                email: Some("riley@example.com".to_string()),
                phone: None,
                password: Some("old-password".to_string()),
                role: MembershipRole::Viewer,
            }),
        )
        .await
        .expect("add member");
        let membership_id = Uuid::parse_str(&member.id).expect("membership id");

        let Json(reset_member) = reset_member_password(
            State(state.clone()),
            admin_headers(&state),
            Path((organization_id, membership_id)),
            Json(ResetPasswordRequest {
                password: "new-password".to_string(),
            }),
        )
        .await
        .expect("reset password");
        assert_eq!(reset_member.id, member.id);

        let mut auth = state.auth_service.lock().expect("auth lock");
        assert!(auth
            .login(LoginInput {
                email: Some("riley@example.com".to_string()),
                phone: None,
                password: "old-password".to_string(),
                installation_id: "phone-riley".to_string(),
            })
            .is_err());
        assert!(auth
            .login(LoginInput {
                email: Some("riley@example.com".to_string()),
                phone: None,
                password: "new-password".to_string(),
                installation_id: "phone-riley".to_string(),
            })
            .is_ok());
    }
}
