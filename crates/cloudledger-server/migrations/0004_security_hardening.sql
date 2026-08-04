-- This is an intentionally incompatible security cutover. Existing sessions
-- contain plaintext bearer material and must never be copied forward.
ALTER TABLE auth_users ADD COLUMN updated_at TIMESTAMPTZ;
UPDATE auth_users SET updated_at = created_at;
ALTER TABLE auth_users ALTER COLUMN updated_at SET NOT NULL;

DROP TABLE auth_sessions;

CREATE TABLE auth_sessions (
  id UUID PRIMARY KEY,
  family_id UUID NOT NULL,
  user_id UUID NOT NULL REFERENCES auth_users(id) ON DELETE CASCADE,
  installation_id TEXT,
  access_token_hash BYTEA NOT NULL UNIQUE,
  refresh_token_hash BYTEA UNIQUE,
  client_kind TEXT NOT NULL,
  access_expires_at TIMESTAMPTZ NOT NULL,
  refresh_expires_at TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL,
  rotated_at TIMESTAMPTZ,
  revoked_at TIMESTAMPTZ,
  CHECK (client_kind IN ('tauri', 'web', 'admin')),
  CHECK (
    (client_kind = 'admin' AND refresh_token_hash IS NULL AND refresh_expires_at IS NULL)
    OR
    (client_kind IN ('tauri', 'web') AND refresh_token_hash IS NOT NULL AND refresh_expires_at IS NOT NULL)
  )
);

CREATE INDEX auth_sessions_user_idx ON auth_sessions (user_id);
CREATE INDEX auth_sessions_family_idx ON auth_sessions (family_id);
CREATE INDEX auth_sessions_expiry_idx
  ON auth_sessions (refresh_expires_at, access_expires_at);

CREATE TABLE login_failure_buckets (
  surface TEXT NOT NULL,
  bucket_kind TEXT NOT NULL,
  client_ip INET NOT NULL,
  identifier_hmac BYTEA NOT NULL,
  failure_count INTEGER NOT NULL,
  window_started_at TIMESTAMPTZ NOT NULL,
  blocked_until TIMESTAMPTZ,
  last_seen_at TIMESTAMPTZ NOT NULL,
  PRIMARY KEY (surface, bucket_kind, client_ip, identifier_hmac),
  CHECK (bucket_kind IN ('login', 'ip')),
  CHECK (failure_count >= 0)
);

CREATE INDEX login_failure_buckets_cleanup_idx
  ON login_failure_buckets (last_seen_at, blocked_until);

CREATE TABLE security_rate_limits (
  bucket_kind TEXT NOT NULL,
  client_ip INET NOT NULL,
  request_count INTEGER NOT NULL,
  window_started_at TIMESTAMPTZ NOT NULL,
  blocked_until TIMESTAMPTZ,
  last_seen_at TIMESTAMPTZ NOT NULL,
  PRIMARY KEY (bucket_kind, client_ip),
  CHECK (bucket_kind IN ('refresh', 'invalid_bearer', 'anonymous_probe')),
  CHECK (request_count >= 0)
);

CREATE INDEX security_rate_limits_cleanup_idx
  ON security_rate_limits (last_seen_at, blocked_until);

ALTER TABLE audit_logs RENAME TO audit_logs_legacy;

CREATE TABLE audit_events (
  id UUID PRIMARY KEY,
  scope_key TEXT NOT NULL,
  sequence BIGINT NOT NULL,
  previous_hash BYTEA NOT NULL,
  event_hash BYTEA NOT NULL UNIQUE,
  key_id TEXT NOT NULL,
  actor_type TEXT NOT NULL,
  actor_id UUID,
  action TEXT NOT NULL,
  resource_type TEXT NOT NULL,
  resource_id UUID,
  metadata JSONB NOT NULL,
  occurred_at TIMESTAMPTZ NOT NULL,
  UNIQUE (scope_key, sequence),
  CHECK (scope_key = 'platform' OR scope_key ~ '^organization:[0-9a-f-]{36}$'),
  CHECK (sequence > 0),
  CHECK (octet_length(previous_hash) IN (0, 32)),
  CHECK (octet_length(event_hash) = 32),
  CHECK (actor_type IN ('platform_admin', 'organization_admin', 'business_user', 'system')),
  CHECK (jsonb_typeof(metadata) = 'object')
);

CREATE INDEX audit_events_occurred_idx ON audit_events (scope_key, occurred_at, id);
CREATE INDEX audit_events_resource_idx
  ON audit_events (resource_type, resource_id)
  WHERE resource_id IS NOT NULL;

CREATE OR REPLACE FUNCTION cloudledger_reject_mutation()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  RAISE EXCEPTION '% is append-only', TG_TABLE_NAME
    USING ERRCODE = '55000';
END;
$$;

CREATE OR REPLACE FUNCTION cloudledger_validate_audit_append()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
  expected_sequence BIGINT;
  expected_previous_hash BYTEA;
BEGIN
  -- Serialize writers for this chain even when the first event has no row to lock.
  PERFORM pg_advisory_xact_lock(hashtextextended(NEW.scope_key, 0));
  SELECT sequence + 1, event_hash
    INTO expected_sequence, expected_previous_hash
    FROM audit_events
    WHERE scope_key = NEW.scope_key
    ORDER BY sequence DESC
    LIMIT 1;

  IF expected_sequence IS NULL THEN
    expected_sequence := 1;
    expected_previous_hash := ''::bytea;
  END IF;
  IF NEW.sequence <> expected_sequence OR NEW.previous_hash <> expected_previous_hash THEN
    RAISE EXCEPTION 'invalid audit chain head for %', NEW.scope_key
      USING ERRCODE = '23514';
  END IF;
  RETURN NEW;
END;
$$;

CREATE TRIGGER audit_events_validate_append
BEFORE INSERT ON audit_events
FOR EACH ROW EXECUTE FUNCTION cloudledger_validate_audit_append();

CREATE TRIGGER audit_events_reject_update_delete
BEFORE UPDATE OR DELETE ON audit_events
FOR EACH ROW EXECUTE FUNCTION cloudledger_reject_mutation();

CREATE TRIGGER audit_logs_legacy_reject_update_delete
BEFORE UPDATE OR DELETE ON audit_logs_legacy
FOR EACH ROW EXECUTE FUNCTION cloudledger_reject_mutation();

REVOKE UPDATE, DELETE, TRUNCATE ON audit_events FROM PUBLIC;
REVOKE INSERT, UPDATE, DELETE, TRUNCATE ON audit_logs_legacy FROM PUBLIC;

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'cloudledger_runtime') THEN
    EXECUTE 'REVOKE UPDATE, DELETE, TRUNCATE ON audit_events FROM cloudledger_runtime';
    EXECUTE 'REVOKE INSERT, UPDATE, DELETE, TRUNCATE ON audit_logs_legacy FROM cloudledger_runtime';
  END IF;
END;
$$;

UPDATE app_metadata SET schema_version = 4 WHERE schema_version < 4;
