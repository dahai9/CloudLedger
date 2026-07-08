use crate::auth::LedgerRole;
use crate::book::{Ledger, LedgerKind};
use crate::journal::JournalEntry;
use crate::{ApprovalId, JournalEntryId, LedgerId, UserId};

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ApprovalRequirement {
    NotRequired,
    Required(ApprovalRequirementDetail),
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ApprovalRequirementDetail {
    pub min_approvals: u8,
    pub required_roles: Vec<LedgerRole>,
    pub reason: ApprovalReason,
}

impl ApprovalRequirementDetail {
    pub fn accepts_any_role(&self, roles: &[LedgerRole]) -> bool {
        roles
            .iter()
            .any(|role| self.required_roles.iter().any(|required| required == role))
    }

    pub fn accepted_role(&self, roles: &[LedgerRole]) -> Option<LedgerRole> {
        self.required_roles
            .iter()
            .copied()
            .find(|required| roles.contains(required))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ApprovalReason {
    CompanyLedger,
    HighValueCompanyEntry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ApprovalVote {
    Approve,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Rejected,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ApprovalDecision {
    pub approver_id: UserId,
    pub role: LedgerRole,
    pub vote: ApprovalVote,
    pub note: Option<String>,
}

impl ApprovalDecision {
    pub fn new(
        approver_id: UserId,
        role: LedgerRole,
        vote: ApprovalVote,
        note: Option<String>,
    ) -> Self {
        Self {
            approver_id,
            role,
            vote,
            note,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ApprovalRequest {
    pub id: ApprovalId,
    pub ledger_id: LedgerId,
    pub entry_id: JournalEntryId,
    pub submitted_by: UserId,
    pub requirement: ApprovalRequirementDetail,
    pub status: ApprovalStatus,
    pub decisions: Vec<ApprovalDecision>,
}

impl ApprovalRequest {
    pub fn new(
        id: ApprovalId,
        ledger_id: LedgerId,
        entry_id: JournalEntryId,
        submitted_by: UserId,
        requirement: ApprovalRequirementDetail,
    ) -> Self {
        Self {
            id,
            ledger_id,
            entry_id,
            submitted_by,
            requirement,
            status: ApprovalStatus::Pending,
            decisions: Vec::new(),
        }
    }

    pub fn add_decision(
        &mut self,
        decision: ApprovalDecision,
    ) -> Result<(), ApprovalDecisionError> {
        if self.status != ApprovalStatus::Pending {
            return Err(ApprovalDecisionError::AlreadyClosed);
        }

        if self
            .decisions
            .iter()
            .any(|existing| existing.approver_id == decision.approver_id)
        {
            return Err(ApprovalDecisionError::DuplicateApprover);
        }

        if !self.requirement.required_roles.contains(&decision.role) {
            return Err(ApprovalDecisionError::RoleNotAccepted);
        }

        self.decisions.push(decision);
        if self
            .decisions
            .iter()
            .any(|decision| decision.vote == ApprovalVote::Reject)
        {
            self.status = ApprovalStatus::Rejected;
        } else if self.approval_count() >= usize::from(self.requirement.min_approvals) {
            self.status = ApprovalStatus::Approved;
        }

        Ok(())
    }

    pub fn approval_count(&self) -> usize {
        self.decisions
            .iter()
            .filter(|decision| decision.vote == ApprovalVote::Approve)
            .count()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalDecisionError {
    AlreadyClosed,
    DuplicateApprover,
    RoleNotAccepted,
}

impl std::fmt::Display for ApprovalDecisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApprovalDecisionError::AlreadyClosed => {
                f.write_str("approval request is already closed")
            }
            ApprovalDecisionError::DuplicateApprover => {
                f.write_str("approver has already decided this request")
            }
            ApprovalDecisionError::RoleNotAccepted => {
                f.write_str("approver role does not satisfy the approval requirement")
            }
        }
    }
}

impl std::error::Error for ApprovalDecisionError {}

pub fn approval_requirement(
    ledger: &Ledger,
    actor_roles: &[LedgerRole],
    entry: &JournalEntry,
) -> ApprovalRequirement {
    match &ledger.kind {
        LedgerKind::PersonalPrivate { .. } => ApprovalRequirement::NotRequired,
        LedgerKind::CompanyPublic { .. } => {
            let high_value = entry
                .largest_line_amount()
                .map(|amount| {
                    amount.currency == ledger.approval_policy.high_value_threshold.currency
                        && amount.minor_units
                            >= ledger.approval_policy.high_value_threshold.minor_units
                })
                .unwrap_or(false);

            if high_value {
                return ApprovalRequirement::Required(ApprovalRequirementDetail {
                    min_approvals: ledger.approval_policy.min_approvals.max(1),
                    required_roles: vec![LedgerRole::Owner],
                    reason: ApprovalReason::HighValueCompanyEntry,
                });
            }

            if ledger.approval_policy.company_entries_require_approval
                && !actor_roles.contains(&LedgerRole::Owner)
            {
                return ApprovalRequirement::Required(ApprovalRequirementDetail {
                    min_approvals: ledger.approval_policy.min_approvals.max(1),
                    required_roles: vec![LedgerRole::Approver, LedgerRole::Owner],
                    reason: ApprovalReason::CompanyLedger,
                });
            }

            ApprovalRequirement::NotRequired
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        amount::Money, AccountId, CompanyId, CurrencyCode, JournalLine, LedgerId, PostingSide,
        UserId,
    };

    fn company_ledger() -> Ledger {
        Ledger::company(
            LedgerId::from("company"),
            CompanyId::from("acme"),
            "ACME",
            CurrencyCode::new("CNY"),
            Money::new(CurrencyCode::new("CNY"), 10_000),
        )
    }

    fn entry(amount: i64) -> JournalEntry {
        JournalEntry::new(
            JournalEntryId::from("entry"),
            LedgerId::from("company"),
            UserId::from("bookkeeper"),
            "Expense",
            vec![
                JournalLine::new(
                    AccountId::from("expense"),
                    PostingSide::Debit,
                    Money::new(CurrencyCode::new("CNY"), amount),
                    None,
                ),
                JournalLine::new(
                    AccountId::from("cash"),
                    PostingSide::Credit,
                    Money::new(CurrencyCode::new("CNY"), amount),
                    None,
                ),
            ],
        )
    }

    #[test]
    fn company_bookkeeper_entry_requires_approver_or_owner() {
        let requirement =
            approval_requirement(&company_ledger(), &[LedgerRole::Bookkeeper], &entry(2_000));

        assert_eq!(
            requirement,
            ApprovalRequirement::Required(ApprovalRequirementDetail {
                min_approvals: 1,
                required_roles: vec![LedgerRole::Approver, LedgerRole::Owner],
                reason: ApprovalReason::CompanyLedger,
            })
        );
    }

    #[test]
    fn high_value_company_entry_requires_owner() {
        let requirement =
            approval_requirement(&company_ledger(), &[LedgerRole::Bookkeeper], &entry(2_0000));

        assert_eq!(
            requirement,
            ApprovalRequirement::Required(ApprovalRequirementDetail {
                min_approvals: 1,
                required_roles: vec![LedgerRole::Owner],
                reason: ApprovalReason::HighValueCompanyEntry,
            })
        );
    }

    #[test]
    fn owner_normal_company_entry_can_skip_approval() {
        let requirement =
            approval_requirement(&company_ledger(), &[LedgerRole::Owner], &entry(2_000));

        assert_eq!(requirement, ApprovalRequirement::NotRequired);
    }
}
