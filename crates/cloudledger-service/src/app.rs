use std::collections::BTreeMap;

use cloudledger_core::{
    can_perform, Action, ApprovalState, AuditLog, AuthorizationContext, FinancialAccount,
    FinancialAccountKind, Ledger, LedgerKind, Membership, MembershipRole, Money, Organization,
    Transaction, TransactionKind, User,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum AppServiceError {
    #[error("ledger was not found")]
    LedgerNotFound,
    #[error("account was not found")]
    AccountNotFound,
    #[error("actor is not authorized for this action")]
    Unauthorized,
    #[error("invalid amount: {0}")]
    InvalidAmount(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppCreateTransactionInput {
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

#[derive(Debug, Clone)]
pub struct AppLedgerService {
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
    pub fn seeded() -> Self {
        let alice = User::new("Alice", Some("alice@cloudledger.local".to_string()), None);
        let bob = User::new("Bob", Some("bob@cloudledger.local".to_string()), None);
        let alice_id = alice.id;
        let bob_id = bob.id;
        let organization = Organization::new("Acme Trading", alice_id);
        let personal_ledger = Ledger::personal(alice_id, "Alice 私账");
        let public_ledger = Ledger::organization_public(organization.id, "Acme 公账");

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
            current_user_id: alice_id,
            users: BTreeMap::from([(alice_id, alice), (bob_id, bob)]),
            organizations: BTreeMap::from([(organization.id, organization.clone())]),
            memberships: vec![
                Membership::new(organization.id, alice_id, MembershipRole::Owner),
                Membership::new(organization.id, bob_id, MembershipRole::Viewer),
            ],
            ledgers: BTreeMap::from([
                (personal_ledger.id, personal_ledger),
                (public_ledger.id, public_ledger),
            ]),
            accounts: BTreeMap::from([
                (personal_cash.id, personal_cash),
                (company_bank.id, company_bank),
            ]),
            transactions: BTreeMap::from([(salary.id, salary), (office.id, office)]),
            audit_logs: vec![audit],
        }
    }

    pub fn current_user_id(&self) -> Uuid {
        self.current_user_id
    }

    pub fn overview(&self, actor_user_id: Uuid) -> LedgerOverview {
        let ledgers: Vec<_> = self
            .ledgers
            .values()
            .filter(|ledger| self.authorized(actor_user_id, ledger, Action::ViewLedger))
            .map(|ledger| self.ledger_dto(ledger))
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

    fn ledger_dto(&self, ledger: &Ledger) -> LedgerDto {
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
        }
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
        assert_eq!(overview.pending_approval_count, 1);
        assert_eq!(overview.audit_logs.len(), 1);
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
