CREATE TABLE app_metadata (
  singleton_id SMALLINT PRIMARY KEY,
  schema_version INTEGER NOT NULL,
  current_user_id UUID,
  CHECK (singleton_id = 1)
);

CREATE TABLE domain_users (
  id UUID PRIMARY KEY,
  display_name TEXT NOT NULL,
  email TEXT UNIQUE,
  phone TEXT UNIQUE,
  created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE organizations (
  id UUID PRIMARY KEY,
  name TEXT NOT NULL,
  created_by UUID NOT NULL REFERENCES domain_users(id) ON DELETE RESTRICT,
  created_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX organizations_created_by_idx ON organizations (created_by);

CREATE TABLE organization_memberships (
  id UUID PRIMARY KEY,
  organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
  user_id UUID NOT NULL REFERENCES domain_users(id) ON DELETE RESTRICT,
  role TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL,
  UNIQUE (organization_id, user_id),
  CHECK (role IN ('owner', 'admin', 'accountant', 'approver', 'member', 'viewer'))
);

CREATE INDEX organization_memberships_org_idx
  ON organization_memberships (organization_id);
CREATE INDEX organization_memberships_user_idx
  ON organization_memberships (user_id);

CREATE TABLE ledgers (
  id UUID PRIMARY KEY,
  name TEXT NOT NULL,
  kind TEXT NOT NULL,
  owner_user_id UUID REFERENCES domain_users(id) ON DELETE RESTRICT,
  organization_id UUID REFERENCES organizations(id) ON DELETE CASCADE,
  created_at TIMESTAMPTZ NOT NULL,
  deleted_at TIMESTAMPTZ,
  CHECK (kind IN ('personal', 'organization_public')),
  CHECK (
    (kind = 'personal' AND owner_user_id IS NOT NULL AND organization_id IS NULL)
    OR
    (kind = 'organization_public' AND owner_user_id IS NULL AND organization_id IS NOT NULL)
  )
);

CREATE INDEX ledgers_owner_idx ON ledgers (owner_user_id);
CREATE INDEX ledgers_organization_idx ON ledgers (organization_id);

CREATE TABLE financial_accounts (
  id UUID PRIMARY KEY,
  ledger_id UUID NOT NULL REFERENCES ledgers(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  kind TEXT NOT NULL,
  opening_balance_minor BIGINT NOT NULL,
  currency TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL,
  deleted_at TIMESTAMPTZ,
  CHECK (kind IN ('cash', 'bank', 'wallet', 'credit', 'receivable', 'payable')),
  CHECK (char_length(currency) = 3 AND currency = upper(currency))
);

CREATE INDEX financial_accounts_ledger_idx ON financial_accounts (ledger_id);

CREATE TABLE transactions (
  id UUID PRIMARY KEY,
  ledger_id UUID NOT NULL REFERENCES ledgers(id) ON DELETE CASCADE,
  account_id UUID NOT NULL REFERENCES financial_accounts(id) ON DELETE RESTRICT,
  category_id UUID,
  kind TEXT NOT NULL,
  amount_minor BIGINT NOT NULL,
  currency TEXT NOT NULL,
  occurred_at TIMESTAMPTZ NOT NULL,
  description TEXT NOT NULL,
  approval_state TEXT NOT NULL,
  created_by UUID NOT NULL REFERENCES domain_users(id) ON DELETE RESTRICT,
  submitted_by UUID REFERENCES domain_users(id) ON DELETE RESTRICT,
  approved_by UUID REFERENCES domain_users(id) ON DELETE RESTRICT,
  version BIGINT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL,
  deleted_at TIMESTAMPTZ,
  CHECK (kind IN ('income', 'expense', 'transfer')),
  CHECK (approval_state IN ('draft', 'submitted', 'approved', 'rejected', 'voided')),
  CHECK (char_length(currency) = 3 AND currency = upper(currency)),
  CHECK (version > 0)
);

CREATE INDEX transactions_ledger_occurred_idx
  ON transactions (ledger_id, occurred_at DESC);
CREATE INDEX transactions_approval_idx
  ON transactions (ledger_id, approval_state);

CREATE TABLE audit_logs (
  id UUID PRIMARY KEY,
  organization_id UUID REFERENCES organizations(id)
    ON DELETE NO ACTION DEFERRABLE INITIALLY DEFERRED,
  ledger_id UUID NOT NULL REFERENCES ledgers(id) ON DELETE RESTRICT,
  actor_user_id UUID NOT NULL REFERENCES domain_users(id) ON DELETE RESTRICT,
  action TEXT NOT NULL,
  resource_type TEXT NOT NULL,
  resource_id UUID NOT NULL,
  summary TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX audit_logs_ledger_created_idx ON audit_logs (ledger_id, created_at DESC);
CREATE INDEX audit_logs_organization_idx ON audit_logs (organization_id);

CREATE TABLE auth_users (
  id UUID PRIMARY KEY,
  display_name TEXT NOT NULL,
  email TEXT UNIQUE,
  phone TEXT UNIQUE,
  password_hash TEXT NOT NULL,
  account_kind TEXT NOT NULL,
  organization_id UUID REFERENCES organizations(id)
    ON DELETE NO ACTION DEFERRABLE INITIALLY DEFERRED,
  created_at TIMESTAMPTZ NOT NULL,
  CHECK (account_kind IN ('business', 'organization_admin')),
  CHECK (
    (account_kind = 'business' AND organization_id IS NULL)
    OR
    (account_kind = 'organization_admin' AND organization_id IS NOT NULL)
  )
);

CREATE INDEX auth_users_organization_idx ON auth_users (organization_id);

CREATE TABLE auth_installations (
  installation_id TEXT PRIMARY KEY,
  user_id UUID NOT NULL REFERENCES auth_users(id) ON DELETE CASCADE
);

CREATE INDEX auth_installations_user_idx ON auth_installations (user_id);

CREATE TABLE auth_sessions (
  access_token TEXT PRIMARY KEY,
  user_id UUID NOT NULL REFERENCES auth_users(id) ON DELETE CASCADE,
  installation_id TEXT,
  refresh_token TEXT,
  kind TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL,
  refreshed_at TIMESTAMPTZ NOT NULL,
  CHECK (kind IN ('app', 'admin'))
);

CREATE INDEX auth_sessions_user_idx ON auth_sessions (user_id);
CREATE UNIQUE INDEX auth_sessions_refresh_token_uq
  ON auth_sessions (refresh_token)
  WHERE refresh_token IS NOT NULL;
