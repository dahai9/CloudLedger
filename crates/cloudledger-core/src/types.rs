use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::Money;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: Uuid,
    pub display_name: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub created_at: OffsetDateTime,
}

impl User {
    pub fn new(
        display_name: impl Into<String>,
        email: Option<String>,
        phone: Option<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            display_name: display_name.into(),
            email,
            phone,
            created_at: OffsetDateTime::now_utc(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Organization {
    pub id: Uuid,
    pub name: String,
    pub created_by: Uuid,
    pub created_at: OffsetDateTime,
}

impl Organization {
    pub fn new(name: impl Into<String>, created_by: Uuid) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            created_by,
            created_at: OffsetDateTime::now_utc(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Membership {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub user_id: Uuid,
    pub role: MembershipRole,
    pub created_at: OffsetDateTime,
}

impl Membership {
    pub fn new(organization_id: Uuid, user_id: Uuid, role: MembershipRole) -> Self {
        Self {
            id: Uuid::new_v4(),
            organization_id,
            user_id,
            role,
            created_at: OffsetDateTime::now_utc(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipRole {
    Owner,
    Admin,
    BusinessOwner,
    Employee,
    Accountant,
    Approver,
    Member,
    Viewer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerKind {
    Personal,
    OrganizationPublic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ledger {
    pub id: Uuid,
    pub name: String,
    pub kind: LedgerKind,
    pub owner_user_id: Option<Uuid>,
    pub organization_id: Option<Uuid>,
    pub created_at: OffsetDateTime,
    pub deleted_at: Option<OffsetDateTime>,
}

impl Ledger {
    pub fn personal(owner_user_id: Uuid, name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            kind: LedgerKind::Personal,
            owner_user_id: Some(owner_user_id),
            organization_id: None,
            created_at: OffsetDateTime::now_utc(),
            deleted_at: None,
        }
    }

    pub fn organization_public(organization_id: Uuid, name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            kind: LedgerKind::OrganizationPublic,
            owner_user_id: None,
            organization_id: Some(organization_id),
            created_at: OffsetDateTime::now_utc(),
            deleted_at: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinancialAccountKind {
    Cash,
    Bank,
    Wallet,
    Wechat,
    Alipay,
    Credit,
    Receivable,
    Payable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinancialAccount {
    pub id: Uuid,
    pub ledger_id: Uuid,
    pub name: String,
    pub kind: FinancialAccountKind,
    pub opening_balance: Money,
    pub created_at: OffsetDateTime,
    pub deleted_at: Option<OffsetDateTime>,
}

impl FinancialAccount {
    pub fn new(
        ledger_id: Uuid,
        name: impl Into<String>,
        kind: FinancialAccountKind,
        opening_balance: Money,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            ledger_id,
            name: name.into(),
            kind,
            opening_balance,
            created_at: OffsetDateTime::now_utc(),
            deleted_at: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CategoryKind {
    Income,
    Expense,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Category {
    pub id: Uuid,
    pub ledger_id: Uuid,
    pub name: String,
    pub kind: CategoryKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionKind {
    Income,
    Expense,
    Transfer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    Draft,
    Submitted,
    Approved,
    Rejected,
    Voided,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentState {
    #[default]
    NotApplicable,
    PendingPayment,
    PaidPendingReceipt,
    Received,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transaction {
    pub id: Uuid,
    pub ledger_id: Uuid,
    pub account_id: Uuid,
    pub category_id: Option<Uuid>,
    pub kind: TransactionKind,
    pub amount: Money,
    pub occurred_at: OffsetDateTime,
    pub description: String,
    pub approval_state: ApprovalState,
    #[serde(default)]
    pub payment_state: PaymentState,
    pub created_by: Uuid,
    pub submitted_by: Option<Uuid>,
    pub approved_by: Option<Uuid>,
    #[serde(default)]
    pub approved_at: Option<OffsetDateTime>,
    #[serde(default)]
    pub paid_by: Option<Uuid>,
    #[serde(default)]
    pub paid_at: Option<OffsetDateTime>,
    #[serde(default)]
    pub received_by: Option<Uuid>,
    #[serde(default)]
    pub received_at: Option<OffsetDateTime>,
    pub version: i64,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub deleted_at: Option<OffsetDateTime>,
}

impl Transaction {
    pub fn draft(
        ledger_id: Uuid,
        account_id: Uuid,
        category_id: Option<Uuid>,
        kind: TransactionKind,
        amount: Money,
        description: impl Into<String>,
        created_by: Uuid,
    ) -> Self {
        let now = OffsetDateTime::now_utc();
        Self {
            id: Uuid::new_v4(),
            ledger_id,
            account_id,
            category_id,
            kind,
            amount,
            occurred_at: now,
            description: description.into(),
            approval_state: ApprovalState::Draft,
            payment_state: PaymentState::NotApplicable,
            created_by,
            submitted_by: None,
            approved_by: None,
            approved_at: None,
            paid_by: None,
            paid_at: None,
            received_by: None,
            received_at: None,
            version: 1,
            created_at: now,
            updated_at: now,
            deleted_at: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditLog {
    pub id: Uuid,
    pub organization_id: Option<Uuid>,
    pub ledger_id: Uuid,
    pub actor_user_id: Uuid,
    pub action: String,
    pub resource_type: String,
    pub resource_id: Uuid,
    pub summary: String,
    pub created_at: OffsetDateTime,
}

impl AuditLog {
    pub fn new(
        organization_id: Option<Uuid>,
        ledger_id: Uuid,
        actor_user_id: Uuid,
        action: impl Into<String>,
        resource_type: impl Into<String>,
        resource_id: Uuid,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            organization_id,
            ledger_id,
            actor_user_id,
            action: action.into(),
            resource_type: resource_type.into(),
            resource_id,
            summary: summary.into(),
            created_at: OffsetDateTime::now_utc(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncEvent {
    pub id: Uuid,
    pub ledger_id: Uuid,
    pub resource_type: String,
    pub resource_id: Uuid,
    pub version: i64,
    pub payload_json: String,
    pub created_at: OffsetDateTime,
}
