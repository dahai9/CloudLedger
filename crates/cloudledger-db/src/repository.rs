use cloudledger_core::{
    book::Ledger, ApprovalId, ApprovalRequest, JournalEntry, JournalEntryId, LedgerId, LedgerMember,
};

use crate::error::RepoResult;

pub trait LedgerRepository: Send + Sync {
    fn save_ledger(&self, ledger: Ledger) -> RepoResult<()>;
    fn get_ledger(&self, id: &LedgerId) -> RepoResult<Option<Ledger>>;

    fn save_member(&self, member: LedgerMember) -> RepoResult<()>;
    fn list_members(&self, ledger_id: &LedgerId) -> RepoResult<Vec<LedgerMember>>;

    fn save_journal_entry(&self, entry: JournalEntry) -> RepoResult<()>;
    fn get_journal_entry(&self, id: &JournalEntryId) -> RepoResult<Option<JournalEntry>>;
    fn list_journal_entries(&self, ledger_id: &LedgerId) -> RepoResult<Vec<JournalEntry>>;

    fn save_approval_request(&self, request: ApprovalRequest) -> RepoResult<()>;
    fn get_approval_request(&self, id: &ApprovalId) -> RepoResult<Option<ApprovalRequest>>;
    fn list_approval_requests(&self, ledger_id: &LedgerId) -> RepoResult<Vec<ApprovalRequest>>;
}
