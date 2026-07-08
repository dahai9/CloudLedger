# CloudLedger Data Model

Core entities:

- `User`: login identity.
- `Organization`: company or team.
- `Membership`: user role within an organization.
- `Ledger`: personal private ledger or organization public ledger.
- `FinancialAccount`: cash, bank, wallet, company account, receivable, payable.
- `Category`: income or expense classification.
- `Transaction`: ledger transaction with approval state and soft-delete fields.
- `TransactionEntry`: reserved for future double-entry accounting.
- `AuditLog`: append-only record of sensitive public-ledger changes.
- `SyncEvent`: ordered change stream for cloud sync.

All persisted accounting rows carry stable UUIDs, timestamps, version metadata,
and ledger/organization scope fields so private and public data cannot rely on
UI-only isolation.
