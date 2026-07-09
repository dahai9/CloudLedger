use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use cloudledger_core::{
    can_perform, Action, ApprovalState, AuditLog, AuthorizationContext, FinancialAccount,
    FinancialAccountKind, Ledger, LedgerKind, Membership, MembershipRole, Money, Organization,
    Transaction, TransactionKind, User,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use uuid::Uuid;

const APP_STATE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Error)]
pub enum AppServiceError {
    #[error("ledger was not found")]
    LedgerNotFound,
    #[error("user was not found")]
    UserNotFound,
    #[error("account was not found")]
    AccountNotFound,
    #[error("actor is not authorized for this action")]
    Unauthorized,
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
pub struct LedgerOverview {
    pub current_user: UserDto,
    pub users: Vec<UserDto>,
    pub ledgers: Vec<LedgerDto>,
    pub accounts: Vec<AccountDto>,
    pub transactions: Vec<TransactionDto>,
    pub audit_logs: Vec<AuditLogDto>,
    pub monthly_income_minor: i64,
    pub monthly_expense_minor: i64,
    pub pending_approval_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserDto {
    pub id: String,
    pub display_name: String,
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
    pub created_by: String,
    pub occurred_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditLogDto {
    pub id: String,
    pub ledger_id: String,
    pub actor_user_id: String,
    pub action: String,
    pub resource_type: String,
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

impl AppLedgerService {
    pub fn load_or_seed(path: impl AsRef<Path>) -> Result<Self, AppServiceError> {
        let path = path.as_ref();
        if !path.exists() {
            let service = Self::seeded();
            service.save_to_path(path)?;
            return Ok(service);
        }

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
        service.schema_version = APP_STATE_SCHEMA_VERSION;
        service.ensure_mvp_seed_data();
        service.save_to_path(path)?;
        Ok(service)
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
            alice_id,
        );
        office.approval_state = ApprovalState::Submitted;
        office.submitted_by = Some(alice_id);

        let audit = AuditLog::new(
            Some(organization.id),
            public_ledger.id,
            alice_id,
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
                Membership::new(organization.id, alice_id, MembershipRole::Owner),
                Membership::new(organization.id, bob_id, MembershipRole::Accountant),
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
            .map(transaction_dto)
            .collect();

        let monthly_income_minor = transactions
            .iter()
            .filter(|transaction| transaction.kind == "income")
            .map(|transaction| transaction.amount_minor)
            .sum();
        let monthly_expense_minor = transactions
            .iter()
            .filter(|transaction| transaction.kind == "expense")
            .map(|transaction| transaction.amount_minor)
            .sum();
        let pending_approval_count = transactions
            .iter()
            .filter(|transaction| transaction.approval_state == "submitted")
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
            users: self.users(),
            ledgers,
            accounts,
            transactions,
            audit_logs: self
                .audit_logs
                .iter()
                .filter(|audit| visible_ledger_ids.contains(&audit.ledger_id))
                .map(audit_dto)
                .collect(),
            monthly_income_minor,
            monthly_expense_minor,
            pending_approval_count,
        }
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
            transaction.approval_state = ApprovalState::Submitted;
            transaction.submitted_by = Some(input.actor_user_id);
            self.audit_logs.push(AuditLog::new(
                ledger.organization_id,
                ledger.id,
                input.actor_user_id,
                "transaction.submitted",
                "transaction",
                transaction.id,
                format!("提交公账流水：{}", transaction.description),
            ));
        } else {
            transaction.approval_state = ApprovalState::Approved;
            transaction.approved_by = Some(input.actor_user_id);
        }

        let dto = transaction_dto(&transaction);
        self.transactions.insert(transaction.id, transaction);
        Ok(dto)
    }

    fn account_dto(&self, account: &FinancialAccount) -> AccountDto {
        let transaction_delta: i64 = self
            .transactions
            .values()
            .filter(|transaction| {
                transaction.account_id == account.id
                    && transaction.deleted_at.is_none()
                    && transaction.approval_state != ApprovalState::Rejected
                    && transaction.approval_state != ApprovalState::Voided
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
                MembershipRole::Accountant => "accountant",
                MembershipRole::Approver => "approver",
                MembershipRole::Member => "member",
                MembershipRole::Viewer => "viewer",
            })
            .unwrap_or("viewer")
            .to_string()
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

    fn ensure_mvp_seed_data(&mut self) {
        let Some(bob_id) = self
            .users
            .values()
            .find(|user| user.display_name == "Bob")
            .map(|user| user.id)
        else {
            return;
        };

        if let Some(organization_id) = self.organizations.keys().next().copied() {
            if let Some(membership) = self.memberships.iter_mut().find(|membership| {
                membership.organization_id == organization_id && membership.user_id == bob_id
            }) {
                membership.role = MembershipRole::Accountant;
            } else {
                self.memberships.push(Membership::new(
                    organization_id,
                    bob_id,
                    MembershipRole::Accountant,
                ));
            }
        }

        let has_bob_private_ledger = self
            .ledgers
            .values()
            .any(|ledger| ledger.owner_user_id == Some(bob_id));
        if has_bob_private_ledger {
            return;
        }

        let bob_ledger = Ledger::personal(bob_id, "Bob 私账");
        let bob_wallet = FinancialAccount::new(
            bob_ledger.id,
            "Bob 钱包",
            FinancialAccountKind::Wallet,
            Money::new(18_5000, "CNY").expect("valid seed money"),
        );
        let mut bob_meal = Transaction::draft(
            bob_ledger.id,
            bob_wallet.id,
            None,
            TransactionKind::Expense,
            Money::new(2_600, "CNY").expect("valid seed money"),
            "咖啡",
            bob_id,
        );
        bob_meal.approval_state = ApprovalState::Approved;

        self.ledgers.insert(bob_ledger.id, bob_ledger);
        self.accounts.insert(bob_wallet.id, bob_wallet);
        self.transactions.insert(bob_meal.id, bob_meal);
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
        Self::seeded()
    }
}

fn user_dto(user: &User) -> UserDto {
    UserDto {
        id: user.id.to_string(),
        display_name: user.display_name.clone(),
    }
}

fn transaction_dto(transaction: &Transaction) -> TransactionDto {
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
        created_by: transaction.created_by.to_string(),
        occurred_at: format_time(transaction.occurred_at),
    }
}

fn audit_dto(audit: &AuditLog) -> AuditLogDto {
    AuditLogDto {
        id: audit.id.to_string(),
        ledger_id: audit.ledger_id.to_string(),
        actor_user_id: audit.actor_user_id.to_string(),
        action: audit.action.clone(),
        resource_type: audit.resource_type.clone(),
        summary: audit.summary.clone(),
        created_at: format_time(audit.created_at),
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
            .any(|ledger| { ledger.kind == "organization_public" && ledger.role == "accountant" }));
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
    fn creating_public_transaction_generates_audit_and_submitted_state() {
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

        assert_eq!(transaction.approval_state, "submitted");
        assert_eq!(
            service.overview(service.current_user_id()).audit_logs.len(),
            2
        );
    }
}
