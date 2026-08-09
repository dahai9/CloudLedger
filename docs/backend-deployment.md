# CloudLedger Backend Deployment

This guide deploys CloudLedger v0.1.4 on a systemd Linux host with Docker,
PostgreSQL, Caddy, Cloudflare, and encrypted OneDrive backups. PostgreSQL is the
authoritative store. The API reaches the Internet only through Cloudflare HTTPS,
and the admin listener remains private on the host loopback interface.

## Interactive Operations Toolbox

Run the operations script from the checked-out, explicit v0.1.4 tag. It is the
single supported interface for production administrators:

```bash
sudo ./deploy/cloudledger-ops.sh
```

Choose features only through the numeric, multi-level menus. Every menu uses
`0` to return or exit, invalid input redisplays the current menu, and completed
actions return to that menu after an Enter prompt. The toolbox hides passwords,
PATs, rclone credentials, webhook tokens, and database secrets. It shows an
impact summary and asks twice before destructive operations; database restore
also requires the selected backup number to be typed again.

The install wizard atomically stages the script, Compose file, Caddyfile,
PostgreSQL initialization files, and systemd units under `/opt/cloudledger`.
Private configuration is not stored beside those deployment assets:

| Path | Purpose | Required mode |
| --- | --- | --- |
| `/etc/cloudledger/ops.env` | Image tags, deployment credentials, domains, thresholds, and backup settings | `0600` |
| `/etc/cloudledger/server.toml` | Runtime database URL, Turnstile, admin, and audit secrets | `0600` |
| `/etc/cloudledger/rclone.conf` | OneDrive and rclone crypt remotes | `0600` |
| `/var/lib/cloudledger-ops` | State, logs, backup cache, and lock file | private to root |

There are no public subcommands. Four hidden internal tasks exist solely as
systemd implementation details; administrators use the corresponding numeric
menus instead of running these examples manually:

```ini
ExecStart=/opt/cloudledger/cloudledger-ops.sh --internal backup
ExecStart=/opt/cloudledger/cloudledger-ops.sh --internal health
ExecStart=/opt/cloudledger/cloudledger-ops.sh --internal restore-test
ExecStart=/opt/cloudledger/cloudledger-ops.sh --internal firewall-refresh
```

The wizard installs eight unit files, one service and one persistent timer for
each of `backup`, `health`, `restore-test`, and `firewall-refresh`. Use the
“定时任务管理” numeric menu to inspect, enable, disable, reschedule, or run
them. `flock` serializes deployment, upgrade, backup, restore, firewall refresh,
and role-hardening work.

The backup service requires a real, non-empty custom-format `pg_dump -Fc`, adds
the deployment configuration and Origin CA pair, writes a manifest and
`SHA256SUMS`, and first creates a hidden local `.new` candidate. It validates
that the manifest ID matches the canonical archive filename and that its UTC
creation time is valid and not in the future. It then validates
that candidate, uploads it to a hidden `.new` object on the rclone crypt remote,
downloads the remote object, and compares it byte for byte. Only then are the
remote and local objects atomically published under their canonical names. A
failed validation, upload, download, or comparison leaves old backups untouched
and never exposes the new candidate as a completed backup.

Restore accepts only a non-symlink regular file in the protected backup
directory whose name is exactly
`cloudledger-YYYYMMDD-HHMMSS-PID.tar` (or its corresponding numeric ID). Before
extracting, it enforces the archive and member size limits and requires exactly
nine unique regular members: `postgres.dump`, `server.toml`, `compose.env`,
`compose.yml`, `Caddyfile`, `origin-cert.pem`, `origin-key.pem`,
`manifest.json`, and `SHA256SUMS`. Checksums and the custom dump format must be
valid. The archived `compose.env` is parsed with the strict `ops.env` allowlist;
unknown or duplicate keys and unsafe shell syntax are rejected. Its explicit
release tag and four GHCR images must agree and use the currently configured
trusted owner. Fixed ports and certificate paths must remain safe; the archived
`server.toml` must exactly match the toolbox's canonical security template; the
Origin CA certificate and key must match, remain valid, and cover the API SAN;
and the archived Compose and Caddyfile must exactly match the current toolbox's
trusted templates. Compose validation explicitly removes inherited
`CLOUDLEDGER_*` variables before reading the archived environment file.

The weekly restore drill applies the same checks, restores the latest verified
backup into a temporary PostgreSQL database, rejects a remote backup older than
`CLOUDLEDGER_REMOTE_BACKUP_MAX_AGE_HOURS` (72 hours by default), and refuses a
remote filename older than the root-only monotonic checkpoint at
`/var/lib/cloudledger-ops/last-remote-backup`. This detects stale or rolled-back
remote listings after the host has observed a newer verified backup. A new host
establishes the checkpoint only after a full successful drill. The drill checks
that every recorded SQLx
migration succeeded and that the target migration image accepts the schema as
current, checks core tables, verifies the audit chain, and then removes the
temporary database. A failed database removal fails the task and cannot write a
success record. A missing prerequisite or failed check is not treated as a
skipped success. Keep the crypt password outside both the server and backup
package.

The deployment has four principals. Keep their credentials separate.

| Principal | Use | Where its password belongs |
| --- | --- | --- |
| `cloudledger_migration` | Owns schema objects and runs the one-time migration | Deployment secret only |
| `cloudledger_runtime` | Long-running API and admin process | `/etc/cloudledger/server.toml` |
| `cloudledger_bootstrap` | Operations toolbox and one-time role hardening | `/etc/cloudledger/ops.env` only |
| `cloudledger` OS user | Runs the service and reads its private TOML | No database superuser credential |

## 1. Host Requirements

Use a supported 64-bit systemd host (Debian 12, Ubuntu 24.04, RHEL, Rocky, or a
compatible derivative), a public DNS name for the API site, and a Cloudflare
orange-cloud DNS record. Set Cloudflare SSL/TLS to **Full (strict)**. The
CloudLedger origin exposes host port `443` only to the official Cloudflare IPv4
and IPv6 ranges; the toolbox maintains those nftables rules automatically.
There is deliberately no CloudLedger requirement for public port 80: xray or
another existing service may continue to own it. CloudLedger's HTTP listener is
published as `127.0.0.1:18080` for local diagnostics only. The admin listener is
published as `127.0.0.1:8788` and is reached through an SSH tunnel.

The complete-install wizard checks and can install Docker Engine, the Compose
plugin, `rclone`, `curl`, `jq`, `openssl`, `tar`, `nft`, and the distribution's
CA certificates. Production images are pulled from GHCR; the host does not
build CloudLedger images. Docker Desktop, OpenRC-only hosts, and non-systemd
machines are outside this release's support boundary. A remote PostgreSQL
server is supported only when its TLS certificate is trusted and the runtime URL
uses `sslmode=verify-full`.

### Distribution packages

The wizard uses the host's supported package manager and official Docker
repository only when Docker or the Compose plugin is missing. Missing helper
tools are installed from the distribution repository without replacing or
restarting an existing Docker daemon. When adding the official Compose plugin
to an existing Docker installation, the wizard shows the running containers and
requires confirmation. Do not substitute a host PostgreSQL or host Caddy
for the Compose services in a v0.1.4 production deployment; their lifecycle,
health gates, and shared network namespace are managed by the toolbox.

### Developer-only source-build reference

This subsection is for backend development and image maintainers, not for
operating a production host. Production administrators select a reviewed,
explicit GHCR tag through the toolbox and never build on the server.

The repository's Nix shell is the reproducible build path and supplies the
Rust toolchain, compiler, and required Android/Tauri tooling. It is also the
recommended option when the distribution's Rust version is too old.

```bash
git clone --branch <tag> --depth 1 <repository-url> /srv/cloudledger
cd /srv/cloudledger
nix develop path:. -c cargo build -p cloudledger-server --release --locked
```

For a native source build, install Rust from `rustup` plus the distribution's C
compiler, linker, `pkg-config`, Git, curl, and CA certificates. A C compiler is
required by transitive Rust dependencies even though PostgreSQL development
headers are not.

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
. "$HOME/.cargo/env"
cd /srv/cloudledger
cargo build -p cloudledger-server --release --locked
```

The binary is `target/release/cloudledger-server`. A Nix-built binary relies
on the Nix store, so run it through `nix develop ... -c` in the service unit
below or keep the full Nix closure available. A native build should run only on
the same distribution family and glibc baseline as its target host.

## 2. Standalone-Binary Account and Files

This section records the ownership invariants used inside the production
containers and can also be used for developer-only standalone testing. It is
not an alternative production operations interface.

```bash
sudo useradd --system --home-dir /var/lib/cloudledger --create-home \
  --shell /usr/sbin/nologin cloudledger
sudo install -d -o cloudledger -g cloudledger -m 0750 /etc/cloudledger
sudo install -d -o cloudledger -g cloudledger -m 0750 /var/lib/cloudledger
sudo install -o cloudledger -g cloudledger -m 0600 \
  /srv/cloudledger/cloudledger-server.example.toml /etc/cloudledger/server.toml
```

Edit `/etc/cloudledger/server.toml` before starting the service. At minimum:

```toml
[server]
mode = "reverse_proxy"
api_bind_addr = "127.0.0.1:8787"
admin_bind_addr = "127.0.0.1:8788"
public_api_url = "https://api.example.com"
# Required by reverse-proxy validation. The admin listener itself is private
# and accessed over an SSH tunnel.
public_admin_url = "https://api.example.com"
allow_insecure_lan = false
web_login_enabled = false
data_dir = "/var/lib/cloudledger"

[database]
url = "postgres://cloudledger_runtime:REPLACE@127.0.0.1:5432/cloudledger"
auto_migrate = false
max_connections = 10
connect_timeout_seconds = 10
```

Set both Cloudflare Turnstile keys in `[security.turnstile]`. Preserve the
default loopback-only `trusted_proxy_cidrs` and Tauri-only CORS origins. The
first successful start generates the admin path/token and audit keys when they
are empty, then rewrites this private TOML; therefore `cloudledger` must own
the file. Back up the completed TOML with the database. Never store its
contents in Git, a frontend `config.js`, or systemd's journal.

## 3. PostgreSQL

Production administrators create and harden these roles through the numeric
install/security menus. The commands below document the role model for
standalone development and troubleshooting only.

Create an empty database owned by the bootstrap operator, then run the supplied
role bootstrap as that operator. Choose long random passwords and pass
them through your deployment secret system; do not paste live passwords into
shell history.

```bash
sudo -u postgres createuser --superuser cloudledger_bootstrap
sudo -u postgres createdb --owner=cloudledger_bootstrap cloudledger

sudo -u postgres psql -d cloudledger \
  -v database_name=cloudledger \
  -v migration_password='MIGRATION_PASSWORD' \
  -v runtime_password='RUNTIME_PASSWORD' \
  -f /srv/cloudledger/deploy/postgres_roles.sql
```

Replace the placeholders through a protected automation mechanism, then put
only the `cloudledger_runtime` connection URL in `server.toml`. The bootstrap
role is used only by the operations toolbox, and the migration role is
intentionally absent from the systemd service. Existing installations should
use the toolbox's one-time account hardening flow before upgrading.

Apply schema changes before starting or upgrading the service:

```bash
cd /srv/cloudledger
sudo -u cloudledger env \
  CLOUDLEDGER_MIGRATION_DATABASE_URL='postgres://cloudledger_migration:MIGRATION_PASSWORD@127.0.0.1:5432/cloudledger' \
  nix develop path:. -c target/release/cloudledger-server migrate --config /etc/cloudledger/server.toml

sudo -u cloudledger nix develop path:. -c \
  target/release/cloudledger-server audit verify --config /etc/cloudledger/server.toml
```

For native builds, omit `nix develop path:. -c` from the two commands. A
successful migration prints its audit-chain and event counts. A failed audit
verification is a deployment stop condition.

## 4. Standalone-Binary systemd Reference

Create `/etc/systemd/system/cloudledger.service` for a Nix-built checkout:

```ini
[Unit]
Description=CloudLedger backend
After=network-online.target postgresql.service
Wants=network-online.target

[Service]
Type=simple
User=cloudledger
Group=cloudledger
WorkingDirectory=/srv/cloudledger
# This is the standard multi-user Nix location. Use `command -v nix` to
# replace it if your Nix installation uses a different absolute path.
ExecStart=/nix/var/nix/profiles/default/bin/nix develop path:. -c target/release/cloudledger-server --config /etc/cloudledger/server.toml
Restart=on-failure
RestartSec=5
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/var/lib/cloudledger /etc/cloudledger
UMask=0077

[Install]
WantedBy=multi-user.target
```

For a native build, replace `ExecStart` with the absolute path to the binary,
for example `/srv/cloudledger/target/release/cloudledger-server --config
/etc/cloudledger/server.toml`. If PostgreSQL runs in a versioned systemd unit,
replace `postgresql.service` in `After=` with that exact unit.

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now cloudledger
sudo systemctl status cloudledger --no-pager
sudo journalctl -u cloudledger -f
```

Do not expose ports 8787 or 8788 in the host firewall. In the supported Compose
deployment, the host mapping `127.0.0.1:8788:18788` reaches a
`network-anchor` socat relay, which forwards container port `18788` to the
backend's namespace-loopback `127.0.0.1:8788`. The API listener also remains
inside the shared network namespace.

## 5. Cloudflare, Caddy, and Host Ports

Through “Cloudflare 与 HTTPS 证书”, configure the single public API hostname,
then import a Cloudflare Origin CA certificate and its matching private key.
The certificate must cover that hostname (a matching wildcard is acceptable),
must not be expired or within the configured warning period, and must match the
private key. The toolbox stores the pair under `/etc/cloudledger/caddy`, renders
the Caddy environment, validates the Caddyfile, and only then reloads Caddy.

The Cloudflare DNS record must remain proxied and SSL/TLS encryption mode must
be **Full (strict)**. The supplied Caddyfile uses the mounted Origin CA pair,
sets security headers and request limits, proxies the API to
`127.0.0.1:8787` in the shared namespace, and converts the trusted
`CF-Connecting-IP` value to `X-Forwarded-For`.

The fixed host-port contract is:

| Host listener | Owner and exposure |
| --- | --- |
| `127.0.0.1:18080` | `network-anchor` HTTP; local diagnostics only |
| `443` | `network-anchor` HTTPS; nftables accepts only official Cloudflare IPv4/IPv6 sources |
| `127.0.0.1:8788:18788` | Host-loopback admin mapping; `network-anchor` relays `18788` to namespace-loopback `127.0.0.1:8788`; SSH tunnel only |
| `80` | Not allocated or modified by CloudLedger; an existing xray/public-HTTP service is preserved |

The firewall-refresh task retrieves both official Cloudflare lists, builds a
dedicated nftables table, checks the candidate with `nft --check`, and applies
it atomically while retaining the last-good rules. It protects both host input
and Docker-forwarded traffic to `443`; it does not replace the host firewall or
modify SSH, xray, or port 80. Forwarded client headers are trustworthy only
after this direct-origin restriction passes verification.

The firewall service has no `Requires=docker.service` or
`Before=docker.service` relationship, and the installer removes the legacy
Docker drop-in if present. A CloudLedger firewall refresh failure does not
start, stop, restart, delay, or fail unrelated Docker services and containers.

The management UI has no public DNS route. Open an SSH tunnel such as
`ssh -N -L 8788:127.0.0.1:8788 <host>`, then browse to the randomized admin path
on `http://127.0.0.1:8788`.

## 6. Verification and Operations

Use “服务监控与压力查看 → 执行完整健康检查” after every deployment or
upgrade. A successful report requires healthy containers and PostgreSQL,
successful `/health` and `/ready` responses, valid public Cloudflare HTTPS,
valid Origin CA coverage and lifetime, a rejected non-Cloudflare direct-origin
connection, and a successful audit-chain check. Also verify the admin endpoint
through the SSH tunnel; the fixed `/admin` path must return `404`, while the
randomized path from the private TOML is usable.

Turnstile verification checks both the API's enabled status/site key and the
secret itself. The toolbox sends the configured secret with a deliberately
invalid probe response to Cloudflare's HTTPS `siteverify` endpoint. Success for
this configuration probe means Cloudflare rejects only the dummy response and
does not report an invalid or missing secret; an invalid secret or unreachable
Cloudflare endpoint stops deployment verification.

Use “数据备份与恢复 → 立即创建完整备份” before every upgrade. A completed
backup includes `postgres.dump`, `server.toml`, `compose.env`, `compose.yml`,
`Caddyfile`, `origin-cert.pem`, `origin-key.pem`, `manifest.json`, and
`SHA256SUMS`. The database and TOML must remain paired because the TOML holds
the audit HMAC keys and generated admin credentials. A local archive or an
unverified upload is not a successful production backup.

Configure OneDrive and a crypt remote through “OneDrive / rclone 管理”. Store
the crypt password in an offline recovery record because it is intentionally
excluded from the disaster-recovery package. Enable the daily backup timer only
after a real encrypted upload and download verification succeeds, and enable
the weekly restore-test timer only after a real temporary-database drill passes.

For the security model, role restrictions, and rollback rules, see
`docs/security-hardening.md`. For the relational schema, see
`docs/backend-data-model.md`.

## 7. Docker and GitHub Container Registry

Each pushed Git tag runs the backend workflow and publishes the deployment
images to GitHub Container Registry (GHCR). Always select a specific immutable
tag such as `v0.1.4` through “首次安装与部署 → 选择要部署的 CloudLedger
版本” or the numeric upgrade wizard. `latest` is rejected for production.

The workflow uses `deploy/Dockerfile.server`, `deploy/Dockerfile.postgres`,
`deploy/Dockerfile.caddy`, and `deploy/Dockerfile.anchor`; production servers
only pull their outputs. Public packages need no credential. For private
packages, use “配置 GitHub Container Registry” and enter a classic PAT with
`read:packages` at the hidden prompt. The toolbox must never write that PAT to
`ops.env` or logs.

The workflow publishes four version-matched images. Caddy and `network-anchor`
are CloudLedger GHCR images rather than direct references to upstream images.

```text
ghcr.io/<repository-owner>/cloudledger-server:<tag>
ghcr.io/<repository-owner>/cloudledger-postgres:<tag>
ghcr.io/<repository-owner>/cloudledger-caddy:<tag>
ghcr.io/<repository-owner>/cloudledger-network-anchor:<tag>
```

`deploy/docker-compose.yml` defines a stable `network-anchor` container.
PostgreSQL, the dedicated `migration` service, the long-running backend, and
Caddy all share its network namespace and communicate over `127.0.0.1`.
`network-anchor` alone declares the fixed host bindings. Do not add `ports:` to
the other services. Its image includes socat solely to relay container port
`18788` to the backend's loopback-only admin listener on `127.0.0.1:8788`.

The wizard writes all Compose variables only to
`/etc/cloudledger/ops.env`. It fixes the following publication settings instead
of asking the administrator to choose host ports:

```text
CLOUDLEDGER_HTTP_PUBLISH=127.0.0.1:18080:80
CLOUDLEDGER_HTTPS_PUBLISH=443:443
# Fixed in compose.yml, not configurable in ops.env:
127.0.0.1:8788:18788
```

The generated `server.toml` uses the runtime PostgreSQL role, loopback API and
admin bind addresses, `auto_migrate = false`, the selected public API URL, and
the supplied Turnstile site/secret pair. Deployment verifies the secret against
Cloudflare `siteverify`; a syntactically valid but rejected secret cannot pass.
The bootstrap credential remains only in `ops.env`; the migration credential is
injected only into the one-shot Compose `migration` profile; the backend
receives only the minimum-privilege runtime credential.

The complete-install and upgrade wizards enforce this order:

1. Check host requirements, stage assets, configure rclone crypt, and validate
   four same-owner GHCR images with one explicit tag.
2. Check whether host port `443` is free or already belongs to this deployment;
   abort before changing the firewall when another process or container owns it.
3. Pull the server, PostgreSQL, Caddy, and `network-anchor` images.
4. Start `network-anchor` and PostgreSQL only, then build and atomically apply
   the Cloudflare-only `443` rules without a Docker service requirement.
5. Wait for database health and verify the bootstrap, migration, and runtime
   role properties. Run the one-shot `migration` service through the Compose
   profile, then verify migration state and the audit chain.
6. Start the backend and require local `/health` and `/ready` checks to pass.
7. Validate and start Caddy, check public Cloudflare `/health` and `/ready`,
   verify the Turnstile site key and secret through `siteverify`, then recheck
   the installed firewall table.
8. Install the systemd units and enable only the base health/firewall timers.
9. Create a real encrypted backup, verify its downloaded remote copy, and pass
   a real temporary-database restore drill before enabling backup and restore
   timers.

An upgrade creates and verifies the paired database/configuration backup before
stopping the old entrypoint or backend. Failure before schema migration can
restore the prior image tag automatically. Failure after a successful migration
must not blindly start an older binary; recovery requires the database,
`server.toml`, Compose configuration, and Origin CA files from the matching
verified backup.
