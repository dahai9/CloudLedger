pragma foreign_keys = on;

create table if not exists schema_migrations (
  version text primary key,
  applied_at text not null default (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

insert or ignore into schema_migrations(version) values ('0001_init');

create table if not exists users (
  id text primary key,
  display_name text not null,
  email text unique,
  phone text unique,
  created_at text not null
);

create table if not exists organizations (
  id text primary key,
  name text not null,
  created_by text not null references users(id),
  created_at text not null,
  deleted_at text
);

create table if not exists organization_members (
  id text primary key,
  organization_id text not null references organizations(id),
  user_id text not null references users(id),
  role text not null check (role in ('owner', 'admin', 'accountant', 'approver', 'member', 'viewer')),
  created_at text not null,
  deleted_at text,
  unique (organization_id, user_id)
);

create table if not exists ledgers (
  id text primary key,
  name text not null,
  kind text not null check (kind in ('personal', 'organization_public')),
  owner_user_id text references users(id),
  organization_id text references organizations(id),
  created_at text not null,
  deleted_at text,
  check (
    (kind = 'personal' and owner_user_id is not null and organization_id is null) or
    (kind = 'organization_public' and owner_user_id is null and organization_id is not null)
  )
);

create table if not exists financial_accounts (
  id text primary key,
  ledger_id text not null references ledgers(id),
  name text not null,
  kind text not null,
  opening_balance_minor integer not null,
  currency text not null,
  created_at text not null,
  deleted_at text
);

create table if not exists categories (
  id text primary key,
  ledger_id text not null references ledgers(id),
  name text not null,
  kind text not null check (kind in ('income', 'expense')),
  deleted_at text
);

create table if not exists transactions (
  id text primary key,
  ledger_id text not null references ledgers(id),
  account_id text not null references financial_accounts(id),
  category_id text references categories(id),
  kind text not null check (kind in ('income', 'expense', 'transfer')),
  amount_minor integer not null,
  currency text not null,
  occurred_at text not null,
  description text not null,
  approval_state text not null check (approval_state in ('draft', 'submitted', 'approved', 'rejected', 'voided')),
  created_by text not null references users(id),
  submitted_by text references users(id),
  approved_by text references users(id),
  version integer not null,
  created_at text not null,
  updated_at text not null,
  deleted_at text
);

create table if not exists transaction_entries (
  id text primary key,
  transaction_id text not null references transactions(id),
  account_id text not null references financial_accounts(id),
  direction text not null check (direction in ('debit', 'credit')),
  amount_minor integer not null,
  currency text not null
);

create table if not exists attachments (
  id text primary key,
  transaction_id text not null references transactions(id),
  file_name text not null,
  content_type text not null,
  storage_uri text not null,
  sha256 text,
  created_at text not null,
  deleted_at text
);

create table if not exists audit_logs (
  id text primary key,
  organization_id text references organizations(id),
  ledger_id text not null references ledgers(id),
  actor_user_id text not null references users(id),
  action text not null,
  resource_type text not null,
  resource_id text not null,
  summary text not null,
  created_at text not null
);

create table if not exists sync_events (
  id text primary key,
  ledger_id text not null references ledgers(id),
  resource_type text not null,
  resource_id text not null,
  version integer not null,
  payload_json text not null,
  created_at text not null
);
