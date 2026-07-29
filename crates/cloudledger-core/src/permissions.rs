use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{Ledger, LedgerKind, MembershipRole};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    ViewLedger,
    CreateTransaction,
    SubmitTransaction,
    ApproveTransaction,
    MarkTransactionPaid,
    ConfirmReceipt,
    EditApprovedTransaction,
    VoidTransaction,
    ExportLedger,
    ManageAccounts,
    ManageMembers,
    ViewAuditLog,
    ViewFinancialAnalytics,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationContext<'a> {
    pub actor_user_id: Uuid,
    pub ledger: &'a Ledger,
    pub membership_role: Option<MembershipRole>,
}

pub fn can_perform(ctx: &AuthorizationContext<'_>, action: Action) -> bool {
    match ctx.ledger.kind {
        LedgerKind::Personal => can_perform_personal(ctx, action),
        LedgerKind::OrganizationPublic => can_perform_organization(ctx, action),
    }
}

fn can_perform_personal(ctx: &AuthorizationContext<'_>, action: Action) -> bool {
    let is_owner = ctx.ledger.owner_user_id == Some(ctx.actor_user_id);
    if !is_owner {
        return false;
    }

    matches!(
        action,
        Action::ViewLedger
            | Action::CreateTransaction
            | Action::SubmitTransaction
            | Action::VoidTransaction
            | Action::ExportLedger
            | Action::ManageAccounts
            | Action::ViewAuditLog
    )
}

fn can_perform_organization(ctx: &AuthorizationContext<'_>, action: Action) -> bool {
    let Some(role) = ctx.membership_role else {
        return false;
    };

    match role {
        MembershipRole::Owner | MembershipRole::Admin => false,
        MembershipRole::BusinessOwner | MembershipRole::Approver => matches!(
            action,
            Action::ViewLedger
                | Action::CreateTransaction
                | Action::SubmitTransaction
                | Action::ApproveTransaction
                | Action::MarkTransactionPaid
                | Action::ConfirmReceipt
                | Action::VoidTransaction
                | Action::ExportLedger
                | Action::ManageAccounts
                | Action::ViewAuditLog
                | Action::ViewFinancialAnalytics
        ),
        MembershipRole::Employee | MembershipRole::Accountant | MembershipRole::Member => matches!(
            action,
            Action::ViewLedger
                | Action::CreateTransaction
                | Action::SubmitTransaction
                | Action::ConfirmReceipt
                | Action::ViewAuditLog
        ),
        MembershipRole::Viewer => matches!(action, Action::ViewLedger | Action::ViewAuditLog),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Ledger;

    #[test]
    fn organization_admin_cannot_read_personal_ledger() {
        let owner = Uuid::new_v4();
        let admin = Uuid::new_v4();
        let ledger = Ledger::personal(owner, "Private cash");

        let ctx = AuthorizationContext {
            actor_user_id: admin,
            ledger: &ledger,
            membership_role: Some(MembershipRole::Owner),
        };

        assert!(!can_perform(&ctx, Action::ViewLedger));
    }

    #[test]
    fn business_owner_can_manage_public_ledger() {
        let owner = Uuid::new_v4();
        let org = Uuid::new_v4();
        let ledger = Ledger::organization_public(org, "Company books");

        let ctx = AuthorizationContext {
            actor_user_id: owner,
            ledger: &ledger,
            membership_role: Some(MembershipRole::BusinessOwner),
        };

        assert!(can_perform(&ctx, Action::ApproveTransaction));
        assert!(can_perform(&ctx, Action::MarkTransactionPaid));
        assert!(can_perform(&ctx, Action::ViewFinancialAnalytics));
        assert!(!can_perform(&ctx, Action::ManageMembers));
    }

    #[test]
    fn viewer_is_read_only() {
        let viewer = Uuid::new_v4();
        let org = Uuid::new_v4();
        let ledger = Ledger::organization_public(org, "Company books");

        let ctx = AuthorizationContext {
            actor_user_id: viewer,
            ledger: &ledger,
            membership_role: Some(MembershipRole::Viewer),
        };

        assert!(can_perform(&ctx, Action::ViewLedger));
        assert!(!can_perform(&ctx, Action::CreateTransaction));
        assert!(!can_perform(&ctx, Action::ApproveTransaction));
        assert!(!can_perform(&ctx, Action::ViewFinancialAnalytics));
    }
}
