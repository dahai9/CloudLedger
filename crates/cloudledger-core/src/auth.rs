use crate::book::{Ledger, LedgerKind};
use crate::ids::{LedgerId, UserId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum LedgerRole {
    Owner,
    Bookkeeper,
    Approver,
    Auditor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LedgerMember {
    pub ledger_id: LedgerId,
    pub user_id: UserId,
    pub role: LedgerRole,
}

impl LedgerMember {
    pub fn new(ledger_id: LedgerId, user_id: UserId, role: LedgerRole) -> Self {
        Self {
            ledger_id,
            user_id,
            role,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerAction {
    ViewLedger,
    CreateDraft,
    SubmitEntry,
    ApproveEntry,
    PostWithoutApproval,
    ManageMembers,
}

pub fn roles_for(
    ledger: &Ledger,
    memberships: &[LedgerMember],
    user_id: &UserId,
) -> Vec<LedgerRole> {
    let mut roles = Vec::new();

    if let LedgerKind::PersonalPrivate { owner_id } = &ledger.kind {
        if owner_id == user_id {
            roles.push(LedgerRole::Owner);
        }
    }

    for member in memberships {
        if member.ledger_id == ledger.id
            && &member.user_id == user_id
            && !roles.contains(&member.role)
        {
            roles.push(member.role);
        }
    }

    roles.sort();
    roles
}

pub fn has_permission(
    ledger: &Ledger,
    memberships: &[LedgerMember],
    user_id: &UserId,
    action: LedgerAction,
) -> bool {
    roles_for(ledger, memberships, user_id)
        .into_iter()
        .any(|role| role_allows(role, action))
}

fn role_allows(role: LedgerRole, action: LedgerAction) -> bool {
    match role {
        LedgerRole::Owner => true,
        LedgerRole::Bookkeeper => matches!(
            action,
            LedgerAction::ViewLedger | LedgerAction::CreateDraft | LedgerAction::SubmitEntry
        ),
        LedgerRole::Approver => {
            matches!(
                action,
                LedgerAction::ViewLedger | LedgerAction::ApproveEntry
            )
        }
        LedgerRole::Auditor => matches!(action, LedgerAction::ViewLedger),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{amount::Money, CompanyId, CurrencyCode};

    #[test]
    fn personal_owner_has_private_ledger_permissions() {
        let owner = UserId::from("owner");
        let ledger = Ledger::personal(
            LedgerId::from("personal"),
            owner.clone(),
            "Personal",
            CurrencyCode::new("CNY"),
        );

        assert!(has_permission(
            &ledger,
            &[],
            &owner,
            LedgerAction::PostWithoutApproval
        ));
        assert!(!has_permission(
            &ledger,
            &[],
            &UserId::from("other"),
            LedgerAction::ViewLedger
        ));
    }

    #[test]
    fn company_auditor_is_read_only() {
        let auditor = UserId::from("auditor");
        let ledger = Ledger::company(
            LedgerId::from("company"),
            CompanyId::from("acme"),
            "ACME",
            CurrencyCode::new("CNY"),
            Money::new(CurrencyCode::new("CNY"), 10_000),
        );
        let members = vec![LedgerMember::new(
            ledger.id.clone(),
            auditor.clone(),
            LedgerRole::Auditor,
        )];

        assert!(has_permission(
            &ledger,
            &members,
            &auditor,
            LedgerAction::ViewLedger
        ));
        assert!(!has_permission(
            &ledger,
            &members,
            &auditor,
            LedgerAction::SubmitEntry
        ));
    }
}
