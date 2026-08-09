-- Run as the bootstrap database owner. psql variables are required:
--   psql -v database_name=cloudledger -v migration_password='...' \
--     -v runtime_password='...' -f deploy/postgres_roles.sql
SELECT format('CREATE ROLE cloudledger_migration LOGIN PASSWORD %L', :'migration_password')
WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'cloudledger_migration') \gexec
SELECT format('CREATE ROLE cloudledger_runtime LOGIN PASSWORD %L', :'runtime_password')
WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'cloudledger_runtime') \gexec

-- cloudledger_bootstrap is created by the official PostgreSQL image from
-- POSTGRES_USER/POSTGRES_PASSWORD and is reserved for this operations tool.
-- It is intentionally not used by the long-running CloudLedger service.

ALTER ROLE cloudledger_migration LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS PASSWORD :'migration_password';
ALTER ROLE cloudledger_runtime LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS PASSWORD :'runtime_password';

GRANT CONNECT ON DATABASE :"database_name" TO cloudledger_migration, cloudledger_runtime;
GRANT USAGE, CREATE ON SCHEMA public TO cloudledger_migration;
GRANT USAGE ON SCHEMA public TO cloudledger_runtime;

-- Existing installations may have created schema objects with the old service
-- role. Transfer ownership so the dedicated migration role can apply 0004.
DO $$
DECLARE
  relation RECORD;
BEGIN
  FOR relation IN
    SELECT format('%I.%I', schemaname, tablename) AS name
    FROM pg_tables
    WHERE schemaname = 'public'
  LOOP
    EXECUTE format('ALTER TABLE %s OWNER TO cloudledger_migration', relation.name);
  END LOOP;
END;
$$;

DO $$
DECLARE
  sequence_name RECORD;
BEGIN
  FOR sequence_name IN
    SELECT format('%I.%I', schemaname, sequencename) AS name
    FROM pg_sequences
    WHERE schemaname = 'public'
  LOOP
    EXECUTE format('ALTER SEQUENCE %s OWNER TO cloudledger_migration', sequence_name.name);
  END LOOP;
END;
$$;

GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO cloudledger_runtime;

ALTER DEFAULT PRIVILEGES FOR ROLE cloudledger_migration IN SCHEMA public
  GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO cloudledger_runtime;

SELECT 'REVOKE INSERT, UPDATE, DELETE, TRUNCATE ON _sqlx_migrations FROM cloudledger_runtime'
WHERE to_regclass('public._sqlx_migrations') IS NOT NULL \gexec
SELECT 'GRANT SELECT ON _sqlx_migrations TO cloudledger_runtime'
WHERE to_regclass('public._sqlx_migrations') IS NOT NULL \gexec

-- These relations exist only after 0004. Re-running the bootstrap after
-- migration is harmless and reinforces the grants made by the migration.
SELECT 'REVOKE UPDATE, DELETE, TRUNCATE ON audit_events FROM cloudledger_runtime'
WHERE to_regclass('public.audit_events') IS NOT NULL \gexec
SELECT 'REVOKE INSERT, UPDATE, DELETE, TRUNCATE ON audit_logs_legacy FROM cloudledger_runtime'
WHERE to_regclass('public.audit_logs_legacy') IS NOT NULL \gexec
