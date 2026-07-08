use cloudledger_core::{
    approval_requirement, has_permission, roles_for, ApprovalDecision, ApprovalId, ApprovalRequest,
    ApprovalRequirement, ApprovalStatus, ApprovalVote, JournalEntry, JournalStatus, LedgerAction,
    UserId,
};
use cloudledger_db::LedgerRepository;

use crate::ServiceError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmissionOutcome {
    Posted(JournalEntry),
    PendingApproval(ApprovalRequest),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalOutcome {
    pub request: ApprovalRequest,
    pub entry: JournalEntry,
}

pub struct LedgerService<R> {
    repository: R,
}

impl<R> LedgerService<R>
where
    R: LedgerRepository,
{
    pub fn new(repository: R) -> Self {
        Self { repository }
    }

    pub fn repository(&self) -> &R {
        &self.repository
    }

    pub fn submit_entry(
        &self,
        actor: &UserId,
        mut entry: JournalEntry,
    ) -> Result<SubmissionOutcome, ServiceError> {
        if &entry.created_by != actor {
            return Err(ServiceError::ActorMismatch);
        }

        let ledger = self
            .repository
            .get_ledger(&entry.ledger_id)?
            .ok_or(ServiceError::LedgerNotFound)?;
        let members = self.repository.list_members(&ledger.id)?;

        if !has_permission(&ledger, &members, actor, LedgerAction::SubmitEntry) {
            return Err(ServiceError::PermissionDenied);
        }

        entry.validate()?;
        let actor_roles = roles_for(&ledger, &members, actor);
        match approval_requirement(&ledger, &actor_roles, &entry) {
            ApprovalRequirement::Required(requirement) => {
                entry.status = JournalStatus::PendingApproval;
                self.repository.save_journal_entry(entry.clone())?;

                let request = ApprovalRequest::new(
                    ApprovalId::new(format!("approval:{}", entry.id.as_str())),
                    entry.ledger_id.clone(),
                    entry.id.clone(),
                    actor.clone(),
                    requirement,
                );
                self.repository.save_approval_request(request.clone())?;
                Ok(SubmissionOutcome::PendingApproval(request))
            }
            ApprovalRequirement::NotRequired => {
                if !has_permission(&ledger, &members, actor, LedgerAction::PostWithoutApproval) {
                    return Err(ServiceError::PermissionDenied);
                }

                entry.status = JournalStatus::Posted;
                self.repository.save_journal_entry(entry.clone())?;
                Ok(SubmissionOutcome::Posted(entry))
            }
        }
    }

    pub fn decide_approval(
        &self,
        actor: &UserId,
        approval_id: &ApprovalId,
        vote: ApprovalVote,
        note: Option<String>,
    ) -> Result<ApprovalOutcome, ServiceError> {
        let mut request = self
            .repository
            .get_approval_request(approval_id)?
            .ok_or(ServiceError::ApprovalRequestNotFound)?;

        let mut entry = self
            .repository
            .get_journal_entry(&request.entry_id)?
            .ok_or(ServiceError::JournalEntryNotFound)?;

        let ledger = self
            .repository
            .get_ledger(&request.ledger_id)?
            .ok_or(ServiceError::LedgerNotFound)?;
        let members = self.repository.list_members(&ledger.id)?;

        if !has_permission(&ledger, &members, actor, LedgerAction::ApproveEntry) {
            return Err(ServiceError::PermissionDenied);
        }

        if &request.submitted_by == actor {
            return Err(ServiceError::SelfApprovalDenied);
        }

        let actor_roles = roles_for(&ledger, &members, actor);
        let role = request
            .requirement
            .accepted_role(&actor_roles)
            .ok_or(ServiceError::RoleNotAccepted)?;

        request.add_decision(ApprovalDecision::new(actor.clone(), role, vote, note))?;

        match request.status {
            ApprovalStatus::Approved => entry.status = JournalStatus::Posted,
            ApprovalStatus::Rejected => entry.status = JournalStatus::Rejected,
            ApprovalStatus::Pending | ApprovalStatus::Cancelled => {}
        }

        self.repository.save_journal_entry(entry.clone())?;
        self.repository.save_approval_request(request.clone())?;

        Ok(ApprovalOutcome { request, entry })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cloudledger_core::{
        amount::Money, book::Ledger, AccountId, CompanyId, CurrencyCode, JournalEntryId,
        JournalLine, LedgerId, LedgerMember, LedgerRole, PostingSide,
    };
    use cloudledger_db::MemoryLedgerRepository;

    fn cny(minor_units: i64) -> Money {
        Money::new(CurrencyCode::new("CNY"), minor_units)
    }

    fn personal_ledger(owner: &UserId) -> Ledger {
        Ledger::personal(
            LedgerId::from("personal"),
            owner.clone(),
            "Personal",
            CurrencyCode::new("CNY"),
        )
    }

    fn company_ledger() -> Ledger {
        Ledger::company(
            LedgerId::from("company"),
            CompanyId::from("acme"),
            "ACME",
            CurrencyCode::new("CNY"),
            cny(10_000),
        )
    }

    fn balanced_entry(id: &str, ledger_id: &LedgerId, actor: &UserId, amount: i64) -> JournalEntry {
        JournalEntry::new(
            JournalEntryId::from(id),
            ledger_id.clone(),
            actor.clone(),
            "Expense",
            vec![
                JournalLine::new(
                    AccountId::from("expense"),
                    PostingSide::Debit,
                    cny(amount),
                    None,
                ),
                JournalLine::new(
                    AccountId::from("cash"),
                    PostingSide::Credit,
                    cny(amount),
                    None,
                ),
            ],
        )
    }

    #[test]
    fn personal_owner_posts_without_approval() {
        let owner = UserId::from("owner");
        let repo = MemoryLedgerRepository::default();
        let ledger = personal_ledger(&owner);
        repo.save_ledger(ledger.clone()).unwrap();
        let service = LedgerService::new(repo.clone());

        let outcome = service
            .submit_entry(&owner, balanced_entry("entry-1", &ledger.id, &owner, 2_000))
            .unwrap();

        assert!(matches!(
            outcome,
            SubmissionOutcome::Posted(JournalEntry {
                status: JournalStatus::Posted,
                ..
            })
        ));
        assert_eq!(
            repo.list_approval_requests(&ledger.id).unwrap(),
            Vec::<ApprovalRequest>::new()
        );
    }

    #[test]
    fn company_bookkeeper_flow_requires_approval_then_posts() {
        let bookkeeper = UserId::from("bookkeeper");
        let approver = UserId::from("approver");
        let repo = MemoryLedgerRepository::default();
        let ledger = company_ledger();
        repo.save_ledger(ledger.clone()).unwrap();
        repo.save_member(LedgerMember::new(
            ledger.id.clone(),
            bookkeeper.clone(),
            LedgerRole::Bookkeeper,
        ))
        .unwrap();
        repo.save_member(LedgerMember::new(
            ledger.id.clone(),
            approver.clone(),
            LedgerRole::Approver,
        ))
        .unwrap();
        let service = LedgerService::new(repo);

        let outcome = service
            .submit_entry(
                &bookkeeper,
                balanced_entry("entry-2", &ledger.id, &bookkeeper, 2_000),
            )
            .unwrap();
        let approval_id = match outcome {
            SubmissionOutcome::PendingApproval(request) => request.id,
            SubmissionOutcome::Posted(_) => panic!("company bookkeeper entry should need approval"),
        };

        let outcome = service
            .decide_approval(&approver, &approval_id, ApprovalVote::Approve, None)
            .unwrap();

        assert_eq!(outcome.request.status, ApprovalStatus::Approved);
        assert_eq!(outcome.entry.status, JournalStatus::Posted);
    }

    #[test]
    fn submitter_cannot_approve_own_company_entry() {
        let bookkeeper = UserId::from("bookkeeper");
        let repo = MemoryLedgerRepository::default();
        let ledger = company_ledger();
        repo.save_ledger(ledger.clone()).unwrap();
        repo.save_member(LedgerMember::new(
            ledger.id.clone(),
            bookkeeper.clone(),
            LedgerRole::Bookkeeper,
        ))
        .unwrap();
        repo.save_member(LedgerMember::new(
            ledger.id.clone(),
            bookkeeper.clone(),
            LedgerRole::Approver,
        ))
        .unwrap();
        let service = LedgerService::new(repo);

        let approval_id = match service
            .submit_entry(
                &bookkeeper,
                balanced_entry("entry-3", &ledger.id, &bookkeeper, 2_000),
            )
            .unwrap()
        {
            SubmissionOutcome::PendingApproval(request) => request.id,
            SubmissionOutcome::Posted(_) => panic!("company bookkeeper entry should need approval"),
        };

        assert!(matches!(
            service.decide_approval(&bookkeeper, &approval_id, ApprovalVote::Approve, None),
            Err(ServiceError::SelfApprovalDenied)
        ));
    }

    #[test]
    fn auditor_cannot_submit_company_entry() {
        let auditor = UserId::from("auditor");
        let repo = MemoryLedgerRepository::default();
        let ledger = company_ledger();
        repo.save_ledger(ledger.clone()).unwrap();
        repo.save_member(LedgerMember::new(
            ledger.id.clone(),
            auditor.clone(),
            LedgerRole::Auditor,
        ))
        .unwrap();
        let service = LedgerService::new(repo);

        assert!(matches!(
            service.submit_entry(
                &auditor,
                balanced_entry("entry-4", &ledger.id, &auditor, 2_000)
            ),
            Err(ServiceError::PermissionDenied)
        ));
    }

    #[test]
    fn high_value_company_entry_requires_owner_approval() {
        let bookkeeper = UserId::from("bookkeeper");
        let approver = UserId::from("approver");
        let owner = UserId::from("owner");
        let repo = MemoryLedgerRepository::default();
        let ledger = company_ledger();
        repo.save_ledger(ledger.clone()).unwrap();
        repo.save_member(LedgerMember::new(
            ledger.id.clone(),
            bookkeeper.clone(),
            LedgerRole::Bookkeeper,
        ))
        .unwrap();
        repo.save_member(LedgerMember::new(
            ledger.id.clone(),
            approver.clone(),
            LedgerRole::Approver,
        ))
        .unwrap();
        repo.save_member(LedgerMember::new(
            ledger.id.clone(),
            owner.clone(),
            LedgerRole::Owner,
        ))
        .unwrap();
        let service = LedgerService::new(repo);

        let approval_id = match service
            .submit_entry(
                &bookkeeper,
                balanced_entry("entry-5", &ledger.id, &bookkeeper, 2_0000),
            )
            .unwrap()
        {
            SubmissionOutcome::PendingApproval(request) => request.id,
            SubmissionOutcome::Posted(_) => panic!("high value company entry should need approval"),
        };

        assert!(matches!(
            service.decide_approval(&approver, &approval_id, ApprovalVote::Approve, None),
            Err(ServiceError::RoleNotAccepted)
        ));

        let outcome = service
            .decide_approval(&owner, &approval_id, ApprovalVote::Approve, None)
            .unwrap();
        assert_eq!(outcome.request.status, ApprovalStatus::Approved);
        assert_eq!(outcome.entry.status, JournalStatus::Posted);
    }
}
