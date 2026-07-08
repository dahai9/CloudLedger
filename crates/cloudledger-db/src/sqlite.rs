use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use cloudledger_core::{
    book::{Ledger, LedgerKind},
    ApprovalId, ApprovalRequest, JournalEntry, JournalEntryId, LedgerId, LedgerMember,
};
use rusqlite::{params, Connection, OptionalExtension};

use crate::{LedgerRepository, RepoResult, RepositoryError};

#[derive(Debug)]
pub struct SqliteLedgerRepository {
    conn: Mutex<Connection>,
}

impl SqliteLedgerRepository {
    pub fn open(path: impl AsRef<Path>) -> RepoResult<Self> {
        let repo = Self {
            conn: Mutex::new(Connection::open(path)?),
        };
        repo.init_schema()?;
        Ok(repo)
    }

    pub fn in_memory() -> RepoResult<Self> {
        let repo = Self {
            conn: Mutex::new(Connection::open_in_memory()?),
        };
        repo.init_schema()?;
        Ok(repo)
    }

    fn conn(&self) -> RepoResult<MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| RepositoryError::backend("sqlite connection lock poisoned"))
    }

    fn init_schema(&self) -> RepoResult<()> {
        self.conn()?.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS ledgers (
                id TEXT PRIMARY KEY NOT NULL,
                kind TEXT NOT NULL,
                document TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS ledger_members (
                ledger_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                role TEXT NOT NULL,
                document TEXT NOT NULL,
                PRIMARY KEY (ledger_id, user_id, role)
            );

            CREATE TABLE IF NOT EXISTS journal_entries (
                id TEXT PRIMARY KEY NOT NULL,
                ledger_id TEXT NOT NULL,
                status TEXT NOT NULL,
                document TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS approval_requests (
                id TEXT PRIMARY KEY NOT NULL,
                ledger_id TEXT NOT NULL,
                entry_id TEXT NOT NULL,
                status TEXT NOT NULL,
                document TEXT NOT NULL
            );
            ",
        )?;
        Ok(())
    }
}

impl LedgerRepository for SqliteLedgerRepository {
    fn save_ledger(&self, ledger: Ledger) -> RepoResult<()> {
        let kind = match &ledger.kind {
            LedgerKind::PersonalPrivate { .. } => "personal_private",
            LedgerKind::CompanyPublic { .. } => "company_public",
        };
        self.conn()?.execute(
            "
            INSERT INTO ledgers (id, kind, document)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(id) DO UPDATE SET kind = excluded.kind, document = excluded.document
            ",
            params![ledger.id.as_str(), kind, serde_json::to_string(&ledger)?],
        )?;
        Ok(())
    }

    fn get_ledger(&self, id: &LedgerId) -> RepoResult<Option<Ledger>> {
        let document = self
            .conn()?
            .query_row(
                "SELECT document FROM ledgers WHERE id = ?1",
                params![id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;

        document
            .map(|document| serde_json::from_str(&document).map_err(RepositoryError::from))
            .transpose()
    }

    fn save_member(&self, member: LedgerMember) -> RepoResult<()> {
        self.conn()?.execute(
            "
            INSERT INTO ledger_members (ledger_id, user_id, role, document)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(ledger_id, user_id, role) DO UPDATE SET document = excluded.document
            ",
            params![
                member.ledger_id.as_str(),
                member.user_id.as_str(),
                format!("{:?}", member.role),
                serde_json::to_string(&member)?
            ],
        )?;
        Ok(())
    }

    fn list_members(&self, ledger_id: &LedgerId) -> RepoResult<Vec<LedgerMember>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "
            SELECT document
            FROM ledger_members
            WHERE ledger_id = ?1
            ORDER BY user_id, role
            ",
        )?;
        let members = stmt
            .query_map(params![ledger_id.as_str()], |row| row.get::<_, String>(0))?
            .map(|document| {
                document
                    .map_err(RepositoryError::from)
                    .and_then(|document| {
                        serde_json::from_str(&document).map_err(RepositoryError::from)
                    })
            })
            .collect::<RepoResult<Vec<_>>>()?;
        Ok(members)
    }

    fn save_journal_entry(&self, entry: JournalEntry) -> RepoResult<()> {
        self.conn()?.execute(
            "
            INSERT INTO journal_entries (id, ledger_id, status, document)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(id) DO UPDATE SET
                ledger_id = excluded.ledger_id,
                status = excluded.status,
                document = excluded.document
            ",
            params![
                entry.id.as_str(),
                entry.ledger_id.as_str(),
                format!("{:?}", entry.status),
                serde_json::to_string(&entry)?
            ],
        )?;
        Ok(())
    }

    fn get_journal_entry(&self, id: &JournalEntryId) -> RepoResult<Option<JournalEntry>> {
        let document = self
            .conn()?
            .query_row(
                "SELECT document FROM journal_entries WHERE id = ?1",
                params![id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;

        document
            .map(|document| serde_json::from_str(&document).map_err(RepositoryError::from))
            .transpose()
    }

    fn list_journal_entries(&self, ledger_id: &LedgerId) -> RepoResult<Vec<JournalEntry>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "
            SELECT document
            FROM journal_entries
            WHERE ledger_id = ?1
            ORDER BY id
            ",
        )?;
        let entries = stmt
            .query_map(params![ledger_id.as_str()], |row| row.get::<_, String>(0))?
            .map(|document| {
                document
                    .map_err(RepositoryError::from)
                    .and_then(|document| {
                        serde_json::from_str(&document).map_err(RepositoryError::from)
                    })
            })
            .collect::<RepoResult<Vec<_>>>()?;
        Ok(entries)
    }

    fn save_approval_request(&self, request: ApprovalRequest) -> RepoResult<()> {
        self.conn()?.execute(
            "
            INSERT INTO approval_requests (id, ledger_id, entry_id, status, document)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(id) DO UPDATE SET
                ledger_id = excluded.ledger_id,
                entry_id = excluded.entry_id,
                status = excluded.status,
                document = excluded.document
            ",
            params![
                request.id.as_str(),
                request.ledger_id.as_str(),
                request.entry_id.as_str(),
                format!("{:?}", request.status),
                serde_json::to_string(&request)?
            ],
        )?;
        Ok(())
    }

    fn get_approval_request(&self, id: &ApprovalId) -> RepoResult<Option<ApprovalRequest>> {
        let document = self
            .conn()?
            .query_row(
                "SELECT document FROM approval_requests WHERE id = ?1",
                params![id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;

        document
            .map(|document| serde_json::from_str(&document).map_err(RepositoryError::from))
            .transpose()
    }

    fn list_approval_requests(&self, ledger_id: &LedgerId) -> RepoResult<Vec<ApprovalRequest>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "
            SELECT document
            FROM approval_requests
            WHERE ledger_id = ?1
            ORDER BY id
            ",
        )?;
        let requests = stmt
            .query_map(params![ledger_id.as_str()], |row| row.get::<_, String>(0))?
            .map(|document| {
                document
                    .map_err(RepositoryError::from)
                    .and_then(|document| {
                        serde_json::from_str(&document).map_err(RepositoryError::from)
                    })
            })
            .collect::<RepoResult<Vec<_>>>()?;
        Ok(requests)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cloudledger_core::{amount::Money, book::Ledger, CompanyId, CurrencyCode};

    #[test]
    fn sqlite_repository_round_trips_ledger() {
        let repo = SqliteLedgerRepository::in_memory().unwrap();
        let ledger = Ledger::company(
            LedgerId::from("company"),
            CompanyId::from("acme"),
            "ACME",
            CurrencyCode::new("CNY"),
            Money::new(CurrencyCode::new("CNY"), 10_000),
        );

        repo.save_ledger(ledger.clone()).unwrap();

        assert_eq!(repo.get_ledger(&ledger.id).unwrap(), Some(ledger));
    }
}
