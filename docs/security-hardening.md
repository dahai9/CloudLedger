# Security Hardening And Production Rollout

This release is an incompatible authentication and audit cutover. Production
supports the Tauri client only. Caddy terminates HTTPS while both CloudLedger
listeners remain on loopback. Every pre-cutover session is intentionally
invalidated by migration `0004_security_hardening.sql`.

## Production Invariants

- Set `server.mode = "reverse_proxy"`, keep both bind addresses on loopback,
  use HTTPS public URLs, disable Web login, and set `database.auto_migrate = false`.
- Configure both Turnstile keys. Configure only `tauri://localhost` and
  `https://tauri.localhost` as CORS origins.
- Keep the runtime PostgreSQL URL in the private TOML file. Never place the
  migration URL, Turnstile secret, audit keys, password, or raw token in client
  configuration or logs.
- A remote PostgreSQL URL must use `sslmode=verify-full`; install the required
  CA and use a hostname covered by the database certificate.
- Back up `security.audit.key_id`, `hmac_key`, and `identifier_hmac_key` with the
  database. Losing the HMAC key makes historical verification impossible.

The supplied `deploy/Caddyfile` uses separate API and admin hostnames, automatic
certificates, HSTS, a 64 KiB body limit, and bounded connection/request
timeouts. Set `CLOUDLEDGER_API_DOMAIN` and `CLOUDLEDGER_ADMIN_DOMAIN` in Caddy's
environment. Caddy replaces forwarding headers with its direct client address;
the application trusts them only from configured loopback proxy CIDRs.

## Database Roles

Run the role bootstrap as the database owner. Pass passwords as `psql`
variables so they do not enter the SQL file:

```bash
psql -d cloudledger \
  -v database_name=cloudledger \
  -v migration_password='...' \
  -v runtime_password='...' \
  -f deploy/postgres_roles.sql
```

The migration role owns schema objects and is used only by the one-time
command. The runtime role can query and append normal data, but cannot change
or delete audit events and cannot write the legacy audit archive. Database
triggers independently reject audit `UPDATE` and `DELETE` and validate each
new sequence and previous hash.

## Fixed Rollout Order

1. Back up PostgreSQL, the private backend TOML, and the audit HMAC key material.
2. Stop every old CloudLedger server instance.
3. Run `deploy/postgres_roles.sql` and set the runtime URL in the production TOML.
4. Run the migration once:

   ```bash
   CLOUDLEDGER_MIGRATION_DATABASE_URL='postgres://cloudledger_migration:...@127.0.0.1/cloudledger' \
     cloudledger-server migrate --config /etc/cloudledger/server.toml
   ```

5. Run `cloudledger-server audit verify --config /etc/cloudledger/server.toml`.
6. Validate and deploy Caddy, then start CloudLedger with its runtime credential.
7. Confirm the API/admin processes listen only on loopback, HTTPS `/ready`
   succeeds, HTTP redirects or is rejected, and Caddy rejects bodies above 64 KiB.
8. Release the new Tauri client. Confirm an old client receives HTTP `426` from
   `/auth/login`, then confirm login, refresh rotation, logout, and admin login.
9. Confirm PostgreSQL and browser storage contain no raw access or refresh token.

## Organization Administrator Password Recovery

An organization administrator can open the `修改密码` action in the admin
backend, provide the current password, and set a new 12–128 character password.
After a successful change, every administrator session for that account is
revoked and the account must sign in again.

If the current password is forgotten, a platform administrator signs in through
the platform-token entry, selects the organization administrator, and uses
`重置密码`. The reset also revokes all existing sessions. This path is limited
to `owner`/`admin` memberships whose authentication identity is scoped to the
same organization; it cannot reset employee accounts or accounts in another
organization. Passwords and reset values are never written to audit metadata.

Do not restart an old binary after migration. Restore the database and matching
audit keys from the pre-cutover backup if rollback is required.

## Audit Verification

Audit events form one global platform chain and one independent chain per
organization. Each event signs its scope, sequence, previous hash, key ID,
actor, action, resource, structured metadata, timestamp, and event ID using
HMAC-SHA256. Legacy business audit rows are imported in timestamp/ID order,
followed by `legacy_cutover`; the old table remains a read-only archive.

Run verification after migration, before deployment, and during operational
checks:

```bash
cloudledger-server audit verify --config /etc/cloudledger/server.toml
```

A linkage, key ID, or HMAC mismatch exits nonzero. Each successful append emits
a structured `audit_chain_head` log record containing the scope, sequence,
digest, and key ID, never the HMAC key or sensitive request material.

## Threat Model

The audit chain detects database-only modification, deletion, insertion, and
reordering by an actor who does not possess the application audit key. Database
permissions and triggers also prevent the normal runtime role from mutating
history. It does not claim to resist an attacker who simultaneously controls
the application host and audit HMAC key, nor does it replace encrypted backups,
host hardening, PostgreSQL access logging, or key custody procedures.

Audit metadata must never contain passwords, raw access/refresh tokens,
Turnstile responses, or complete login identifiers. Login protection stores a
keyed identifier digest and client IP only. The first release has no online
audit-key rotation; a future rotation must preserve verification keys by key ID.
