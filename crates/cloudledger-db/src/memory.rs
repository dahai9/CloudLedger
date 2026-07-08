use std::collections::HashMap;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use cloudledger_core::{
    book::Ledger, ApprovalId, ApprovalRequest, JournalEntry, JournalEntryId, LedgerId,
    LedgerMember, LedgerRole, UserId,
};

use crate::{LedgerRepository, RepoResult, RepositoryError};

#[derive(Debug, Default)]
struct MemoryState {
    ledgers: HashMap<LedgerId, Ledger>,
    members: HashMap<(LedgerId, UserId, LedgerRole), LedgerMember>,
    entries: HashMap<JournalEntryId, JournalEntry>,
    approvals: HashMap<ApprovalId, ApprovalRequest>,
}

#[derive(Debug, Clone, Default)]
pub struct MemoryLedgerRepository {
    inner: Arc<RwLock<MemoryState>>,
}

impl MemoryLedgerRepository {
    fn read(&self) -> RepoResult<RwLockReadGuard<'_, MemoryState>> {
        self.inner
            .read()
            .map_err(|_| RepositoryError::backend("memory repository lock poisoned"))
    }

    fn write(&self) -> RepoResult<RwLockWriteGuard<'_, MemoryState>> {
        self.inner
            .write()
            .map_err(|_| RepositoryError::backend("memory repository lock poisoned"))
    }
}

impl LedgerRepository for MemoryLedgerRepository {
    fn save_ledger(&self, ledger: Ledger) -> RepoResult<()> {
        self.write()?.ledgers.insert(ledger.id.clone(), ledger);
        Ok(())
    }

    fn get_ledger(&self, id: &LedgerId) -> RepoResult<Option<Ledger>> {
        Ok(self.read()?.ledgers.get(id).cloned())
    }

    fn save_member(&self, member: LedgerMember) -> RepoResult<()> {
        let key = (
            member.ledger_id.clone(),
            member.user_id.clone(),
            member.role,
        );
        self.write()?.members.insert(key, member);
        Ok(())
    }

    fn list_members(&self, ledger_id: &LedgerId) -> RepoResult<Vec<LedgerMember>> {
        let mut members = self
            .read()?
            .members
            .values()
            .filter(|member| &member.ledger_id == ledger_id)
            .cloned()
            .collect::<Vec<_>>();
        members.sort_by(|a, b| a.user_id.cmp(&b.user_id).then_with(|| a.role.cmp(&b.role)));
        Ok(members)
    }

    fn save_journal_entry(&self, entry: JournalEntry) -> RepoResult<()> {
        self.write()?.entries.insert(entry.id.clone(), entry);
        Ok(())
    }

    fn get_journal_entry(&self, id: &JournalEntryId) -> RepoResult<Option<JournalEntry>> {
        Ok(self.read()?.entries.get(id).cloned())
    }

    fn list_journal_entries(&self, ledger_id: &LedgerId) -> RepoResult<Vec<JournalEntry>> {
        let mut entries = self
            .read()?
            .entries
            .values()
            .filter(|entry| &entry.ledger_id == ledger_id)
            .cloned()
            .collect::<Vec<_>>();
        entries.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(entries)
    }

    fn save_approval_request(&self, request: ApprovalRequest) -> RepoResult<()> {
        self.write()?.approvals.insert(request.id.clone(), request);
        Ok(())
    }

    fn get_approval_request(&self, id: &ApprovalId) -> RepoResult<Option<ApprovalRequest>> {
        Ok(self.read()?.approvals.get(id).cloned())
    }

    fn list_approval_requests(&self, ledger_id: &LedgerId) -> RepoResult<Vec<ApprovalRequest>> {
        let mut approvals = self
            .read()?
            .approvals
            .values()
            .filter(|request| &request.ledger_id == ledger_id)
            .cloned()
            .collect::<Vec<_>>();
        approvals.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(approvals)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cloudledger_core::{amount::Money, CompanyId, CurrencyCode};

    #[test]
    fn memory_repository_round_trips_ledger_members_and_entries() {
        let repo = MemoryLedgerRepository::default();
        let ledger = Ledger::company(
            LedgerId::from("company"),
            CompanyId::from("acme"),
            "ACME",
            CurrencyCode::new("CNY"),
            Money::new(CurrencyCode::new("CNY"), 10_000),
        );
        let member = LedgerMember::new(
            ledger.id.clone(),
            UserId::from("bookkeeper"),
            LedgerRole::Bookkeeper,
        );

        repo.save_ledger(ledger.clone()).unwrap();
        repo.save_member(member.clone()).unwrap();

        assert_eq!(repo.get_ledger(&ledger.id).unwrap(), Some(ledger.clone()));
        assert_eq!(repo.list_members(&ledger.id).unwrap(), vec![member]);
        assert!(repo.list_journal_entries(&ledger.id).unwrap().is_empty());
    }
}
