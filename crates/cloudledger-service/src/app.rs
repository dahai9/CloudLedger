use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use cloudledger_core::{
    can_perform, Action, ApprovalState, AuditLog, AuthorizationContext, FinancialAccount,
    FinancialAccountKind, Ledger, LedgerKind, Membership, MembershipRole, Money, Organization,
    PaymentState, Transaction, TransactionKind, User,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use uuid::Uuid;

const APP_STATE_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Error)]
pub enum AppServiceError {
    #[error("ledger was not found")]
    LedgerNotFound,
    #[error("user was not found")]
    UserNotFound,
    #[error("account was not found")]
    AccountNotFound,
    #[error("organization was not found")]
    OrganizationNotFound,
    #[error("membership was not found")]
    MembershipNotFound,
    #[error("employee account already belongs to another organization")]
    EmployeeBelongsToAnotherOrganization,
    #[error("transaction was not found")]
    TransactionNotFound,
    #[error("actor is not authorized for this action")]
    Unauthorized,
    #[error("transaction is not pending approval")]
    InvalidApprovalState,
    #[error("transaction is not waiting for payment")]
    InvalidPaymentState,
    #[error("only the applicant can confirm receipt")]
    ReceiptConfirmationDenied,
    #[error("submitter cannot approve their own transaction")]
    SelfApprovalDenied,
    #[error("rejection reason is required")]
    DecisionNoteRequired,
    #[error("transaction currency must match account currency")]
    CurrencyMismatch,
    #[error("transfer transactions are not supported in the MVP")]
    UnsupportedTransactionKind,
    #[error("organization must keep at least one owner")]
    LastOwnerDenied,
    #[error("CloudLedger setup is incomplete")]
    SetupIncomplete,
    #[error("organization name is required")]
    InvalidOrganizationName,
    #[error("user display name is required")]
    InvalidUserDisplayName,
    #[error("invalid amount: {0}")]
    InvalidAmount(String),
    #[error("storage error: {0}")]
    Storage(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppCreateTransactionInput {
    #[serde(default)]
    pub actor_user_id: Uuid,
    pub ledger_id: Uuid,
    pub account_id: Uuid,
    pub kind: TransactionKind,
    pub amount_minor: i64,
    pub currency: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppDecideApprovalInput {
    #[serde(default)]
    pub actor_user_id: Uuid,
    pub transaction_id: Uuid,
    pub decision: ApprovalDecision,
    pub decision_note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppMarkTransactionPaidInput {
    #[serde(default)]
    pub actor_user_id: Uuid,
    pub transaction_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfirmTransactionReceiptInput {
    #[serde(default)]
    pub actor_user_id: Uuid,
    pub transaction_id: Uuid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Approve,
    Reject,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerOverview {
    pub current_user: UserDto,
    pub ledgers: Vec<LedgerDto>,
    pub accounts: Vec<AccountDto>,
    pub transactions: Vec<TransactionDto>,
    pub audit_logs: Vec<AuditLogDto>,
    pub monthly_income_minor: i64,
    pub monthly_expense_minor: i64,
    pub pending_approval_count: usize,
    pub pending_payment_count: usize,
    pub pending_receipt_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserDto {
    pub id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganizationDto {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MembershipDto {
    pub id: String,
    pub organization_id: String,
    pub user_id: String,
    pub display_name: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppAddOrganizationMemberInput {
    pub organization_id: Uuid,
    pub display_name: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub role: MembershipRole,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUpdateOrganizationMemberRoleInput {
    pub organization_id: Uuid,
    pub membership_id: Uuid,
    pub role: MembershipRole,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppEnsureUserIdentityInput {
    pub user_id: Uuid,
    pub display_name: String,
    pub email: Option<String>,
    pub phone: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppCreateOrganizationInput {
    pub organization_name: String,
    pub admin_user_id: Uuid,
    pub admin_display_name: String,
    pub admin_email: Option<String>,
    pub admin_phone: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSetupStatus {
    pub initialized: bool,
    pub reason: Option<String>,
    pub organization_count: usize,
    pub owner_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerDto {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub scope_label: String,
    pub organization_id: Option<String>,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountDto {
    pub id: String,
    pub ledger_id: String,
    pub name: String,
    pub kind: String,
    pub balance_minor: i64,
    pub currency: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionDto {
    pub id: String,
    pub ledger_id: String,
    pub account_id: String,
    pub kind: String,
    pub amount_minor: i64,
    pub currency: String,
    pub description: String,
    pub approval_state: String,
    pub payment_state: String,
    pub created_by: String,
    pub created_by_user_id: String,
    pub approved_at: Option<String>,
    pub paid_at: Option<String>,
    pub received_at: Option<String>,
    pub occurred_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditLogDto {
    pub id: String,
    pub ledger_id: String,
    pub actor_user_id: String,
    pub actor_display_name: String,
    pub action: String,
    pub resource_type: String,
    pub resource_id: String,
    pub summary: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppLedgerService {
    #[serde(default = "default_schema_version")]
    schema_version: u32,
    current_user_id: Uuid,
    users: BTreeMap<Uuid, User>,
    organizations: BTreeMap<Uuid, Organization>,
    memberships: Vec<Membership>,
    ledgers: BTreeMap<Uuid, Ledger>,
    accounts: BTreeMap<Uuid, FinancialAccount>,
    transactions: BTreeMap<Uuid, Transaction>,
    audit_logs: Vec<AuditLog>,
}

#[derive(Debug, Clone)]
pub struct AppLedgerSnapshot {
    pub schema_version: u32,
    pub current_user_id: Uuid,
    pub users: Vec<User>,
    pub organizations: Vec<Organization>,
    pub memberships: Vec<Membership>,
    pub ledgers: Vec<Ledger>,
    pub accounts: Vec<FinancialAccount>,
    pub transactions: Vec<Transaction>,
    pub audit_logs: Vec<AuditLog>,
}

impl AppLedgerService {
    pub fn snapshot(&self) -> AppLedgerSnapshot {
        AppLedgerSnapshot {
            schema_version: self.schema_version,
            current_user_id: self.current_user_id,
            users: self.users.values().cloned().collect(),
            organizations: self.organizations.values().cloned().collect(),
            memberships: self.memberships.clone(),
            ledgers: self.ledgers.values().cloned().collect(),
            accounts: self.accounts.values().cloned().collect(),
            transactions: self.transactions.values().cloned().collect(),
            audit_logs: self.audit_logs.clone(),
        }
    }

    pub fn from_snapshot(snapshot: AppLedgerSnapshot) -> Self {
        let mut service = Self {
            schema_version: snapshot.schema_version,
            current_user_id: snapshot.current_user_id,
            users: snapshot
                .users
                .into_iter()
                .map(|user| (user.id, user))
                .collect(),
            organizations: snapshot
                .organizations
                .into_iter()
                .map(|organization| (organization.id, organization))
                .collect(),
            memberships: snapshot.memberships,
            ledgers: snapshot
                .ledgers
                .into_iter()
                .map(|ledger| (ledger.id, ledger))
                .collect(),
            accounts: snapshot
                .accounts
                .into_iter()
                .map(|account| (account.id, account))
                .collect(),
            transactions: snapshot
                .transactions
                .into_iter()
                .map(|transaction| (transaction.id, transaction))
                .collect(),
            audit_logs: snapshot.audit_logs,
        };
        service.migrate_to_current_schema();
        service
    }

    pub fn load_or_seed(path: impl AsRef<Path>) -> Result<Self, AppServiceError> {
        let path = path.as_ref();
        if !path.exists() {
            let service = Self::uninitialized();
            service.save_to_path(path)?;
            return Ok(service);
        }

        let service = Self::load_from_path(path)?;
        service.save_to_path(path)?;
        Ok(service)
    }

    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self, AppServiceError> {
        let path = path.as_ref();

        let document = fs::read_to_string(path)
            .map_err(|err| AppServiceError::Storage(format!("read {}: {err}", path.display())))?;
        let mut service: Self = match serde_json::from_str(&document) {
            Ok(service) => service,
            Err(error) => {
                let backup_path = sidecar_path(path, ".bak");
                if !backup_path.exists() {
                    return Err(AppServiceError::Storage(format!(
                        "parse {}: {error}",
                        path.display()
                    )));
                }

                let backup_document = fs::read_to_string(&backup_path).map_err(|err| {
                    AppServiceError::Storage(format!("read {}: {err}", backup_path.display()))
                })?;
                serde_json::from_str(&backup_document).map_err(|backup_error| {
                    AppServiceError::Storage(format!(
                        "parse {}: {error}; parse backup {}: {backup_error}",
                        path.display(),
                        backup_path.display()
                    ))
                })?
            }
        };
        service.migrate_to_current_schema();
        Ok(service)
    }

    fn migrate_to_current_schema(&mut self) {
        if self.schema_version < 2 {
            for membership in &mut self.memberships {
                membership.role = match membership.role {
                    MembershipRole::Approver => MembershipRole::BusinessOwner,
                    MembershipRole::Accountant
                    | MembershipRole::Member
                    | MembershipRole::Viewer => MembershipRole::Employee,
                    role => role,
                };
            }

            for transaction in self.transactions.values_mut() {
                if transaction.approval_state == ApprovalState::Approved {
                    transaction
                        .approved_at
                        .get_or_insert(transaction.updated_at);
                    let is_public_expense = transaction.kind == TransactionKind::Expense
                        && self
                            .ledgers
                            .get(&transaction.ledger_id)
                            .is_some_and(|ledger| ledger.kind == LedgerKind::OrganizationPublic);
                    if is_public_expense {
                        transaction.payment_state = PaymentState::Received;
                        transaction.received_by = Some(transaction.created_by);
                        transaction.received_at = Some(transaction.updated_at);
                    }
                }
            }
        }
        self.schema_version = APP_STATE_SCHEMA_VERSION;
    }

    pub fn save_to_path(&self, path: impl AsRef<Path>) -> Result<(), AppServiceError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                AppServiceError::Storage(format!("create {}: {err}", parent.display()))
            })?;
        }

        let document = serde_json::to_string_pretty(self).map_err(|err| {
            AppServiceError::Storage(format!("serialize app ledger state: {err}"))
        })?;
        let temporary_path = sidecar_path(path, ".tmp");
        let backup_path = sidecar_path(path, ".bak");

        {
            let mut temporary_file = fs::File::create(&temporary_path).map_err(|err| {
                AppServiceError::Storage(format!("create {}: {err}", temporary_path.display()))
            })?;
            temporary_file
                .write_all(document.as_bytes())
                .map_err(|err| {
                    AppServiceError::Storage(format!("write {}: {err}", temporary_path.display()))
                })?;
            temporary_file.sync_all().map_err(|err| {
                AppServiceError::Storage(format!("sync {}: {err}", temporary_path.display()))
            })?;
        }

        fs::rename(&temporary_path, path).map_err(|err| {
            AppServiceError::Storage(format!(
                "replace {} with {}: {err}",
                path.display(),
                temporary_path.display()
            ))
        })?;

        if let Some(parent) = path.parent() {
            if let Ok(parent_directory) = fs::File::open(parent) {
                let _ = parent_directory.sync_all();
            }
        }

        fs::copy(path, &backup_path).map_err(|err| {
            AppServiceError::Storage(format!("backup {}: {err}", backup_path.display()))
        })?;
        Ok(())
    }

    pub fn seeded() -> Self {
        let alice = User::new("Alice", Some("alice@cloudledger.local".to_string()), None);
        let bob = User::new("Bob", Some("bob@cloudledger.local".to_string()), None);
        let alice_id = alice.id;
        let bob_id = bob.id;
        let organization = Organization::new("Acme Trading", alice_id);
        let personal_ledger = Ledger::personal(alice_id, "Alice 私账");
        let public_ledger = Ledger::organization_public(organization.id, "Acme 公账");
        let bob_personal_ledger = Ledger::personal(bob_id, "Bob 私账");

        let personal_cash = FinancialAccount::new(
            personal_ledger.id,
            "个人现金",
            FinancialAccountKind::Cash,
            Money::new(32_0000, "CNY").expect("valid seed money"),
        );
        let company_bank = FinancialAccount::new(
            public_ledger.id,
            "公司银行账户",
            FinancialAccountKind::Bank,
            Money::new(5_000_000, "CNY").expect("valid seed money"),
        );
        let bob_wallet = FinancialAccount::new(
            bob_personal_ledger.id,
            "Bob 钱包",
            FinancialAccountKind::Wallet,
            Money::new(18_5000, "CNY").expect("valid seed money"),
        );

        let mut salary = Transaction::draft(
            personal_ledger.id,
            personal_cash.id,
            None,
            TransactionKind::Income,
            Money::new(1_800_000, "CNY").expect("valid seed money"),
            "工资收入",
            alice_id,
        );
        salary.approval_state = ApprovalState::Approved;

        let mut bob_meal = Transaction::draft(
            bob_personal_ledger.id,
            bob_wallet.id,
            None,
            TransactionKind::Expense,
            Money::new(2_600, "CNY").expect("valid seed money"),
            "咖啡",
            bob_id,
        );
        bob_meal.approval_state = ApprovalState::Approved;

        let mut office = Transaction::draft(
            public_ledger.id,
            company_bank.id,
            None,
            TransactionKind::Expense,
            Money::new(86_000, "CNY").expect("valid seed money"),
            "办公用品采购",
            bob_id,
        );
        office.approval_state = ApprovalState::Submitted;
        office.submitted_by = Some(bob_id);

        let audit = AuditLog::new(
            Some(organization.id),
            public_ledger.id,
            bob_id,
            "transaction.submitted",
            "transaction",
            office.id,
            "提交公账支出：办公用品采购",
        );

        Self {
            schema_version: APP_STATE_SCHEMA_VERSION,
            current_user_id: alice_id,
            users: BTreeMap::from([(alice_id, alice), (bob_id, bob)]),
            organizations: BTreeMap::from([(organization.id, organization.clone())]),
            memberships: vec![
                Membership::new(organization.id, alice_id, MembershipRole::BusinessOwner),
                Membership::new(organization.id, bob_id, MembershipRole::Employee),
            ],
            ledgers: BTreeMap::from([
                (personal_ledger.id, personal_ledger),
                (bob_personal_ledger.id, bob_personal_ledger),
                (public_ledger.id, public_ledger),
            ]),
            accounts: BTreeMap::from([
                (personal_cash.id, personal_cash),
                (bob_wallet.id, bob_wallet),
                (company_bank.id, company_bank),
            ]),
            transactions: BTreeMap::from([
                (salary.id, salary),
                (bob_meal.id, bob_meal),
                (office.id, office),
            ]),
            audit_logs: vec![audit],
        }
    }

    pub fn uninitialized() -> Self {
        Self {
            schema_version: APP_STATE_SCHEMA_VERSION,
            current_user_id: Uuid::nil(),
            users: BTreeMap::new(),
            organizations: BTreeMap::new(),
            memberships: Vec::new(),
            ledgers: BTreeMap::new(),
            accounts: BTreeMap::new(),
            transactions: BTreeMap::new(),
            audit_logs: Vec::new(),
        }
    }

    pub fn current_user_id(&self) -> Uuid {
        self.current_user_id
    }

    pub fn users(&self) -> Vec<UserDto> {
        let mut users = self.users.values().map(user_dto).collect::<Vec<_>>();
        users.sort_by(|a, b| a.display_name.cmp(&b.display_name));
        users
    }

    pub fn switch_user(&mut self, user_id: Uuid) -> Result<LedgerOverview, AppServiceError> {
        if !self.users.contains_key(&user_id) {
            return Err(AppServiceError::UserNotFound);
        }

        self.current_user_id = user_id;
        Ok(self.overview(user_id))
    }

    pub fn overview(&self, actor_user_id: Uuid) -> LedgerOverview {
        let mut visible_ledgers: Vec<_> = self
            .ledgers
            .values()
            .filter(|ledger| self.authorized(actor_user_id, ledger, Action::ViewLedger))
            .collect();
        visible_ledgers.sort_by(|left, right| {
            ledger_sort_rank(actor_user_id, left)
                .cmp(&ledger_sort_rank(actor_user_id, right))
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.id.cmp(&right.id))
        });

        let ledgers: Vec<_> = visible_ledgers
            .into_iter()
            .map(|ledger| self.ledger_dto(actor_user_id, ledger))
            .collect();

        let visible_ledger_ids: Vec<_> = ledgers
            .iter()
            .filter_map(|ledger| Uuid::parse_str(&ledger.id).ok())
            .collect();

        let accounts = self
            .accounts
            .values()
            .filter(|account| visible_ledger_ids.contains(&account.ledger_id))
            .map(|account| self.account_dto(account))
            .collect();

        let transactions: Vec<_> = self
            .transactions
            .values()
            .filter(|transaction| visible_ledger_ids.contains(&transaction.ledger_id))
            .map(|transaction| self.transaction_dto(transaction))
            .collect();

        let monthly_income_minor = transactions
            .iter()
            .filter(|transaction| transaction.approval_state == "approved")
            .filter(|transaction| transaction.kind == "income")
            .map(|transaction| transaction.amount_minor)
            .sum();
        let monthly_expense_minor = transactions
            .iter()
            .filter(|transaction| transaction.approval_state == "approved")
            .filter(|transaction| transaction.payment_state != "pending_payment")
            .filter(|transaction| transaction.kind == "expense")
            .map(|transaction| transaction.amount_minor)
            .sum();
        let pending_approval_count = transactions
            .iter()
            .filter(|transaction| transaction.approval_state == "submitted")
            .count();
        let pending_payment_count = transactions
            .iter()
            .filter(|transaction| transaction.payment_state == "pending_payment")
            .count();
        let pending_receipt_count = transactions
            .iter()
            .filter(|transaction| transaction.payment_state == "paid_pending_receipt")
            .count();

        LedgerOverview {
            current_user: self
                .users
                .get(&actor_user_id)
                .map(user_dto)
                .unwrap_or(UserDto {
                    id: actor_user_id.to_string(),
                    display_name: "Unknown".to_string(),
                }),
            ledgers,
            accounts,
            transactions,
            audit_logs: self
                .audit_logs
                .iter()
                .filter(|audit| {
                    visible_ledger_ids.contains(&audit.ledger_id)
                        && self.ledgers.get(&audit.ledger_id).is_some_and(|ledger| {
                            self.authorized(actor_user_id, ledger, Action::ViewAuditLog)
                        })
                })
                .map(|audit| self.audit_dto(audit))
                .collect(),
            monthly_income_minor,
            monthly_expense_minor,
            pending_approval_count,
            pending_payment_count,
            pending_receipt_count,
        }
    }

    pub fn organizations(&self) -> Vec<OrganizationDto> {
        let mut organizations = self
            .organizations
            .values()
            .map(|organization| OrganizationDto {
                id: organization.id.to_string(),
                name: organization.name.clone(),
            })
            .collect::<Vec<_>>();
        organizations
            .sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
        organizations
    }

    pub fn setup_status(&self) -> AppSetupStatus {
        let organization_count = self.organizations.len();
        let owner_count = self
            .organizations
            .keys()
            .map(|organization_id| self.owner_count(*organization_id))
            .sum();
        let reason = if organization_count == 0 {
            Some("missing_organization".to_string())
        } else if self
            .organizations
            .keys()
            .any(|organization_id| self.owner_count(*organization_id) == 0)
        {
            Some("missing_owner".to_string())
        } else {
            None
        };

        AppSetupStatus {
            initialized: reason.is_none(),
            reason,
            organization_count,
            owner_count,
        }
    }

    pub fn create_organization(
        &mut self,
        input: AppCreateOrganizationInput,
    ) -> Result<MembershipDto, AppServiceError> {
        let organization_name = input.organization_name.trim();
        if organization_name.is_empty() {
            return Err(AppServiceError::InvalidOrganizationName);
        }
        let admin_display_name = input.admin_display_name.trim();
        if admin_display_name.is_empty() {
            return Err(AppServiceError::InvalidUserDisplayName);
        }
        let admin_email = input.admin_email.and_then(normalize_optional_email);
        let admin_phone = input.admin_phone.and_then(normalize_optional_phone);
        if admin_email.is_none() && admin_phone.is_none() {
            return Err(AppServiceError::SetupIncomplete);
        }

        let admin = User {
            id: input.admin_user_id,
            display_name: admin_display_name.to_string(),
            email: admin_email,
            phone: admin_phone,
            created_at: OffsetDateTime::now_utc(),
        };
        let organization = Organization::new(organization_name, admin.id);
        let public_ledger =
            Ledger::organization_public(organization.id, format!("{} 公账", organization.name));
        let company_bank = FinancialAccount::new(
            public_ledger.id,
            "公司银行账户",
            FinancialAccountKind::Bank,
            Money::new(0, "CNY").expect("zero CNY is valid"),
        );
        let membership = Membership::new(organization.id, admin.id, MembershipRole::Owner);
        let membership_dto = MembershipDto {
            id: membership.id.to_string(),
            organization_id: membership.organization_id.to_string(),
            user_id: membership.user_id.to_string(),
            display_name: admin.display_name.clone(),
            email: admin.email.clone(),
            phone: admin.phone.clone(),
            role: membership_role_name(membership.role).to_string(),
        };

        if self.current_user_id.is_nil() {
            self.current_user_id = admin.id;
        }
        self.users.insert(admin.id, admin);
        self.organizations.insert(organization.id, organization);
        self.memberships.push(membership);
        self.accounts.insert(company_bank.id, company_bank);
        self.ledgers.insert(public_ledger.id, public_ledger);
        Ok(membership_dto)
    }

    pub fn find_user_id_by_email_or_phone(
        &self,
        email: Option<&str>,
        phone: Option<&str>,
    ) -> Option<Uuid> {
        let normalized_email = email.map(normalize_email).filter(|email| !email.is_empty());
        let normalized_phone = phone.map(normalize_phone).filter(|phone| !phone.is_empty());

        self.users
            .values()
            .find(|user| {
                normalized_email.as_deref().is_some_and(|email| {
                    user.email
                        .as_deref()
                        .is_some_and(|existing| normalize_email(existing) == email)
                }) || normalized_phone.as_deref().is_some_and(|phone| {
                    user.phone
                        .as_deref()
                        .is_some_and(|existing| normalize_phone(existing) == phone)
                })
            })
            .map(|user| user.id)
    }

    pub fn ensure_user_identity(
        &mut self,
        input: AppEnsureUserIdentityInput,
    ) -> Result<UserDto, AppServiceError> {
        let display_name = input.display_name.trim();
        if display_name.is_empty() {
            return Err(AppServiceError::InvalidUserDisplayName);
        }

        let email = input.email.and_then(normalize_optional_email);
        let phone = input.phone.and_then(normalize_optional_phone);
        let user_id = self
            .find_user_id_by_email_or_phone(email.as_deref(), phone.as_deref())
            .unwrap_or(input.user_id);

        if let Some(user) = self.users.get_mut(&user_id) {
            user.display_name = display_name.to_string();
            if email.is_some() {
                user.email = email;
            }
            if phone.is_some() {
                user.phone = phone;
            }
        } else {
            self.users.insert(
                user_id,
                User {
                    id: user_id,
                    display_name: display_name.to_string(),
                    email,
                    phone,
                    created_at: OffsetDateTime::now_utc(),
                },
            );
        }

        self.rename_personal_ledger_for_user(user_id, display_name);
        self.ensure_personal_ledger_for_user(user_id, display_name)?;
        self.users
            .get(&user_id)
            .map(user_dto)
            .ok_or(AppServiceError::UserNotFound)
    }

    pub fn organization_members(
        &self,
        organization_id: Uuid,
    ) -> Result<Vec<MembershipDto>, AppServiceError> {
        if !self.organizations.contains_key(&organization_id) {
            return Err(AppServiceError::OrganizationNotFound);
        }

        let mut memberships = self
            .memberships
            .iter()
            .filter(|membership| membership.organization_id == organization_id)
            .map(|membership| self.membership_dto(membership))
            .collect::<Vec<_>>();
        memberships.sort_by(|left, right| {
            left.display_name
                .cmp(&right.display_name)
                .then(left.user_id.cmp(&right.user_id))
        });
        Ok(memberships)
    }

    pub fn add_organization_member(
        &mut self,
        input: AppAddOrganizationMemberInput,
    ) -> Result<MembershipDto, AppServiceError> {
        if !self.organizations.contains_key(&input.organization_id) {
            return Err(AppServiceError::OrganizationNotFound);
        }

        let display_name = input.display_name.trim();
        if display_name.is_empty() {
            return Err(AppServiceError::InvalidUserDisplayName);
        }
        let email = input.email.and_then(normalize_optional_email);
        let phone = input.phone.and_then(normalize_optional_phone);
        let user_id = email
            .as_deref()
            .and_then(|email| self.find_user_id_by_email_or_phone(Some(email), None))
            .or_else(|| {
                phone
                    .as_deref()
                    .and_then(|phone| self.find_user_id_by_email_or_phone(None, Some(phone)))
            })
            .unwrap_or_else(|| {
                let user = User::new(display_name, email.clone(), phone.clone());
                let user_id = user.id;
                self.users.insert(user_id, user);
                user_id
            });

        if self.memberships.iter().any(|membership| {
            membership.user_id == user_id && membership.organization_id != input.organization_id
        }) {
            return Err(AppServiceError::EmployeeBelongsToAnotherOrganization);
        }

        if let Some(user) = self.users.get_mut(&user_id) {
            user.display_name = display_name.to_string();
            if email.is_some() {
                user.email = email.clone();
            }
            if phone.is_some() {
                user.phone = phone.clone();
            }
        }
        self.ensure_personal_ledger_for_user(user_id, display_name)?;

        if let Some(index) = self.memberships.iter().position(|membership| {
            membership.organization_id == input.organization_id && membership.user_id == user_id
        }) {
            if self.memberships[index].role == MembershipRole::Owner
                && input.role != MembershipRole::Owner
                && self.owner_count(input.organization_id) <= 1
            {
                return Err(AppServiceError::LastOwnerDenied);
            }
            self.memberships[index].role = input.role;
            return Ok(self.membership_dto(&self.memberships[index]));
        }

        self.memberships
            .push(Membership::new(input.organization_id, user_id, input.role));
        self.memberships
            .last()
            .map(|membership| self.membership_dto(membership))
            .ok_or(AppServiceError::MembershipNotFound)
    }

    pub fn update_organization_member_role(
        &mut self,
        input: AppUpdateOrganizationMemberRoleInput,
    ) -> Result<MembershipDto, AppServiceError> {
        if !self.organizations.contains_key(&input.organization_id) {
            return Err(AppServiceError::OrganizationNotFound);
        }
        let index = self
            .memberships
            .iter()
            .position(|membership| {
                membership.id == input.membership_id
                    && membership.organization_id == input.organization_id
            })
            .ok_or(AppServiceError::MembershipNotFound)?;
        if self.memberships[index].role == MembershipRole::Owner
            && input.role != MembershipRole::Owner
            && self.owner_count(input.organization_id) <= 1
        {
            return Err(AppServiceError::LastOwnerDenied);
        }

        self.memberships[index].role = input.role;
        Ok(self.membership_dto(&self.memberships[index]))
    }

    pub fn remove_organization_member(
        &mut self,
        organization_id: Uuid,
        membership_id: Uuid,
    ) -> Result<(), AppServiceError> {
        if !self.organizations.contains_key(&organization_id) {
            return Err(AppServiceError::OrganizationNotFound);
        }
        let index = self
            .memberships
            .iter()
            .position(|membership| {
                membership.id == membership_id && membership.organization_id == organization_id
            })
            .ok_or(AppServiceError::MembershipNotFound)?;
        if self.memberships[index].role == MembershipRole::Owner
            && self.owner_count(organization_id) <= 1
        {
            return Err(AppServiceError::LastOwnerDenied);
        }

        self.memberships.remove(index);
        Ok(())
    }

    pub fn create_transaction(
        &mut self,
        input: AppCreateTransactionInput,
    ) -> Result<TransactionDto, AppServiceError> {
        let ledger = self
            .ledgers
            .get(&input.ledger_id)
            .ok_or(AppServiceError::LedgerNotFound)?;

        if !self.authorized(input.actor_user_id, ledger, Action::CreateTransaction) {
            return Err(AppServiceError::Unauthorized);
        }

        let account = self
            .accounts
            .get(&input.account_id)
            .ok_or(AppServiceError::AccountNotFound)?;
        if account.ledger_id != input.ledger_id {
            return Err(AppServiceError::AccountNotFound);
        }
        if input.kind == TransactionKind::Transfer {
            return Err(AppServiceError::UnsupportedTransactionKind);
        }
        if account.opening_balance.currency != input.currency.as_str() {
            return Err(AppServiceError::CurrencyMismatch);
        }

        let amount = Money::new(input.amount_minor, input.currency)
            .map_err(|err| AppServiceError::InvalidAmount(err.to_string()))?;
        let mut transaction = Transaction::draft(
            input.ledger_id,
            input.account_id,
            None,
            input.kind,
            amount,
            input.description,
            input.actor_user_id,
        );

        if ledger.kind == LedgerKind::OrganizationPublic {
            transaction.submitted_by = Some(input.actor_user_id);
            let organization_id = ledger
                .organization_id
                .ok_or(AppServiceError::OrganizationNotFound)?;
            let role = self.membership_role(input.actor_user_id, organization_id);
            let auto_approve = matches!(
                role,
                Some(MembershipRole::BusinessOwner | MembershipRole::Approver)
            ) && self.business_owner_count(organization_id) == 1;

            if auto_approve {
                transaction.approval_state = ApprovalState::Approved;
                transaction.approved_at = Some(OffsetDateTime::now_utc());
                if transaction.kind == TransactionKind::Expense {
                    transaction.payment_state = PaymentState::PendingPayment;
                }
                self.audit_logs.push(AuditLog::new(
                    ledger.organization_id,
                    ledger.id,
                    input.actor_user_id,
                    "transaction.auto_approved",
                    "transaction",
                    transaction.id,
                    format!("单老板策略自动批准公账流水：{}", transaction.description),
                ));
            } else {
                transaction.approval_state = ApprovalState::Submitted;
                self.audit_logs.push(AuditLog::new(
                    ledger.organization_id,
                    ledger.id,
                    input.actor_user_id,
                    "transaction.submitted",
                    "transaction",
                    transaction.id,
                    format!("提交公账流水：{}", transaction.description),
                ));
            }
        } else {
            transaction.approval_state = ApprovalState::Approved;
            transaction.approved_by = Some(input.actor_user_id);
            transaction.approved_at = Some(OffsetDateTime::now_utc());
        }

        let dto = self.transaction_dto(&transaction);
        self.transactions.insert(transaction.id, transaction);
        Ok(dto)
    }

    pub fn decide_approval(
        &mut self,
        input: AppDecideApprovalInput,
    ) -> Result<TransactionDto, AppServiceError> {
        let transaction = self
            .transactions
            .get(&input.transaction_id)
            .ok_or(AppServiceError::TransactionNotFound)?;
        if transaction.approval_state != ApprovalState::Submitted {
            return Err(AppServiceError::InvalidApprovalState);
        }

        let ledger = self
            .ledgers
            .get(&transaction.ledger_id)
            .cloned()
            .ok_or(AppServiceError::LedgerNotFound)?;
        if !self.authorized(input.actor_user_id, &ledger, Action::ApproveTransaction) {
            return Err(AppServiceError::Unauthorized);
        }
        if transaction.submitted_by == Some(input.actor_user_id)
            || transaction.created_by == input.actor_user_id
        {
            return Err(AppServiceError::SelfApprovalDenied);
        }
        let decision_note = normalize_decision_note(input.decision_note);
        if input.decision == ApprovalDecision::Reject && decision_note.is_none() {
            return Err(AppServiceError::DecisionNoteRequired);
        }

        let (updated_transaction, action, summary) = {
            let transaction = self
                .transactions
                .get_mut(&input.transaction_id)
                .ok_or(AppServiceError::TransactionNotFound)?;
            transaction.updated_at = OffsetDateTime::now_utc();
            let (action, summary) = match input.decision {
                ApprovalDecision::Approve => {
                    transaction.approval_state = ApprovalState::Approved;
                    transaction.approved_by = Some(input.actor_user_id);
                    transaction.approved_at = Some(OffsetDateTime::now_utc());
                    if transaction.kind == TransactionKind::Expense {
                        transaction.payment_state = PaymentState::PendingPayment;
                    }
                    (
                        "transaction.approved",
                        format!("批准公账流水，等待打款：{}", transaction.description),
                    )
                }
                ApprovalDecision::Reject => {
                    transaction.approval_state = ApprovalState::Rejected;
                    (
                        "transaction.rejected",
                        format!(
                            "驳回公账流水：{}，原因：{}",
                            transaction.description,
                            decision_note
                                .as_deref()
                                .expect("validated rejection reason")
                        ),
                    )
                }
            };
            transaction.version += 1;
            (transaction.clone(), action, summary)
        };

        self.audit_logs.push(AuditLog::new(
            ledger.organization_id,
            ledger.id,
            input.actor_user_id,
            action,
            "transaction",
            updated_transaction.id,
            summary,
        ));

        Ok(self.transaction_dto(&updated_transaction))
    }

    pub fn mark_transaction_paid(
        &mut self,
        input: AppMarkTransactionPaidInput,
    ) -> Result<TransactionDto, AppServiceError> {
        let transaction = self
            .transactions
            .get(&input.transaction_id)
            .ok_or(AppServiceError::TransactionNotFound)?;
        let ledger = self
            .ledgers
            .get(&transaction.ledger_id)
            .cloned()
            .ok_or(AppServiceError::LedgerNotFound)?;
        if !self.authorized(input.actor_user_id, &ledger, Action::MarkTransactionPaid) {
            return Err(AppServiceError::Unauthorized);
        }
        if transaction.approval_state != ApprovalState::Approved
            || transaction.payment_state != PaymentState::PendingPayment
        {
            return Err(AppServiceError::InvalidPaymentState);
        }

        let now = OffsetDateTime::now_utc();
        let updated = {
            let transaction = self
                .transactions
                .get_mut(&input.transaction_id)
                .ok_or(AppServiceError::TransactionNotFound)?;
            transaction.payment_state = PaymentState::PaidPendingReceipt;
            transaction.paid_by = Some(input.actor_user_id);
            transaction.paid_at = Some(now);
            transaction.updated_at = now;
            transaction.version += 1;
            transaction.clone()
        };
        self.audit_logs.push(AuditLog::new(
            ledger.organization_id,
            ledger.id,
            input.actor_user_id,
            "transaction.paid",
            "transaction",
            updated.id,
            format!("标记公账申请已打款：{}", updated.description),
        ));
        Ok(self.transaction_dto(&updated))
    }

    pub fn confirm_transaction_receipt(
        &mut self,
        input: AppConfirmTransactionReceiptInput,
    ) -> Result<TransactionDto, AppServiceError> {
        let transaction = self
            .transactions
            .get(&input.transaction_id)
            .ok_or(AppServiceError::TransactionNotFound)?;
        let ledger = self
            .ledgers
            .get(&transaction.ledger_id)
            .cloned()
            .ok_or(AppServiceError::LedgerNotFound)?;
        if transaction.created_by != input.actor_user_id {
            return Err(AppServiceError::ReceiptConfirmationDenied);
        }
        if !self.authorized(input.actor_user_id, &ledger, Action::ConfirmReceipt) {
            return Err(AppServiceError::Unauthorized);
        }
        if transaction.approval_state != ApprovalState::Approved
            || transaction.payment_state != PaymentState::PaidPendingReceipt
        {
            return Err(AppServiceError::InvalidPaymentState);
        }

        let now = OffsetDateTime::now_utc();
        let updated = {
            let transaction = self
                .transactions
                .get_mut(&input.transaction_id)
                .ok_or(AppServiceError::TransactionNotFound)?;
            transaction.payment_state = PaymentState::Received;
            transaction.received_by = Some(input.actor_user_id);
            transaction.received_at = Some(now);
            transaction.updated_at = now;
            transaction.version += 1;
            transaction.clone()
        };
        self.audit_logs.push(AuditLog::new(
            ledger.organization_id,
            ledger.id,
            input.actor_user_id,
            "transaction.received",
            "transaction",
            updated.id,
            format!("申请人确认收到款项：{}", updated.description),
        ));
        Ok(self.transaction_dto(&updated))
    }

    fn account_dto(&self, account: &FinancialAccount) -> AccountDto {
        let transaction_delta: i64 = self
            .transactions
            .values()
            .filter(|transaction| {
                transaction.account_id == account.id
                    && transaction.deleted_at.is_none()
                    && transaction_affects_balance(transaction)
            })
            .map(|transaction| match transaction.kind {
                TransactionKind::Income => transaction.amount.amount_minor,
                TransactionKind::Expense => -transaction.amount.amount_minor,
                TransactionKind::Transfer => 0,
            })
            .sum();

        AccountDto {
            id: account.id.to_string(),
            ledger_id: account.ledger_id.to_string(),
            name: account.name.clone(),
            kind: format!("{:?}", account.kind).to_lowercase(),
            balance_minor: account.opening_balance.amount_minor + transaction_delta,
            currency: account.opening_balance.currency.clone(),
        }
    }

    fn ledger_dto(&self, actor_user_id: Uuid, ledger: &Ledger) -> LedgerDto {
        let organization_name = ledger
            .organization_id
            .and_then(|id| self.organizations.get(&id))
            .map(|organization| organization.name.as_str());

        LedgerDto {
            id: ledger.id.to_string(),
            name: ledger.name.clone(),
            kind: match ledger.kind {
                LedgerKind::Personal => "personal".to_string(),
                LedgerKind::OrganizationPublic => "organization_public".to_string(),
            },
            scope_label: match (ledger.kind, organization_name) {
                (LedgerKind::Personal, _) => "私账".to_string(),
                (LedgerKind::OrganizationPublic, Some(name)) => format!("{name} 公账"),
                (LedgerKind::OrganizationPublic, None) => "公账".to_string(),
            },
            organization_id: ledger.organization_id.map(|id| id.to_string()),
            role: self.ledger_role(actor_user_id, ledger),
        }
    }

    fn ledger_role(&self, actor_user_id: Uuid, ledger: &Ledger) -> String {
        if ledger.owner_user_id == Some(actor_user_id) {
            return "owner".to_string();
        }

        ledger
            .organization_id
            .and_then(|organization_id| {
                self.memberships
                    .iter()
                    .find(|membership| {
                        membership.organization_id == organization_id
                            && membership.user_id == actor_user_id
                    })
                    .map(|membership| membership.role)
            })
            .map(|role| match role {
                MembershipRole::Owner => "owner",
                MembershipRole::Admin => "admin",
                MembershipRole::BusinessOwner => "business_owner",
                MembershipRole::Employee => "employee",
                MembershipRole::Accountant => "accountant",
                MembershipRole::Approver => "approver",
                MembershipRole::Member => "member",
                MembershipRole::Viewer => "viewer",
            })
            .unwrap_or("viewer")
            .to_string()
    }

    fn membership_role(
        &self,
        actor_user_id: Uuid,
        organization_id: Uuid,
    ) -> Option<MembershipRole> {
        self.memberships
            .iter()
            .find(|membership| {
                membership.organization_id == organization_id && membership.user_id == actor_user_id
            })
            .map(|membership| membership.role)
    }

    fn business_owner_count(&self, organization_id: Uuid) -> usize {
        self.memberships
            .iter()
            .filter(|membership| {
                membership.organization_id == organization_id
                    && matches!(
                        membership.role,
                        MembershipRole::BusinessOwner | MembershipRole::Approver
                    )
            })
            .count()
    }

    fn authorized(&self, actor_user_id: Uuid, ledger: &Ledger, action: Action) -> bool {
        let membership_role = ledger.organization_id.and_then(|organization_id| {
            self.memberships
                .iter()
                .find(|membership| {
                    membership.organization_id == organization_id
                        && membership.user_id == actor_user_id
                })
                .map(|membership| membership.role)
        });

        can_perform(
            &AuthorizationContext {
                actor_user_id,
                ledger,
                membership_role,
            },
            action,
        )
    }

    fn transaction_dto(&self, transaction: &Transaction) -> TransactionDto {
        TransactionDto {
            id: transaction.id.to_string(),
            ledger_id: transaction.ledger_id.to_string(),
            account_id: transaction.account_id.to_string(),
            kind: match transaction.kind {
                TransactionKind::Income => "income",
                TransactionKind::Expense => "expense",
                TransactionKind::Transfer => "transfer",
            }
            .to_string(),
            amount_minor: transaction.amount.amount_minor,
            currency: transaction.amount.currency.clone(),
            description: transaction.description.clone(),
            approval_state: approval_state_name(transaction.approval_state).to_string(),
            payment_state: payment_state_name(transaction.payment_state).to_string(),
            created_by: self
                .users
                .get(&transaction.created_by)
                .map(|user| user.display_name.clone())
                .unwrap_or_else(|| transaction.created_by.to_string()),
            created_by_user_id: transaction.created_by.to_string(),
            approved_at: transaction.approved_at.map(format_time),
            paid_at: transaction.paid_at.map(format_time),
            received_at: transaction.received_at.map(format_time),
            occurred_at: format_time(transaction.occurred_at),
        }
    }

    fn audit_dto(&self, audit: &AuditLog) -> AuditLogDto {
        AuditLogDto {
            id: audit.id.to_string(),
            ledger_id: audit.ledger_id.to_string(),
            actor_user_id: audit.actor_user_id.to_string(),
            actor_display_name: self
                .users
                .get(&audit.actor_user_id)
                .map(|user| user.display_name.clone())
                .unwrap_or_else(|| audit.actor_user_id.to_string()),
            action: audit.action.clone(),
            resource_type: audit.resource_type.clone(),
            resource_id: audit.resource_id.to_string(),
            summary: audit.summary.clone(),
            created_at: format_time(audit.created_at),
        }
    }

    fn membership_dto(&self, membership: &Membership) -> MembershipDto {
        let user = self.users.get(&membership.user_id);
        MembershipDto {
            id: membership.id.to_string(),
            organization_id: membership.organization_id.to_string(),
            user_id: membership.user_id.to_string(),
            display_name: user
                .map(|user| user.display_name.clone())
                .unwrap_or_else(|| membership.user_id.to_string()),
            email: user.and_then(|user| user.email.clone()),
            phone: user.and_then(|user| user.phone.clone()),
            role: membership_role_name(membership.role).to_string(),
        }
    }

    fn owner_count(&self, organization_id: Uuid) -> usize {
        self.memberships
            .iter()
            .filter(|membership| {
                membership.organization_id == organization_id
                    && membership.role == MembershipRole::Owner
            })
            .count()
    }

    pub fn organization_admin_accounts(&self) -> Vec<(Uuid, Uuid)> {
        self.memberships
            .iter()
            .filter(|membership| {
                matches!(
                    membership.role,
                    MembershipRole::Owner | MembershipRole::Admin
                )
            })
            .map(|membership| (membership.user_id, membership.organization_id))
            .collect()
    }

    fn ensure_personal_ledger_for_user(
        &mut self,
        user_id: Uuid,
        display_name: &str,
    ) -> Result<(), AppServiceError> {
        let has_personal_ledger = self.ledgers.values().any(|ledger| {
            ledger.kind == LedgerKind::Personal && ledger.owner_user_id == Some(user_id)
        });
        if has_personal_ledger {
            return Ok(());
        }

        let ledger = Ledger::personal(user_id, format!("{display_name} 私账"));
        let account = FinancialAccount::new(
            ledger.id,
            "个人现金",
            FinancialAccountKind::Cash,
            Money::new(0, "CNY").expect("zero CNY is valid"),
        );
        self.accounts.insert(account.id, account);
        self.ledgers.insert(ledger.id, ledger);
        Ok(())
    }

    fn rename_personal_ledger_for_user(&mut self, user_id: Uuid, display_name: &str) {
        for ledger in self.ledgers.values_mut().filter(|ledger| {
            ledger.kind == LedgerKind::Personal && ledger.owner_user_id == Some(user_id)
        }) {
            ledger.name = format!("{display_name} 私账");
        }
    }
}

fn default_schema_version() -> u32 {
    APP_STATE_SCHEMA_VERSION
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "ledger-state.json".into());
    path.with_file_name(format!("{file_name}{suffix}"))
}

fn ledger_sort_rank(actor_user_id: Uuid, ledger: &Ledger) -> u8 {
    match ledger.kind {
        LedgerKind::Personal if ledger.owner_user_id == Some(actor_user_id) => 0,
        LedgerKind::Personal => 1,
        LedgerKind::OrganizationPublic => 2,
    }
}

impl Default for AppLedgerService {
    fn default() -> Self {
        Self::uninitialized()
    }
}

fn user_dto(user: &User) -> UserDto {
    UserDto {
        id: user.id.to_string(),
        display_name: user.display_name.clone(),
    }
}

fn normalize_decision_note(note: Option<String>) -> Option<String> {
    note.map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn normalize_optional_email(email: String) -> Option<String> {
    let normalized = normalize_email(&email);
    (!normalized.is_empty()).then_some(normalized)
}

fn normalize_optional_phone(phone: String) -> Option<String> {
    let normalized = normalize_phone(&phone);
    (!normalized.is_empty()).then_some(normalized)
}

fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

fn normalize_phone(phone: &str) -> String {
    phone.trim().to_string()
}

fn membership_role_name(role: MembershipRole) -> &'static str {
    match role {
        MembershipRole::Owner => "owner",
        MembershipRole::Admin => "admin",
        MembershipRole::BusinessOwner => "business_owner",
        MembershipRole::Employee => "employee",
        MembershipRole::Accountant => "accountant",
        MembershipRole::Approver => "approver",
        MembershipRole::Member => "member",
        MembershipRole::Viewer => "viewer",
    }
}

fn payment_state_name(state: PaymentState) -> &'static str {
    match state {
        PaymentState::NotApplicable => "not_applicable",
        PaymentState::PendingPayment => "pending_payment",
        PaymentState::PaidPendingReceipt => "paid_pending_receipt",
        PaymentState::Received => "received",
    }
}

fn transaction_affects_balance(transaction: &Transaction) -> bool {
    transaction.approval_state == ApprovalState::Approved
        && match transaction.kind {
            TransactionKind::Income => true,
            TransactionKind::Expense => transaction.payment_state != PaymentState::PendingPayment,
            TransactionKind::Transfer => false,
        }
}

fn approval_state_name(state: ApprovalState) -> &'static str {
    match state {
        ApprovalState::Draft => "draft",
        ApprovalState::Submitted => "submitted",
        ApprovalState::Approved => "approved",
        ApprovalState::Rejected => "rejected",
        ApprovalState::Voided => "voided",
    }
}

fn format_time(value: OffsetDateTime) -> String {
    value.format(&Rfc3339).unwrap_or_else(|_| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_load_creates_uninitialized_state() {
        let directory =
            std::env::temp_dir().join(format!("cloudledger-service-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("create temp dir");
        let path = directory.join("ledger-state.json");

        let service = AppLedgerService::load_or_seed(&path).expect("load fresh state");
        let status = service.setup_status();

        assert!(!status.initialized);
        assert_eq!(status.reason.as_deref(), Some("missing_organization"));
        assert_eq!(status.organization_count, 0);
        assert_eq!(status.owner_count, 0);
        assert!(path.exists());

        fs::remove_dir_all(directory).expect("remove temp dir");
    }

    #[test]
    fn create_organization_creates_admin_without_personal_ledger() {
        let mut service = AppLedgerService::uninitialized();
        let admin_user_id = Uuid::new_v4();

        let member = service
            .create_organization(AppCreateOrganizationInput {
                organization_name: "星河贸易".to_string(),
                admin_user_id,
                admin_display_name: "Admin".to_string(),
                admin_email: Some("ADMIN@Example.COM ".to_string()),
                admin_phone: None,
            })
            .expect("create organization");
        let status = service.setup_status();
        let overview = service.overview(admin_user_id);

        assert!(status.initialized);
        assert_eq!(status.organization_count, 1);
        assert_eq!(status.owner_count, 1);
        assert_eq!(member.role, "owner");
        assert_eq!(member.email.as_deref(), Some("admin@example.com"));
        assert_eq!(service.organizations().len(), 1);
        assert_eq!(overview.current_user.display_name, "Admin");
        assert!(!overview
            .ledgers
            .iter()
            .any(|ledger| ledger.kind == "personal"));
        assert!(overview.ledgers.is_empty());
        assert!(overview.accounts.is_empty());
    }

    #[test]
    fn create_multiple_organizations_keeps_admins_scoped() {
        let mut service = AppLedgerService::uninitialized();
        let first = service
            .create_organization(AppCreateOrganizationInput {
                organization_name: "星河贸易".to_string(),
                admin_user_id: Uuid::new_v4(),
                admin_display_name: "First Admin".to_string(),
                admin_email: Some("first-admin@example.com".to_string()),
                admin_phone: None,
            })
            .expect("first organization");

        let second = service
            .create_organization(AppCreateOrganizationInput {
                organization_name: "第二组织".to_string(),
                admin_user_id: Uuid::new_v4(),
                admin_display_name: "Second Admin".to_string(),
                admin_email: Some("second-admin@example.com".to_string()),
                admin_phone: None,
            })
            .expect("second organization");

        assert_eq!(service.organizations().len(), 2);
        assert_eq!(service.setup_status().owner_count, 2);
        assert_ne!(first.organization_id, second.organization_id);
        assert_eq!(service.organization_admin_accounts().len(), 2);
    }

    #[test]
    fn loading_multiple_organizations_is_supported() {
        let directory =
            std::env::temp_dir().join(format!("cloudledger-service-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("create temp dir");
        let path = directory.join("ledger-state.json");

        let mut service = AppLedgerService::uninitialized();
        for (name, email) in [
            ("First Organization", "first@example.com"),
            ("Second Organization", "second@example.com"),
        ] {
            service
                .create_organization(AppCreateOrganizationInput {
                    organization_name: name.to_string(),
                    admin_user_id: Uuid::new_v4(),
                    admin_display_name: format!("{name} Admin"),
                    admin_email: Some(email.to_string()),
                    admin_phone: None,
                })
                .expect("create organization");
        }
        service.save_to_path(&path).expect("save state");

        let loaded = AppLedgerService::load_or_seed(&path).expect("load multi organization state");
        assert_eq!(loaded.organizations().len(), 2);

        fs::remove_dir_all(directory).expect("remove temp dir");
    }

    #[test]
    fn seeded_overview_contains_private_and_public_ledgers_for_owner() {
        let service = AppLedgerService::seeded();
        let overview = service.overview(service.current_user_id());

        assert_eq!(overview.ledgers.len(), 2);
        assert_eq!(overview.ledgers[0].name, "Alice 私账");
        assert_eq!(overview.pending_approval_count, 1);
        assert_eq!(overview.audit_logs.len(), 1);
    }

    #[test]
    fn switching_user_keeps_private_ledgers_isolated_and_public_shared() {
        let mut service = AppLedgerService::seeded();
        let bob = service
            .users()
            .into_iter()
            .find(|user| user.display_name == "Bob")
            .expect("seeded bob");
        let overview = service
            .switch_user(Uuid::parse_str(&bob.id).expect("uuid"))
            .expect("switch user");

        assert_eq!(overview.current_user.display_name, "Bob");
        assert_eq!(overview.ledgers.len(), 2);
        assert_eq!(overview.ledgers[0].name, "Bob 私账");
        assert!(overview
            .ledgers
            .iter()
            .any(|ledger| ledger.name == "Bob 私账"));
        assert!(!overview
            .ledgers
            .iter()
            .any(|ledger| ledger.name == "Alice 私账"));
        assert!(overview
            .ledgers
            .iter()
            .any(|ledger| ledger.kind == "organization_public"));
        assert!(overview
            .ledgers
            .iter()
            .any(|ledger| { ledger.kind == "organization_public" && ledger.role == "employee" }));
    }

    #[test]
    fn actor_cannot_write_another_users_private_ledger() {
        let mut service = AppLedgerService::seeded();
        let alice_overview = service.overview(service.current_user_id());
        let alice_private_ledger = alice_overview
            .ledgers
            .iter()
            .find(|ledger| ledger.name == "Alice 私账")
            .expect("alice private ledger");
        let alice_account = alice_overview
            .accounts
            .iter()
            .find(|account| account.ledger_id == alice_private_ledger.id)
            .expect("alice account");
        let bob = service
            .users()
            .into_iter()
            .find(|user| user.display_name == "Bob")
            .expect("seeded bob");

        let result = service.create_transaction(AppCreateTransactionInput {
            actor_user_id: Uuid::parse_str(&bob.id).expect("uuid"),
            ledger_id: Uuid::parse_str(&alice_private_ledger.id).expect("uuid"),
            account_id: Uuid::parse_str(&alice_account.id).expect("uuid"),
            kind: TransactionKind::Expense,
            amount_minor: 1_200,
            currency: "CNY".to_string(),
            description: "越权支出".to_string(),
        });

        assert!(matches!(result, Err(AppServiceError::Unauthorized)));
    }

    #[test]
    fn load_or_seed_recovers_from_backup_when_primary_json_is_corrupt() {
        let directory =
            std::env::temp_dir().join(format!("cloudledger-service-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("create temp dir");
        let path = directory.join("ledger-state.json");

        let service = AppLedgerService::seeded();
        service.save_to_path(&path).expect("save state");
        fs::write(&path, "{ invalid json").expect("corrupt primary state");

        let recovered = AppLedgerService::load_or_seed(&path).expect("recover from backup");
        let overview = recovered.overview(recovered.current_user_id());

        assert_eq!(overview.current_user.display_name, "Alice");
        assert_eq!(overview.ledgers[0].name, "Alice 私账");

        fs::remove_dir_all(directory).expect("remove temp dir");
    }

    #[test]
    fn single_business_owner_entry_is_auto_approved_and_waits_for_payment() {
        let mut service = AppLedgerService::seeded();
        let overview = service.overview(service.current_user_id());
        let public = overview
            .ledgers
            .iter()
            .find(|ledger| ledger.kind == "organization_public")
            .expect("seeded public ledger");
        let account = overview
            .accounts
            .iter()
            .find(|account| account.ledger_id == public.id)
            .expect("seeded public account");

        let transaction = service
            .create_transaction(AppCreateTransactionInput {
                actor_user_id: service.current_user_id(),
                ledger_id: Uuid::parse_str(&public.id).expect("uuid"),
                account_id: Uuid::parse_str(&account.id).expect("uuid"),
                kind: TransactionKind::Expense,
                amount_minor: 12_800,
                currency: "CNY".to_string(),
                description: "快递费".to_string(),
            })
            .expect("create public transaction");

        assert_eq!(transaction.approval_state, "approved");
        assert_eq!(transaction.payment_state, "pending_payment");
        assert_eq!(transaction.created_by, "Alice");
        let overview = service.overview(service.current_user_id());
        assert_eq!(overview.pending_payment_count, 1);
        assert!(overview.audit_logs.iter().any(|audit| {
            audit.action == "transaction.auto_approved"
                && audit.resource_id == transaction.id
                && audit.actor_display_name == "Alice"
        }));
    }

    #[test]
    fn employee_reimbursement_posts_on_payment_then_applicant_confirms_receipt() {
        let mut service = AppLedgerService::seeded();
        let owner_id = service.current_user_id();
        let employee_id = service
            .users()
            .into_iter()
            .find(|user| user.display_name == "Bob")
            .and_then(|user| Uuid::parse_str(&user.id).ok())
            .expect("seeded employee");
        let overview = service.overview(owner_id);
        let public = overview
            .ledgers
            .iter()
            .find(|ledger| ledger.kind == "organization_public")
            .expect("seeded public ledger");
        let public_account = overview
            .accounts
            .iter()
            .find(|account| account.ledger_id == public.id)
            .expect("seeded public account");
        let pending = overview
            .transactions
            .iter()
            .find(|transaction| transaction.approval_state == "submitted")
            .expect("seeded pending transaction");

        assert_eq!(pending.created_by, "Bob");
        assert_eq!(public_account.balance_minor, 5_000_000);

        let approved = service
            .decide_approval(AppDecideApprovalInput {
                actor_user_id: owner_id,
                transaction_id: Uuid::parse_str(&pending.id).expect("uuid"),
                decision: ApprovalDecision::Approve,
                decision_note: None,
            })
            .expect("approve seeded transaction");
        let overview = service.overview(owner_id);
        let public_account = overview
            .accounts
            .iter()
            .find(|account| account.ledger_id == public.id)
            .expect("seeded public account");

        assert_eq!(approved.approval_state, "approved");
        assert_eq!(approved.payment_state, "pending_payment");
        assert_eq!(public_account.balance_minor, 5_000_000);
        assert_eq!(overview.pending_approval_count, 0);
        assert_eq!(overview.pending_payment_count, 1);

        let employee_payment = service.mark_transaction_paid(AppMarkTransactionPaidInput {
            actor_user_id: employee_id,
            transaction_id: Uuid::parse_str(&pending.id).expect("uuid"),
        });
        assert!(matches!(
            employee_payment,
            Err(AppServiceError::Unauthorized)
        ));

        let paid = service
            .mark_transaction_paid(AppMarkTransactionPaidInput {
                actor_user_id: owner_id,
                transaction_id: Uuid::parse_str(&pending.id).expect("uuid"),
            })
            .expect("mark reimbursement paid");
        let overview = service.overview(owner_id);
        let public_account = overview
            .accounts
            .iter()
            .find(|account| account.ledger_id == public.id)
            .expect("seeded public account");
        assert_eq!(paid.payment_state, "paid_pending_receipt");
        assert!(paid.paid_at.is_some());
        assert_eq!(public_account.balance_minor, 4_914_000);
        assert_eq!(overview.pending_payment_count, 0);
        assert_eq!(overview.pending_receipt_count, 1);

        let owner_confirmation =
            service.confirm_transaction_receipt(AppConfirmTransactionReceiptInput {
                actor_user_id: owner_id,
                transaction_id: Uuid::parse_str(&pending.id).expect("uuid"),
            });
        assert!(matches!(
            owner_confirmation,
            Err(AppServiceError::ReceiptConfirmationDenied)
        ));

        let received = service
            .confirm_transaction_receipt(AppConfirmTransactionReceiptInput {
                actor_user_id: employee_id,
                transaction_id: Uuid::parse_str(&pending.id).expect("uuid"),
            })
            .expect("applicant confirms receipt");
        let overview = service.overview(employee_id);
        let public_account = overview
            .accounts
            .iter()
            .find(|account| account.ledger_id == public.id)
            .expect("seeded public account");
        assert_eq!(received.payment_state, "received");
        assert!(received.received_at.is_some());
        assert_eq!(public_account.balance_minor, 4_914_000);
        assert_eq!(overview.pending_receipt_count, 0);
        assert!(overview
            .audit_logs
            .iter()
            .any(|audit| audit.action == "transaction.approved"
                && audit.actor_display_name == "Alice"));
        assert!(overview.audit_logs.iter().any(|audit| {
            audit.action == "transaction.paid" && audit.actor_display_name == "Alice"
        }));
        assert!(overview.audit_logs.iter().any(|audit| {
            audit.action == "transaction.received" && audit.actor_display_name == "Bob"
        }));
    }

    #[test]
    fn rejected_public_transaction_never_posts_to_balance() {
        let mut service = AppLedgerService::seeded();
        let overview = service.overview(service.current_user_id());
        let public = overview
            .ledgers
            .iter()
            .find(|ledger| ledger.kind == "organization_public")
            .expect("seeded public ledger");
        let public_account = overview
            .accounts
            .iter()
            .find(|account| account.ledger_id == public.id)
            .expect("seeded public account");
        let pending = overview
            .transactions
            .iter()
            .find(|transaction| transaction.approval_state == "submitted")
            .expect("seeded pending transaction");

        assert_eq!(public_account.balance_minor, 5_000_000);

        let rejected = service
            .decide_approval(AppDecideApprovalInput {
                actor_user_id: service.current_user_id(),
                transaction_id: Uuid::parse_str(&pending.id).expect("uuid"),
                decision: ApprovalDecision::Reject,
                decision_note: Some("票据不完整".to_string()),
            })
            .expect("reject seeded transaction");
        let overview = service.overview(service.current_user_id());
        let public_account = overview
            .accounts
            .iter()
            .find(|account| account.ledger_id == public.id)
            .expect("seeded public account");

        assert_eq!(rejected.approval_state, "rejected");
        assert_eq!(public_account.balance_minor, 5_000_000);
        assert_eq!(overview.pending_approval_count, 0);
        assert!(overview
            .audit_logs
            .iter()
            .any(|audit| audit.action == "transaction.rejected"
                && audit.actor_display_name == "Alice"
                && audit.summary.contains("票据不完整")));
    }

    #[test]
    fn second_business_owner_can_approve_first_owners_transaction() {
        let mut service = AppLedgerService::seeded();
        let organization_id = service
            .organizations
            .keys()
            .next()
            .copied()
            .expect("seeded organization");
        let second_owner = service
            .add_organization_member(AppAddOrganizationMemberInput {
                organization_id,
                display_name: "Charlie".to_string(),
                email: Some("charlie@example.com".to_string()),
                phone: None,
                role: MembershipRole::BusinessOwner,
            })
            .expect("add second business owner");
        let second_owner_id = Uuid::parse_str(&second_owner.user_id).expect("uuid");
        let alice_overview = service.overview(service.current_user_id());
        let public = alice_overview
            .ledgers
            .iter()
            .find(|ledger| ledger.kind == "organization_public")
            .expect("seeded public ledger");
        let account = alice_overview
            .accounts
            .iter()
            .find(|account| account.ledger_id == public.id)
            .expect("seeded public account");
        let transaction = service
            .create_transaction(AppCreateTransactionInput {
                actor_user_id: service.current_user_id(),
                ledger_id: Uuid::parse_str(&public.id).expect("uuid"),
                account_id: Uuid::parse_str(&account.id).expect("uuid"),
                kind: TransactionKind::Expense,
                amount_minor: 3_300,
                currency: "CNY".to_string(),
                description: "差旅费".to_string(),
            })
            .expect("alice creates public transaction");
        assert_eq!(transaction.approval_state, "submitted");
        let approved = service
            .decide_approval(AppDecideApprovalInput {
                actor_user_id: second_owner_id,
                transaction_id: Uuid::parse_str(&transaction.id).expect("uuid"),
                decision: ApprovalDecision::Approve,
                decision_note: None,
            })
            .expect("second owner approves transaction");

        assert_eq!(approved.approval_state, "approved");
        assert_eq!(approved.payment_state, "pending_payment");
        assert!(service
            .overview(second_owner_id)
            .audit_logs
            .iter()
            .any(|audit| audit.action == "transaction.approved"
                && audit.actor_display_name == "Charlie"));
    }

    #[test]
    fn employee_can_view_public_ledger_and_its_audit_trail() {
        let mut service = AppLedgerService::seeded();
        let bob = service
            .users()
            .into_iter()
            .find(|user| user.display_name == "Bob")
            .expect("seeded bob");
        let bob_id = Uuid::parse_str(&bob.id).expect("uuid");
        for membership in service
            .memberships
            .iter_mut()
            .filter(|membership| membership.user_id == bob_id)
        {
            membership.role = MembershipRole::Employee;
        }

        let overview = service.overview(bob_id);

        assert!(overview
            .ledgers
            .iter()
            .any(|ledger| ledger.kind == "organization_public"));
        assert!(!overview.audit_logs.is_empty());
    }

    #[test]
    fn reject_requires_a_reason() {
        let mut service = AppLedgerService::seeded();
        let overview = service.overview(service.current_user_id());
        let pending = overview
            .transactions
            .iter()
            .find(|transaction| transaction.approval_state == "submitted")
            .expect("seeded pending transaction");

        let result = service.decide_approval(AppDecideApprovalInput {
            actor_user_id: service.current_user_id(),
            transaction_id: Uuid::parse_str(&pending.id).expect("uuid"),
            decision: ApprovalDecision::Reject,
            decision_note: Some(" ".to_string()),
        });

        assert!(matches!(result, Err(AppServiceError::DecisionNoteRequired)));
    }

    #[test]
    fn create_transaction_rejects_unsupported_transfer_and_currency_mismatch() {
        let mut service = AppLedgerService::seeded();
        let overview = service.overview(service.current_user_id());
        let private = overview
            .ledgers
            .iter()
            .find(|ledger| ledger.kind == "personal")
            .expect("seeded private ledger");
        let account = overview
            .accounts
            .iter()
            .find(|account| account.ledger_id == private.id)
            .expect("seeded private account");

        let transfer = service.create_transaction(AppCreateTransactionInput {
            actor_user_id: service.current_user_id(),
            ledger_id: Uuid::parse_str(&private.id).expect("uuid"),
            account_id: Uuid::parse_str(&account.id).expect("uuid"),
            kind: TransactionKind::Transfer,
            amount_minor: 1_000,
            currency: "CNY".to_string(),
            description: "转账".to_string(),
        });
        let currency_mismatch = service.create_transaction(AppCreateTransactionInput {
            actor_user_id: service.current_user_id(),
            ledger_id: Uuid::parse_str(&private.id).expect("uuid"),
            account_id: Uuid::parse_str(&account.id).expect("uuid"),
            kind: TransactionKind::Expense,
            amount_minor: 1_000,
            currency: "USD".to_string(),
            description: "美元支出".to_string(),
        });

        assert!(matches!(
            transfer,
            Err(AppServiceError::UnsupportedTransactionKind)
        ));
        assert!(matches!(
            currency_mismatch,
            Err(AppServiceError::CurrencyMismatch)
        ));
    }

    #[test]
    fn admin_membership_management_keeps_one_technical_owner() {
        let mut service = AppLedgerService::uninitialized();
        let admin_user_id = Uuid::new_v4();
        let admin = service
            .create_organization(AppCreateOrganizationInput {
                organization_name: "Acme Trading".to_string(),
                admin_user_id,
                admin_display_name: "Admin".to_string(),
                admin_email: Some("admin@example.com".to_string()),
                admin_phone: None,
            })
            .expect("create organization");
        let organization_id = Uuid::parse_str(&admin.organization_id).expect("uuid");

        let member = service
            .add_organization_member(AppAddOrganizationMemberInput {
                organization_id,
                display_name: "Charlie".to_string(),
                email: Some(" Charlie@Example.com ".to_string()),
                phone: None,
                role: MembershipRole::Employee,
            })
            .expect("add member");
        assert_eq!(member.display_name, "Charlie");
        assert_eq!(member.email.as_deref(), Some("charlie@example.com"));
        assert_eq!(member.role, "employee");
        let charlie_user_id = Uuid::parse_str(&member.user_id).expect("uuid");
        assert!(service.ledgers.values().any(|ledger| {
            ledger.kind == LedgerKind::Personal
                && ledger.owner_user_id == Some(charlie_user_id)
                && ledger.name == "Charlie 私账"
        }));

        let updated = service
            .update_organization_member_role(AppUpdateOrganizationMemberRoleInput {
                organization_id,
                membership_id: Uuid::parse_str(&member.id).expect("uuid"),
                role: MembershipRole::BusinessOwner,
            })
            .expect("update role");
        assert_eq!(updated.role, "business_owner");

        service
            .remove_organization_member(
                organization_id,
                Uuid::parse_str(&updated.id).expect("uuid"),
            )
            .expect("remove non-owner");

        let owner_membership_id = service
            .memberships
            .iter()
            .find(|membership| {
                membership.organization_id == organization_id
                    && membership.role == MembershipRole::Owner
            })
            .map(|membership| membership.id)
            .expect("seeded owner");
        let remove_owner = service.remove_organization_member(organization_id, owner_membership_id);
        let downgrade_owner =
            service.update_organization_member_role(AppUpdateOrganizationMemberRoleInput {
                organization_id,
                membership_id: owner_membership_id,
                role: MembershipRole::Admin,
            });

        assert!(matches!(
            remove_owner,
            Err(AppServiceError::LastOwnerDenied)
        ));
        assert!(matches!(
            downgrade_owner,
            Err(AppServiceError::LastOwnerDenied)
        ));
    }

    #[test]
    fn login_identity_reuses_admin_created_member_by_email() {
        let mut service = AppLedgerService::seeded();
        let organization_id = service
            .organizations
            .keys()
            .next()
            .copied()
            .expect("seeded organization");
        let member = service
            .add_organization_member(AppAddOrganizationMemberInput {
                organization_id,
                display_name: "Dana".to_string(),
                email: Some("dana@example.com".to_string()),
                phone: None,
                role: MembershipRole::Employee,
            })
            .expect("add member");
        let auth_user_id = Uuid::new_v4();

        let user = service
            .ensure_user_identity(AppEnsureUserIdentityInput {
                user_id: auth_user_id,
                display_name: "Dana Login".to_string(),
                email: Some(" DANA@example.com ".to_string()),
                phone: None,
            })
            .expect("ensure identity");

        assert_eq!(user.id, member.user_id);
        assert_ne!(user.id, auth_user_id.to_string());
        assert!(service
            .overview(Uuid::parse_str(&user.id).expect("uuid"))
            .ledgers
            .iter()
            .any(|ledger| ledger.name == "Acme 公账"));
    }

    #[test]
    fn submitter_cannot_decide_own_public_transaction() {
        let mut service = AppLedgerService::seeded();
        let organization_id = service
            .organizations
            .keys()
            .next()
            .copied()
            .expect("seeded organization");
        service
            .add_organization_member(AppAddOrganizationMemberInput {
                organization_id,
                display_name: "Charlie".to_string(),
                email: Some("charlie@example.com".to_string()),
                phone: None,
                role: MembershipRole::BusinessOwner,
            })
            .expect("add second business owner");
        let overview = service.overview(service.current_user_id());
        let public = overview
            .ledgers
            .iter()
            .find(|ledger| ledger.kind == "organization_public")
            .expect("seeded public ledger");
        let account = overview
            .accounts
            .iter()
            .find(|account| account.ledger_id == public.id)
            .expect("seeded public account");
        let transaction = service
            .create_transaction(AppCreateTransactionInput {
                actor_user_id: service.current_user_id(),
                ledger_id: Uuid::parse_str(&public.id).expect("uuid"),
                account_id: Uuid::parse_str(&account.id).expect("uuid"),
                kind: TransactionKind::Expense,
                amount_minor: 3_300,
                currency: "CNY".to_string(),
                description: "差旅费".to_string(),
            })
            .expect("owner creates public transaction");
        assert_eq!(transaction.approval_state, "submitted");

        let result = service.decide_approval(AppDecideApprovalInput {
            actor_user_id: service.current_user_id(),
            transaction_id: Uuid::parse_str(&transaction.id).expect("uuid"),
            decision: ApprovalDecision::Approve,
            decision_note: None,
        });

        assert!(matches!(result, Err(AppServiceError::SelfApprovalDenied)));
    }
}
