# CloudLedger Backend Deployment

This guide deploys `cloudledger-server` on a Linux host with PostgreSQL and
Caddy. It is for a production Tauri-client installation: API and admin
listeners stay on loopback, Caddy owns public HTTPS, and PostgreSQL is the
authoritative store.

The deployment has three principals. Keep their credentials separate.

| Principal | Use | Where its password belongs |
| --- | --- | --- |
| `cloudledger_migration` | Owns schema objects and runs the one-time migration | Deployment secret only |
| `cloudledger_runtime` | Long-running API and admin process | `/etc/cloudledger/server.toml` |
| `cloudledger` OS user | Runs the service and reads its private TOML | No database superuser credential |

## 1. Host Requirements

Use a supported 64-bit Linux host, a public DNS name for each of the API and
admin sites, and an inbound firewall allowance for TCP 80 and 443. PostgreSQL,
Caddy, and the CloudLedger server can run on one host. The database may be
remote only when its TLS certificate is trusted and the runtime URL uses
`sslmode=verify-full`.

CloudLedger does not currently publish a portable generic-Linux server binary.
A binary built with Nix must retain its Nix runtime closure and be run through
the Nix environment. A natively built glibc binary must not be copied to Alpine
Linux; use the Nix deployment option or build a musl-compatible binary
specifically for that host. Before installing any prebuilt binary, inspect its
actual runtime dependencies with `ldd cloudledger-server`. The server uses
Rustls for outgoing HTTPS and bundled SQLite for the client-cache crate; it
does not require OpenSSL or SQLite shared libraries at runtime.

### Distribution packages

Install the **runtime** packages on every host. Install the **native build**
packages only when compiling CloudLedger on that host. Package names are for
current mainstream releases; use the distribution's supported PostgreSQL major
version when a versioned package name is required.

| Distribution | Runtime packages | Native source-build packages |
| --- | --- | --- |
| Debian 12 / Ubuntu 24.04 | `postgresql postgresql-client caddy ca-certificates curl` | `build-essential pkg-config git curl ca-certificates` |
| Fedora / Rocky / RHEL | `postgresql-server postgresql caddy ca-certificates curl` | `gcc gcc-c++ make pkgconf-pkg-config git curl ca-certificates` |
| Arch Linux | `postgresql caddy ca-certificates curl` | `base-devel pkgconf git curl ca-certificates` |
| openSUSE Leap / Tumbleweed | `postgresql-server postgresql caddy ca-certificates curl` | `gcc gcc-c++ make pkg-config git curl ca-certificates` |
| Alpine | `postgresql postgresql-client caddy ca-certificates curl` | `build-base pkgconf git curl ca-certificates` |

Examples for the first four distributions:

```bash
# Debian / Ubuntu
sudo apt update
sudo apt install -y postgresql postgresql-client caddy ca-certificates curl

# Fedora / Rocky / RHEL
sudo dnf install -y postgresql-server postgresql caddy ca-certificates curl

# Arch
sudo pacman -Syu --needed postgresql caddy ca-certificates curl

# openSUSE
sudo zypper install -y postgresql-server postgresql caddy ca-certificates curl
```

On RHEL-derived distributions, Caddy may be provided by EPEL or Caddy's
official repository rather than the base repository. Initialize PostgreSQL on
Fedora, RHEL, Rocky, and openSUSE according to the installed major version,
then enable it. Debian and Ubuntu initialize the default cluster during package
installation.

```bash
# Typical systemd hosts; adjust the PostgreSQL unit to the installed major.
sudo systemctl enable --now postgresql
sudo systemctl enable --now caddy
```

Alpine uses OpenRC instead of systemd. It is supported for PostgreSQL and
Caddy, but the CI-produced glibc server binary is not. Use the Nix approach or
produce and test a native Alpine build before placing it behind a supervisor.

### Build the server

Pin deployment to a reviewed Git tag. Do not deploy an uncommitted checkout.

The repository's Nix shell is the reproducible build path and supplies the
Rust toolchain, compiler, and required Android/Tauri tooling. It is also the
recommended option when the distribution's Rust version is too old.

```bash
git clone --branch <tag> --depth 1 <repository-url> /srv/cloudledger
cd /srv/cloudledger
nix develop path:. -c cargo build -p cloudledger-server --release --locked
```

For a native source build, install Rust from `rustup`, then use the build
packages in the table above. A C compiler is required by transitive Rust
dependencies even though PostgreSQL development headers are not.

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

## 2. Operating-System Account and Files

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
public_admin_url = "https://admin.example.com"
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

Create an empty database, then run the supplied role bootstrap as the
PostgreSQL superuser or database owner. Choose long random passwords and pass
them through your deployment secret system; do not paste live passwords into
shell history.

```bash
sudo -u postgres createuser --no-superuser --no-createdb --no-createrole cloudledger_migration
sudo -u postgres createdb --owner=cloudledger_migration cloudledger

sudo -u postgres psql -d cloudledger \
  -v database_name=cloudledger \
  -v migration_password='MIGRATION_PASSWORD' \
  -v runtime_password='RUNTIME_PASSWORD' \
  -f /srv/cloudledger/deploy/postgres_roles.sql
```

Replace the placeholders through a protected automation mechanism, then put
only the `cloudledger_runtime` connection URL in `server.toml`. The migration
role is intentionally absent from the systemd service.

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

## 4. systemd Service

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

Do not expose ports 8787 or 8788 in the host firewall. Only Caddy should bind
the public network ports.

## 5. Caddy HTTPS Proxy

Copy `deploy/Caddyfile` to `/etc/caddy/Caddyfile`, replacing the two hostnames
through Caddy environment variables. On systemd systems, create the drop-in
directory and `/etc/systemd/system/caddy.service.d/cloudledger.conf`:

```ini
[Service]
Environment=CLOUDLEDGER_API_DOMAIN=api.example.com
Environment=CLOUDLEDGER_ADMIN_DOMAIN=admin.example.com
```

Then validate and reload it:

```bash
sudo systemctl daemon-reload
sudo caddy validate --config /etc/caddy/Caddyfile
sudo systemctl reload caddy
```

The supplied Caddyfile obtains certificates automatically, sets security
headers and request limits, proxies API traffic to `127.0.0.1:8787`, proxies
the admin service to `127.0.0.1:8788`, and sets trusted forwarding headers.
Public DNS must point both names to this host before certificate issuance.

## 6. Verification and Operations

After every deployment, run these checks from the server and a separate network
client:

```bash
sudo ss -ltnp | rg ':8787|:8788'
curl --fail --show-error https://api.example.com/ready
sudo -u cloudledger /srv/cloudledger/target/release/cloudledger-server \
  audit verify --config /etc/cloudledger/server.toml
```

The first command must show loopback addresses only. Also confirm the admin
site is reachable through HTTPS but the fixed `/admin` path returns `404`; use
the randomized path generated in the private TOML instead. Test an invalid
login, confirm it returns an application response rather than a browser network
failure, and verify Caddy rejects request bodies larger than 64 KiB.

Back up PostgreSQL and `/etc/cloudledger/server.toml` together. The TOML holds
the audit HMAC keys and the generated admin credentials; restoring the database
without matching audit keys prevents historical audit verification. Before an
upgrade, back up both, stop all service instances, build the new tagged source,
run the one-time migration with the migration credential, verify the audit
chain, then restart the runtime service. Never restart an old binary after a
schema/security migration.

For the security model, role restrictions, and rollback rules, see
`docs/security-hardening.md`. For the relational schema, see
`docs/backend-data-model.md`.

## 7. Docker and GitHub Container Registry

Each pushed Git tag runs the backend workflow and publishes the deployment
images to GitHub Container Registry (GHCR). The `latest` tag follows the most
recently pushed Git tag; deploy a specific immutable tag in production.

The workflow uses `deploy/Dockerfile.server`, builds only
`cloudledger-server`, and pushes with the repository `GITHUB_TOKEN`. The first
published package may inherit the repository visibility; change it in the
GitHub Packages settings when public pulls are required. For a private package,
authenticate on the deployment host with a GitHub classic personal access token
that has `read:packages`:

```bash
export GHCR_USER=<github-user-or-organization>
export GHCR_TOKEN=<read-packages-token>
printf '%s' "$GHCR_TOKEN" | docker login ghcr.io --username "$GHCR_USER" --password-stdin
```

The workflow publishes two images. Caddy uses its official image directly.

```text
ghcr.io/<repository-owner>/cloudledger-server:<tag>
ghcr.io/<repository-owner>/cloudledger-postgres:<tag>
```

`deploy/docker-compose.yml` deploys the server, PostgreSQL, and Caddy together.
The PostgreSQL and Caddy containers share the server container's network
namespace. Consequently, both Caddy-to-server and server-to-PostgreSQL traffic
use `127.0.0.1`, preserving CloudLedger's production loopback requirement.
Only the `cloudledger` service declares public ports; Caddy binds those ports
inside the shared namespace. Do not add backend, PostgreSQL, or Caddy `ports:`
entries. This deployment requires a Linux Docker Engine; it is not for Docker
Desktop.

Prepare a dedicated deployment directory and the private backend config. In
`/etc/cloudledger/server.toml`, set the runtime database URL to
`postgres://cloudledger_runtime:<runtime-password>@127.0.0.1:5432/cloudledger`.
Use the same runtime password in `.env`. Keep `mode = "reverse_proxy"`, both
backend bind addresses on loopback, `auto_migrate = false`, and the required
Turnstile keys.

```bash
sudo install -d -o 10001 -g 10001 -m 0750 /etc/cloudledger
sudo install -o 10001 -g 10001 -m 0600 \
  /srv/cloudledger/cloudledger-server.example.toml /etc/cloudledger/server.toml

sudo install -d -m 0750 /opt/cloudledger
sudo cp /srv/cloudledger/deploy/docker-compose.yml /opt/cloudledger/compose.yml
sudo cp /srv/cloudledger/deploy/Caddyfile /opt/cloudledger/Caddyfile
sudo tee /opt/cloudledger/.env >/dev/null <<'EOF'
CLOUDLEDGER_SERVER_IMAGE=ghcr.io/<repository-owner>/cloudledger-server:<tag>
CLOUDLEDGER_POSTGRES_IMAGE=ghcr.io/<repository-owner>/cloudledger-postgres:<tag>
CLOUDLEDGER_MIGRATION_DB_PASSWORD=<long-random-migration-password>
CLOUDLEDGER_RUNTIME_DB_PASSWORD=<long-random-runtime-password>
CLOUDLEDGER_API_DOMAIN=api.example.com
CLOUDLEDGER_ADMIN_DOMAIN=admin.example.com
EOF
sudo chmod 0600 /opt/cloudledger/.env
```

The PostgreSQL image creates the database, `cloudledger_migration`, and
`cloudledger_runtime` roles on the first initialization of `postgres-data`.
Those passwords are initialization inputs; changing `.env` later does not
change an existing database role. Rotate them through PostgreSQL deliberately.

Start PostgreSQL and the server, then run the one-time migration using the
migration secret from `.env`. The server will restart until PostgreSQL has
finished initialization. Start Caddy after migration succeeds.

```bash
cd /opt/cloudledger
sudo docker compose --env-file .env up -d postgres cloudledger
sudo docker compose --env-file .env exec -e \
  CLOUDLEDGER_MIGRATION_DATABASE_URL='postgres://cloudledger_migration:<migration-password>@127.0.0.1:5432/cloudledger' \
  cloudledger migrate --config /etc/cloudledger/server.toml
sudo docker compose --env-file .env up -d caddy
sudo docker compose --env-file .env logs -f cloudledger caddy
```

Replace `<migration-password>` at execution time from a protected secret store;
do not commit it or include it in the Compose definition. After the first
server start, back up the rewritten TOML because it contains generated audit
keys and admin credentials.

Verify the complete deployment:

```bash
sudo docker compose --env-file .env ps
sudo docker compose --env-file .env exec cloudledger \
  audit verify --config /etc/cloudledger/server.toml
curl --fail --show-error https://api.example.com/ready
```

To upgrade, back up the PostgreSQL volume and the private TOML, update both
GHCR image tags in `/opt/cloudledger/.env`, pull the images, run any migration,
verify the audit chain, and recreate the stack. Do not roll back an image after
a schema migration without restoring the matching database and audit-key
backup.

```bash
cd /opt/cloudledger
sudo docker compose --env-file .env pull
sudo docker compose --env-file .env up -d --remove-orphans
```
