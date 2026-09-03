#!/usr/bin/env bash
set -Eeuo pipefail

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
OPS="$ROOT/deploy/cloudledger-ops.sh"
MOCK_COMMAND="$ROOT/deploy/tests/mock-command.sh"
MOCK_READ="$ROOT/deploy/tests/mock-read.bash"
ORIGINAL_PATH=$PATH
REAL_RM=$(PATH="$ORIGINAL_PATH" command -v rm)
REAL_BASH=$(PATH="$ORIGINAL_PATH" command -v bash)
SUITE_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/cloudledger-ops-test.XXXXXX")

cleanup() {
  if [[ ${CLOUDLEDGER_TEST_KEEP_TMP:-0} == 1 ]]; then
    printf 'kept test workspace: %s\n' "$SUITE_ROOT" >&2
    return
  fi
  case "$SUITE_ROOT" in
    "${TMPDIR:-/tmp}"/cloudledger-ops-test.*) rm -rf -- "$SUITE_ROOT" ;;
  esac
}
trap cleanup EXIT HUP INT TERM

fail_test() {
  printf 'FAIL: %s\n' "$*" >&2
  exit 1
}

assert_contains() {
  local needle=$1 file=$2
  grep -Fq -- "$needle" "$file" || fail_test "missing '$needle' in $file"
}

assert_not_contains() {
  local needle=$1 file=$2
  if grep -Fq -- "$needle" "$file"; then
    fail_test "unexpected '$needle' in $file"
  fi
}

assert_file_mode() {
  local expected=$1 file=$2 actual
  actual=$(stat -c '%a' "$file")
  [[ "$actual" == "$expected" ]] || fail_test "$file has mode $actual, expected $expected"
}

assert_trace_order() {
  local trace=$1 event line cursor=0
  shift
  for event in "$@"; do
    line=$(awk -v event="$event" -v cursor="$cursor" 'NR > cursor && $0 == event { print NR; exit }' "$trace")
    [[ -n "$line" ]] || fail_test "trace event '$event' missing after line $cursor"
    cursor=$line
  done
}

assert_secret_not_logged() {
  local secret=$1
  shift
  [[ -n "$secret" ]] || fail_test 'empty secret supplied to log assertion'
  local file
  for file in "$@"; do
    [[ -f "$file" ]] || continue
    if grep -Fq -- "$secret" "$file"; then
      fail_test "secret leaked into $file"
    fi
  done
}

FIXTURE_DIR="$SUITE_ROOT/fixtures"
mkdir -p "$FIXTURE_DIR"
openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
  -subj '/CN=cloudledger-test.513921.xyz' \
  -addext 'subjectAltName=DNS:cloudledger-test.513921.xyz' \
  -keyout "$FIXTURE_DIR/origin-key.pem" -out "$FIXTURE_DIR/origin-cert.pem" \
  >/dev/null 2>&1

setup_case() {
  local name=$1 include_host_pg_dump=${2:-yes} command_name
  CASE_ROOT="$SUITE_ROOT/$name"
  MOCK_BIN="$CASE_ROOT/bin"
  mkdir -p "$MOCK_BIN" "$CASE_ROOT/state" "$CASE_ROOT/config/caddy" \
    "$CASE_ROOT/deploy" "$CASE_ROOT/systemd" "$CASE_ROOT/remote"
  : >"$CASE_ROOT/trace"
  : >"$CASE_ROOT/read.trace"

  for command_name in docker curl rclone nft systemctl pg_restore psql createdb dropdb rm \
    caddy dig ss systemd-analyze apt-get dnf yum ps; do
    ln -s "$MOCK_COMMAND" "$MOCK_BIN/$command_name"
  done
  if [[ "$include_host_pg_dump" == yes ]]; then
    ln -s "$MOCK_COMMAND" "$MOCK_BIN/pg_dump"
  fi

  export PATH="$MOCK_BIN:$ORIGINAL_PATH"
  export CLOUDLEDGER_ALLOW_NONROOT=1
  export NO_COLOR=1
  export CLOUDLEDGER_OPS_STATE_DIR="$CASE_ROOT/state"
  export CLOUDLEDGER_BACKUP_DIR="$CASE_ROOT/state/backups"
  export CLOUDLEDGER_FIREWALL_DIR="$CASE_ROOT/state/firewall"
  export CLOUDLEDGER_OPS_LOCK="$CASE_ROOT/state/ops.lock"
  export CLOUDLEDGER_OPS_ENV="$CASE_ROOT/config/ops.env"
  export CLOUDLEDGER_SERVER_CONFIG="$CASE_ROOT/config/server.toml"
  export CLOUDLEDGER_CERT_DIR="$CASE_ROOT/config/caddy"
  export CLOUDLEDGER_RCLONE_CONFIG="$CASE_ROOT/config/rclone.conf"
  export CLOUDLEDGER_DEPLOY_DIR="$CASE_ROOT/deploy"
  export CLOUDLEDGER_COMPOSE_FILE="$CASE_ROOT/deploy/compose.yml"
  export CLOUDLEDGER_SYSTEMD_DIR="$CASE_ROOT/systemd"
  export CLOUDLEDGER_TEST_TRACE="$CASE_ROOT/trace"
  export CLOUDLEDGER_TEST_READ_TRACE="$CASE_ROOT/read.trace"
  export CLOUDLEDGER_TEST_REMOTE_DIR="$CASE_ROOT/remote"
  export CLOUDLEDGER_TEST_NFT_STATE="$CASE_ROOT/nft.state"
  export CLOUDLEDGER_TEST_PG_RESTORE_COUNT_FILE="$CASE_ROOT/pg-restore-list.count"
  export CLOUDLEDGER_TEST_PG_RESTORE_APPLY_COUNT_FILE="$CASE_ROOT/pg-restore-apply.count"
  export CLOUDLEDGER_TEST_DROPDB_COUNT_FILE="$CASE_ROOT/dropdb.count"
  export CLOUDLEDGER_TEST_CERT_FILE="$FIXTURE_DIR/origin-cert.pem"
  export CLOUDLEDGER_TEST_KEY_FILE="$FIXTURE_DIR/origin-key.pem"
  export CLOUDLEDGER_TEST_REAL_RM="$REAL_RM"
  export CLOUDLEDGER_TEST_RM_FAILED_PATH_FILE="$CASE_ROOT/rm-failed-paths"
  export CLOUDLEDGER_TEST_API_DOMAIN='cloudledger-test.513921.xyz'
  export CLOUDLEDGER_TEST_GHCR_OWNER='cloudledger'
  export CLOUDLEDGER_TEST_TAG='v0.1.5'
  export CLOUDLEDGER_TEST_GHCR_PAT='ops-test-ghcr-pat-do-not-log'
  export CLOUDLEDGER_TEST_TURNSTILE_SITE_KEY='ops-test-turnstile-site'
  export CLOUDLEDGER_TEST_TURNSTILE_SECRET='ops-test-turnstile-secret-do-not-log'
  export CLOUDLEDGER_TEST_RCLONE_PASSWORD='ops-test-rclone-password-do-not-log'
  export CLOUDLEDGER_TEST_CRYPT_PATH='cloudledger-crypt:CloudLedger/backups'
  export CLOUDLEDGER_TEST_PG_DUMP_MODE=ok
  export CLOUDLEDGER_TEST_PG_RESTORE_MODE=ok
  export CLOUDLEDGER_TEST_DROPDB_MODE=ok
  export CLOUDLEDGER_TEST_RCLONE_MODE=ok
  export CLOUDLEDGER_TEST_CLOUDFLARE_IP_MODE=valid
  export CLOUDLEDGER_TEST_NFT_TABLE_MODE=valid
  export CLOUDLEDGER_TEST_TURNSTILE_PROBE_MODE=valid-secret
  unset BASH_ENV CLOUDLEDGER_RCLONE_REMOTE CLOUDLEDGER_API_URL CLOUDLEDGER_TEST_COMPOSE_FAIL_AT \
    CLOUDLEDGER_TEST_SIGNAL_ON_PG_RESTORE CLOUDLEDGER_TEST_SIGNAL_ON_COMPOSE \
    CLOUDLEDGER_TEST_SIGNAL_ON_RCLONE CLOUDLEDGER_TEST_ANCHOR_ID CLOUDLEDGER_TEST_HTTPS_DOCKER_OWNER \
    CLOUDLEDGER_TEST_HTTPS_LISTENER CLOUDLEDGER_TEST_ASSERT_CLEAN_COMPOSE_ENV \
    CLOUDLEDGER_TEST_MIGRATION_SUMMARY CLOUDLEDGER_TEST_RM_FAIL_PATTERN \
    CLOUDLEDGER_TEST_NFT_CAPTURE CLOUDLEDGER_TEST_NFT_PRETTY_PRIORITY \
    CLOUDLEDGER_TEST_MISSING_RESTORE_TABLE
}

cleanup_injected_rm_paths() {
  local path
  [[ -f ${CLOUDLEDGER_TEST_RM_FAILED_PATH_FILE:-} ]] || return 0
  while IFS= read -r path; do
    case "$path" in
      "$CASE_ROOT"/*|"${TMPDIR:-/tmp}"/cloudledger-*) "$REAL_RM" -rf -- "$path" ;;
      *) fail_test "refusing to clean unexpected injected rm path: $path" ;;
    esac
  done <"$CLOUDLEDGER_TEST_RM_FAILED_PATH_FILE"
  : >"$CLOUDLEDGER_TEST_RM_FAILED_PATH_FILE"
}

seed_rclone_config() {
  printf '%s\n' \
    '[cloudledger-crypt]' \
    'type = crypt' \
    'remote = onedrive:CloudLedger' \
    "password = $CLOUDLEDGER_TEST_RCLONE_PASSWORD" \
    >"$CLOUDLEDGER_RCLONE_CONFIG"
  chmod 600 "$CLOUDLEDGER_RCLONE_CONFIG"
}

seed_backup_fixture() {
  local remote=${1:-no}
  # This fixture intentionally models the immediately previous release so the
  # upgrade tests exercise v0.1.4 -> v0.1.5 and its rollback boundary.
  cp -- "$ROOT/deploy/docker-compose.yml" "$CLOUDLEDGER_DEPLOY_DIR/compose.yml"
  cp -- "$ROOT/deploy/Caddyfile" "$CLOUDLEDGER_DEPLOY_DIR/Caddyfile"
  cp -- "$ROOT/deploy/postgres_roles.sql" "$CLOUDLEDGER_DEPLOY_DIR/postgres_roles.sql"
  cp -- "$OPS" "$CLOUDLEDGER_DEPLOY_DIR/cloudledger-ops.sh"
  chmod 755 "$CLOUDLEDGER_DEPLOY_DIR/cloudledger-ops.sh"
  cp -- "$FIXTURE_DIR/origin-cert.pem" "$CLOUDLEDGER_CERT_DIR/origin-cert.pem"
  cp -- "$FIXTURE_DIR/origin-key.pem" "$CLOUDLEDGER_CERT_DIR/origin-key.pem"
  printf '%s\n' \
    '[server]' \
    'mode = "reverse_proxy"' \
    'api_bind_addr = "127.0.0.1:8787"' \
    'admin_bind_addr = "127.0.0.1:8788"' \
    'public_api_url = "https://cloudledger-test.513921.xyz"' \
    'public_admin_url = "https://cloudledger-test.513921.xyz"' \
    'allow_insecure_lan = false' \
    'web_login_enabled = false' \
    'client_version = "0.1.4"' \
    'min_supported_client_version = "0.1.4"' \
    'client_download_url = "https://github.com/dahai9/CloudLedger/releases/latest"' \
    'data_dir = "/var/lib/cloudledger"' \
    '' \
    '[database]' \
    'url = "postgres://cloudledger_runtime:test-runtime-password@127.0.0.1:5432/cloudledger"' \
    'auto_migrate = false' \
    'max_connections = 10' \
    'connect_timeout_seconds = 10' \
    '' \
    '[admin]' \
    'path = "manage-0123456789abcdef0123456789abcdef"' \
    'token = "admin_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"' \
    '' \
    '[security.login]' \
    'turnstile_after_failures = 3' \
    'max_failures_per_login = 5' \
    'max_failures_per_ip = 20' \
    'window_seconds = 900' \
    'lockout_seconds = 900' \
    '' \
    '[security.turnstile]' \
    'site_key = "ops-test-turnstile-site"' \
    'secret_key = "ops-test-turnstile-secret-do-not-log"' \
    'verify_url = "https://challenges.cloudflare.com/turnstile/v0/siteverify"' \
    '' \
    '[security.network]' \
    'trusted_proxy_cidrs = ["127.0.0.1/32", "::1/128"]' \
    'cors_allowed_origins = ["tauri://localhost", "https://tauri.localhost"]' \
    '' \
    '[security.audit]' \
    'key_id = "audit-0123456789abcdef01234567"' \
    'hmac_key = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"' \
    'identifier_hmac_key = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"' \
    >"$CLOUDLEDGER_SERVER_CONFIG"
  {
    printf '%s\n' \
      'CLOUDLEDGER_GHCR_OWNER=cloudledger' \
      'CLOUDLEDGER_SERVER_IMAGE=ghcr.io/cloudledger/cloudledger-server:v0.1.4' \
      'CLOUDLEDGER_POSTGRES_IMAGE=ghcr.io/cloudledger/cloudledger-postgres:v0.1.4' \
      'CLOUDLEDGER_CADDY_IMAGE=ghcr.io/cloudledger/cloudledger-caddy:v0.1.4' \
      'CLOUDLEDGER_ANCHOR_IMAGE=ghcr.io/cloudledger/cloudledger-network-anchor:v0.1.4' \
      'CLOUDLEDGER_RELEASE_TAG=v0.1.4' \
      'CLOUDLEDGER_CLIENT_VERSION=0.1.4' \
      'CLOUDLEDGER_MIN_SUPPORTED_CLIENT_VERSION=0.1.4' \
      'CLOUDLEDGER_CLIENT_DOWNLOAD_URL=https://github.com/dahai9/CloudLedger/releases/latest' \
      'CLOUDLEDGER_API_DOMAIN=cloudledger-test.513921.xyz' \
      'CLOUDLEDGER_HTTP_PUBLISH=127.0.0.1:18080:80' \
      'CLOUDLEDGER_HTTPS_PUBLISH=443:443' \
      'CLOUDLEDGER_TURNSTILE_SITE_KEY=ops-test-turnstile-site' \
      'CLOUDLEDGER_TURNSTILE_SECRET_KEY=ops-test-turnstile-secret-do-not-log' \
      'CLOUDLEDGER_ADMIN_PATH=manage-0123456789abcdef0123456789abcdef' \
      'CLOUDLEDGER_ADMIN_TOKEN=admin_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef' \
      'CLOUDLEDGER_AUDIT_KEY_ID=audit-0123456789abcdef01234567' \
      'CLOUDLEDGER_AUDIT_HMAC_KEY=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' \
      'CLOUDLEDGER_AUDIT_IDENTIFIER_HMAC_KEY=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb' \
      'CLOUDLEDGER_MIGRATION_DB_PASSWORD=test-migration-password' \
      'CLOUDLEDGER_RUNTIME_DB_PASSWORD=test-runtime-password' \
      'CLOUDLEDGER_BOOTSTRAP_DB_PASSWORD=test-bootstrap-password' \
      'CLOUDLEDGER_BOOTSTRAP_DATABASE_URL=postgres://cloudledger_bootstrap:test-bootstrap-password@127.0.0.1:5432/cloudledger' \
      'CLOUDLEDGER_MIGRATION_DATABASE_URL=postgres://cloudledger_migration:test-migration-password@127.0.0.1:5432/cloudledger' \
      "CLOUDLEDGER_CADDY_ORIGIN_CERT_PATH=$CLOUDLEDGER_CERT_DIR/origin-cert.pem" \
      "CLOUDLEDGER_CADDY_ORIGIN_KEY_PATH=$CLOUDLEDGER_CERT_DIR/origin-key.pem"
    if [[ "$remote" == yes ]]; then
      printf 'CLOUDLEDGER_RCLONE_REMOTE=cloudledger-crypt:CloudLedger/backups\n'
    fi
  } >"$CLOUDLEDGER_OPS_ENV"
  chmod 600 "$CLOUDLEDGER_OPS_ENV" "$CLOUDLEDGER_SERVER_CONFIG" "$CLOUDLEDGER_CERT_DIR/origin-key.pem"
  seed_rclone_config
}

seed_legacy_upgrade_fixture() {
  seed_backup_fixture yes
  cp -- "$ROOT/deploy/legacy/compose-v0.1.3.yml" "$CLOUDLEDGER_DEPLOY_DIR/compose.yml"
  mv -- "$CLOUDLEDGER_CERT_DIR/origin-cert.pem" "$CLOUDLEDGER_CERT_DIR/cloudledger-test-origin.crt"
  mv -- "$CLOUDLEDGER_CERT_DIR/origin-key.pem" "$CLOUDLEDGER_CERT_DIR/cloudledger-test-origin.key"
  sed -i \
    -e '/^CLOUDLEDGER_GHCR_OWNER=/d' \
    -e '/^CLOUDLEDGER_CADDY_IMAGE=/d' \
    -e '/^CLOUDLEDGER_ANCHOR_IMAGE=/d' \
    -e '/^CLOUDLEDGER_RELEASE_TAG=/d' \
    -e '/^CLOUDLEDGER_HTTP_PUBLISH=/d' \
    -e '/^CLOUDLEDGER_HTTPS_PUBLISH=/d' \
    -e '/^CLOUDLEDGER_ADMIN_PATH=/d' \
    -e '/^CLOUDLEDGER_ADMIN_TOKEN=/d' \
    -e '/^CLOUDLEDGER_AUDIT_KEY_ID=/d' \
    -e '/^CLOUDLEDGER_AUDIT_HMAC_KEY=/d' \
    -e '/^CLOUDLEDGER_AUDIT_IDENTIFIER_HMAC_KEY=/d' \
    -e 's#^CLOUDLEDGER_SERVER_IMAGE=.*#CLOUDLEDGER_SERVER_IMAGE=ghcr.io/cloudledger/cloudledger-server:v0.1.3#' \
    -e 's#^CLOUDLEDGER_POSTGRES_IMAGE=.*#CLOUDLEDGER_POSTGRES_IMAGE=ghcr.io/cloudledger/cloudledger-postgres:v0.1.3#' \
    -e "s#^CLOUDLEDGER_CADDY_ORIGIN_CERT_PATH=.*#CLOUDLEDGER_CADDY_ORIGIN_CERT_PATH=$CLOUDLEDGER_CERT_DIR/cloudledger-test-origin.crt#" \
    -e "s#^CLOUDLEDGER_CADDY_ORIGIN_KEY_PATH=.*#CLOUDLEDGER_CADDY_ORIGIN_KEY_PATH=$CLOUDLEDGER_CERT_DIR/cloudledger-test-origin.key#" \
    "$CLOUDLEDGER_OPS_ENV"
  printf '%s\n' \
    'CLOUDLEDGER_HTTP_HOST_PORT=18080' \
    'CLOUDLEDGER_HTTPS_HOST_PORT=443' \
    'CLOUDLEDGER_ADMIN_TUNNEL_PORT=8788' \
    >>"$CLOUDLEDGER_OPS_ENV"
}

latest_archive() {
  find "$CLOUDLEDGER_BACKUP_DIR" -maxdepth 1 -type f -name 'cloudledger-*.tar' -print 2>/dev/null \
    | sort | tail -n1
}

assert_no_pending_backup() {
  if find "$CLOUDLEDGER_BACKUP_DIR" -maxdepth 1 -type f \
    \( -name '*.new' -o -name '.*.new' \) -print 2>/dev/null | grep -q .; then
    fail_test 'a hidden .new backup candidate was left behind'
  fi
}

count_final_backups() {
  find "$CLOUDLEDGER_BACKUP_DIR" -maxdepth 1 -type f -name 'cloudledger-*.tar' -print 2>/dev/null \
    | wc -l
}

assert_valid_archive() {
  local archive=$1 extract_dir=$2
  [[ -n "$archive" && -s "$archive" ]] || fail_test 'backup archive is missing or empty'
  mkdir -p "$extract_dir"
  tar -xf "$archive" -C "$extract_dir"
  [[ -s "$extract_dir/postgres.dump" ]] || fail_test 'postgres.dump is missing or empty'
  assert_contains 'PGDMP' "$extract_dir/postgres.dump"
  for required in server.toml compose.env compose.yml Caddyfile origin-cert.pem origin-key.pem manifest.json SHA256SUMS; do
    [[ -s "$extract_dir/$required" ]] || fail_test "archive is missing $required"
  done
  (cd "$extract_dir" && sha256sum -c SHA256SUMS >/dev/null) \
    || fail_test 'archive checksum validation failed'
}

repack_backup_payload() {
  local payload=$1 archive=$2 name id manifest
  name=${archive##*/}
  id=${name#cloudledger-}
  id=${id%.tar}
  [[ "$id" =~ ^[0-9]{8}-[0-9]{6}-[0-9]+$ ]] || fail_test "invalid repack archive name: $name"
  manifest="$payload/.manifest.json.new"
  jq --arg id "$id" '.id = $id' "$payload/manifest.json" >"$manifest"
  mv -f -- "$manifest" "$payload/manifest.json"
  (
    cd "$payload"
    sha256sum postgres.dump server.toml compose.env compose.yml Caddyfile \
      origin-cert.pem origin-key.pem manifest.json >SHA256SUMS
  )
  tar -C "$payload" -cf "$archive" .
}

verify_candidate_bundle_rejected() {
  local archive=$1 label=$2 output
  output="$CASE_ROOT/$label.out"
  : >"$CLOUDLEDGER_TEST_TRACE"
  printf '4\n4\n%s\n0\n0\n' "$(basename "$archive")" | "$OPS" >"$output" 2>&1 || true
  assert_contains '备份部署配置未通过当前工具的信任校验' "$output"
  assert_not_contains '恢复演练通过' "$output"
}

test_numeric_menus() {
  setup_case numeric-menus
  {
    printf '\n99\n'
    for choice in $(seq 1 11); do printf '%s\n0\n' "$choice"; done
    printf '0\n'
  } | "$OPS" >"$CASE_ROOT/menu.out" 2>&1

  assert_contains '请输入菜单中的数字' "$CASE_ROOT/menu.out"
  assert_contains '执行全部安装向导' "$CASE_ROOT/menu.out"
  assert_contains '拉取当前配置的镜像' "$CASE_ROOT/menu.out"
  assert_contains '升级到指定版本' "$CASE_ROOT/menu.out"
  assert_contains '执行临时数据库恢复演练' "$CASE_ROOT/menu.out"
  assert_contains '进入实时监控模式' "$CASE_ROOT/menu.out"
  assert_contains '检查 Cloudflare 代理访问' "$CASE_ROOT/menu.out"
  assert_contains '显示灾难恢复所需信息' "$CASE_ROOT/menu.out"
  assert_contains '导出脱敏诊断配置' "$CASE_ROOT/menu.out"
  assert_contains '导出脱敏诊断报告' "$CASE_ROOT/menu.out"
  assert_contains '查看下次运行时间' "$CASE_ROOT/menu.out"
  assert_contains '关于 CloudLedger' "$CASE_ROOT/menu.out"
  assert_contains '0. 返回上一级' "$CASE_ROOT/menu.out"

  if "$OPS" unexpected >"$CASE_ROOT/args.out" 2>&1; then
    fail_test 'unknown public arguments were accepted'
  fi
  assert_contains '公开入口只支持数字交互菜单' "$CASE_ROOT/args.out"

  if "$OPS" --internal unknown >"$CASE_ROOT/internal.out" 2>&1; then
    fail_test 'unknown internal task was accepted'
  fi
  assert_contains '未知内部任务' "$CASE_ROOT/internal.out"
}

test_hidden_pat() {
  setup_case hidden-pat
  printf '%s\n' 1 3 2 0 0 >"$CASE_ROOT/menu.answers"
  BASH_ENV="$MOCK_READ" CLOUDLEDGER_TEST_MENU_ANSWERS="$CASE_ROOT/menu.answers" \
    "$OPS" >"$CASE_ROOT/login.out" 2>&1
  assert_contains 'docker:login' "$CLOUDLEDGER_TEST_TRACE"
  assert_secret_not_logged "$CLOUDLEDGER_TEST_GHCR_PAT" \
    "$CASE_ROOT/login.out" "$CLOUDLEDGER_TEST_TRACE" "$CLOUDLEDGER_TEST_READ_TRACE"
}

test_complete_wizard() {
  setup_case complete-wizard
  seed_rclone_config
  printf '%s\n' 1 11 1 1 0 0 >"$CASE_ROOT/menu.answers"
  BASH_ENV="$MOCK_READ" CLOUDLEDGER_TEST_MENU_ANSWERS="$CASE_ROOT/menu.answers" \
    "$OPS" >"$CASE_ROOT/wizard.out" 2>&1

  assert_contains '全部安装向导完成' "$CASE_ROOT/wizard.out"
  [[ -s "$CLOUDLEDGER_DEPLOY_DIR/compose.yml" ]] || fail_test 'wizard did not stage compose.yml'
  [[ -s "$CLOUDLEDGER_DEPLOY_DIR/Caddyfile" ]] || fail_test 'wizard did not stage Caddyfile'
  [[ -x "$CLOUDLEDGER_DEPLOY_DIR/cloudledger-ops.sh" ]] || fail_test 'wizard did not stage executable ops script'
  [[ -s "$CLOUDLEDGER_SERVER_CONFIG" ]] || fail_test 'wizard did not render server.toml'
  assert_file_mode 600 "$CLOUDLEDGER_OPS_ENV"
  assert_file_mode 600 "$CLOUDLEDGER_SERVER_CONFIG"
  assert_file_mode 600 "$CLOUDLEDGER_CERT_DIR/origin-key.pem"
  assert_file_mode 600 "$CLOUDLEDGER_RCLONE_CONFIG"
  assert_contains 'CLOUDLEDGER_SERVER_IMAGE=ghcr.io/cloudledger/cloudledger-server:v0.1.5' "$CLOUDLEDGER_OPS_ENV"
  assert_contains 'CLOUDLEDGER_POSTGRES_IMAGE=ghcr.io/cloudledger/cloudledger-postgres:v0.1.5' "$CLOUDLEDGER_OPS_ENV"
  assert_contains 'CLOUDLEDGER_CADDY_IMAGE=ghcr.io/cloudledger/cloudledger-caddy:v0.1.5' "$CLOUDLEDGER_OPS_ENV"
  assert_contains 'CLOUDLEDGER_ANCHOR_IMAGE=ghcr.io/cloudledger/cloudledger-network-anchor:v0.1.5' "$CLOUDLEDGER_OPS_ENV"
  assert_contains 'CLOUDLEDGER_HTTP_PUBLISH=127.0.0.1:18080:80' "$CLOUDLEDGER_OPS_ENV"
  assert_contains 'CLOUDLEDGER_HTTPS_PUBLISH=443:443' "$CLOUDLEDGER_OPS_ENV"
  assert_contains 'public_api_url = "https://cloudledger-test.513921.xyz"' "$CLOUDLEDGER_SERVER_CONFIG"
  assert_contains 'admin_bind_addr = "127.0.0.1:8788"' "$CLOUDLEDGER_SERVER_CONFIG"
  assert_contains 'site_key = "ops-test-turnstile-site"' "$CLOUDLEDGER_SERVER_CONFIG"
  if [[ -e "$CLOUDLEDGER_SYSTEMD_DIR/docker.service.d/cloudledger-firewall.conf" ]]; then
    fail_test 'install_systemd_units created a Docker Requires drop-in'
  fi

  assert_trace_order "$CLOUDLEDGER_TEST_TRACE" \
    'compose:pull' \
    'compose:up:database' \
    'firewall:check' \
    'firewall:apply' \
    'compose:migration' \
    'compose:audit' \
    'compose:up:backend' \
    'http:local-health' \
    'http:local-ready' \
    'compose:up:caddy' \
    'http:health' \
    'http:ready' \
    'rclone:upload' \
    'rclone:download' \
    'restore:createdb' \
    'restore:pg-restore' \
    'postgres:migrations-exact' \
    'restore:table:organizations' \
    'restore:table:ledgers' \
    'restore:table:financial_accounts' \
    'restore:table:categories' \
    'restore:table:transactions' \
    'restore:table:audit_events' \
    'restore:audit' \
    'systemd:timer:cloudledger-ops-backup.timer' \
    'systemd:timer:cloudledger-ops-restore-test.timer'

  local secret_name secret_value
  for secret_name in CLOUDLEDGER_BOOTSTRAP_DB_PASSWORD CLOUDLEDGER_MIGRATION_DB_PASSWORD \
    CLOUDLEDGER_RUNTIME_DB_PASSWORD CLOUDLEDGER_ADMIN_TOKEN CLOUDLEDGER_AUDIT_HMAC_KEY \
    CLOUDLEDGER_AUDIT_IDENTIFIER_HMAC_KEY; do
    secret_value=$(bash -c 'source "$1"; printf "%s" "${!2}"' _ "$CLOUDLEDGER_OPS_ENV" "$secret_name")
    assert_secret_not_logged "$secret_value" \
      "$CASE_ROOT/wizard.out" "$CLOUDLEDGER_TEST_TRACE" "$CLOUDLEDGER_TEST_READ_TRACE" \
      "$CLOUDLEDGER_OPS_STATE_DIR/deploy.log" "$CLOUDLEDGER_OPS_STATE_DIR/backup.log" \
      "$CLOUDLEDGER_OPS_STATE_DIR/restore-test.log"
  done
  assert_secret_not_logged "$CLOUDLEDGER_TEST_TURNSTILE_SECRET" \
    "$CASE_ROOT/wizard.out" "$CLOUDLEDGER_TEST_TRACE" "$CLOUDLEDGER_TEST_READ_TRACE"
  assert_secret_not_logged "$CLOUDLEDGER_TEST_RCLONE_PASSWORD" \
    "$CASE_ROOT/wizard.out" "$CLOUDLEDGER_TEST_TRACE" "$CLOUDLEDGER_TEST_READ_TRACE"
}

test_backups() {
  local archive

  setup_case backup-container-pg-dump
  seed_backup_fixture no
  "$OPS" --internal backup >"$CASE_ROOT/backup.out" 2>&1
  archive=$(latest_archive)
  assert_valid_archive "$archive" "$CASE_ROOT/extracted"
  assert_contains 'backup:docker-pg-dump' "$CLOUDLEDGER_TEST_TRACE"
  assert_file_mode 600 "$archive"
  assert_no_pending_backup
  if find "$CLOUDLEDGER_BACKUP_DIR" -maxdepth 1 -type d -name '.tmp-*' | grep -q .; then
    fail_test 'plaintext backup staging directory was not cleaned'
  fi
  if tar -tf "$archive" | grep -q 'rclone.conf'; then
    fail_test 'rclone.conf must never be included in a backup'
  fi

  setup_case backup-empty-dump
  seed_backup_fixture no
  export CLOUDLEDGER_TEST_PG_DUMP_MODE=empty
  if "$OPS" --internal backup >"$CASE_ROOT/backup.out" 2>&1; then
    fail_test 'empty pg_dump was accepted'
  fi
  if [[ -n "$(latest_archive)" ]]; then
    fail_test 'an archive was created from an empty pg_dump'
  fi
  assert_no_pending_backup
  assert_contains '空文件' "$CLOUDLEDGER_OPS_STATE_DIR/backup.log"

  setup_case backup-partial-failure
  seed_backup_fixture no
  export CLOUDLEDGER_TEST_PG_DUMP_MODE=partial-fail
  if "$OPS" --internal backup >"$CASE_ROOT/backup.out" 2>&1; then
    fail_test 'nonzero pg_dump with partial output was accepted'
  fi
  [[ -z "$(latest_archive)" ]] || fail_test 'partial failed pg_dump created a final backup'
  assert_no_pending_backup
  assert_contains 'pg_dump 返回失败状态' "$CLOUDLEDGER_OPS_STATE_DIR/backup.log"

  setup_case backup-local-verification-failure
  seed_backup_fixture no
  export CLOUDLEDGER_TEST_PG_RESTORE_MODE=fail-verify
  if "$OPS" --internal backup >"$CASE_ROOT/backup.out" 2>&1; then
    fail_test 'failed local archive verification was accepted'
  fi
  [[ $(count_final_backups) -eq 0 ]] \
    || fail_test 'local verification failure left a final backup visible'
  assert_no_pending_backup

  setup_case backup-upload-failure
  seed_backup_fixture yes
  mkdir -p "$CLOUDLEDGER_BACKUP_DIR"
  printf 'preserve me\n' >"$CLOUDLEDGER_BACKUP_DIR/cloudledger-20000101-000000-old.tar"
  export CLOUDLEDGER_TEST_RCLONE_MODE=fail
  if "$OPS" --internal backup >"$CASE_ROOT/backup.out" 2>&1; then
    fail_test 'failed remote upload was accepted'
  fi
  [[ -f "$CLOUDLEDGER_BACKUP_DIR/cloudledger-20000101-000000-old.tar" ]] \
    || fail_test 'remote upload failure removed an older backup'
  [[ $(count_final_backups) -eq 1 ]] \
    || fail_test 'remote upload failure left a new final backup visible'
  assert_no_pending_backup
  assert_contains '旧备份不会被清理' "$CLOUDLEDGER_OPS_STATE_DIR/backup.log"
}

test_restore_prerequisites() {
  setup_case restore-no-backup
  if "$OPS" --internal restore-test >"$CASE_ROOT/restore.out" 2>&1; then
    fail_test 'restore drill without a backup was reported as success'
  fi
  assert_contains '没有可用于恢复演练的本地备份' "$CASE_ROOT/restore.out"
  assert_not_contains 'restore:createdb' "$CLOUDLEDGER_TEST_TRACE"
}

test_firewall_internal_mode() {
  setup_case firewall-valid
  "$OPS" --internal firewall-refresh >"$CASE_ROOT/firewall.out" 2>&1
  assert_trace_order "$CLOUDLEDGER_TEST_TRACE" \
    'firewall:check' 'firewall:apply' 'cloudflare:ipv4' 'cloudflare:ipv6' \
    'firewall:check' 'firewall:apply'
  local rules="$CLOUDLEDGER_FIREWALL_DIR/cloudledger-origin.nft"
  [[ -s "$rules" ]] || fail_test 'firewall rules file was not persisted'
  assert_contains 'chain input' "$rules"
  assert_contains 'chain forward' "$rules"
  assert_contains 'oifname "cld-origin0"' "$rules"
  assert_contains '173.245.48.0/20' "$rules"
  assert_contains '2400:cb00::/32' "$rules"
  assert_contains 'tcp dport 443' "$rules"
  assert_not_contains 'tcp dport 80 ' "$rules"

  setup_case firewall-invalid
  export CLOUDLEDGER_TEST_CLOUDFLARE_IP_MODE=invalid
  export CLOUDLEDGER_TEST_NFT_CAPTURE="$CASE_ROOT/fail-closed-baseline.nft"
  if "$OPS" --internal firewall-refresh >"$CASE_ROOT/firewall.out" 2>&1; then
    fail_test 'invalid Cloudflare ranges were accepted'
  fi
  [[ -s "$CLOUDLEDGER_TEST_NFT_CAPTURE" ]] || fail_test 'fail-closed baseline was not rendered'
  assert_not_contains 'elements = {  }' "$CLOUDLEDGER_TEST_NFT_CAPTURE"
  assert_contains 'set cloudflare_ipv4 {' "$CLOUDLEDGER_TEST_NFT_CAPTURE"
  assert_contains 'set cloudflare_ipv6 {' "$CLOUDLEDGER_TEST_NFT_CAPTURE"
  [[ $(grep -Fc 'firewall:apply' "$CLOUDLEDGER_TEST_TRACE") -eq 1 ]] \
    || fail_test 'invalid ranges must leave only the fail-closed baseline application'
  assert_contains '无法取得有效的 Cloudflare IPv4 官方列表' "$CASE_ROOT/firewall.out"
}

verify_archive_rejected() {
  local archive=$1 label=$2 output
  output="$CASE_ROOT/$label.out"
  : >"$CLOUDLEDGER_TEST_TRACE"
  printf '4\n4\n%s\n0\n0\n' "$(basename "$archive")" | "$OPS" >"$output" 2>&1 || true
  assert_contains '备份归档成员' "$output"
  assert_not_contains '备份校验通过' "$output"
  assert_not_contains 'backup:pg-restore-list' "$CLOUDLEDGER_TEST_TRACE"
}

test_archive_extraction_guards() {
  local archive baseline duplicate nonregular oversized damaged
  setup_case archive-guards
  seed_backup_fixture no
  "$OPS" --internal backup >"$CASE_ROOT/backup.out" 2>&1
  archive=$(latest_archive)
  baseline="$CASE_ROOT/baseline"
  assert_valid_archive "$archive" "$baseline"
  printf 'CLOUDLEDGER_MAX_BACKUP_BYTES=1048576\n' >>"$CLOUDLEDGER_OPS_ENV"

  duplicate="$CLOUDLEDGER_BACKUP_DIR/cloudledger-20990101-010101-101.tar"
  cp -- "$archive" "$duplicate"
  tar -C "$baseline" -rf "$duplicate" ./server.toml
  verify_archive_rejected "$duplicate" duplicate

  nonregular="$CLOUDLEDGER_BACKUP_DIR/cloudledger-20990101-010102-102.tar"
  cp -a -- "$baseline" "$CASE_ROOT/nonregular-payload"
  rm -f -- "$CASE_ROOT/nonregular-payload/server.toml"
  ln -s manifest.json "$CASE_ROOT/nonregular-payload/server.toml"
  tar -C "$CASE_ROOT/nonregular-payload" -cf "$nonregular" .
  verify_archive_rejected "$nonregular" nonregular

  oversized="$CLOUDLEDGER_BACKUP_DIR/cloudledger-20990101-010103-103.tar"
  cp -a -- "$baseline" "$CASE_ROOT/oversized-payload"
  truncate -s 2097152 "$CASE_ROOT/oversized-payload/postgres.dump"
  (
    cd "$CASE_ROOT/oversized-payload"
    sha256sum postgres.dump server.toml compose.env compose.yml Caddyfile \
      origin-cert.pem origin-key.pem manifest.json >SHA256SUMS
  )
  tar --sparse -C "$CASE_ROOT/oversized-payload" -cf "$oversized" .
  verify_archive_rejected "$oversized" oversized

  damaged="$CLOUDLEDGER_BACKUP_DIR/cloudledger-20990101-010104-104.tar"
  cp -- "$archive" "$damaged"
  truncate -s 1536 "$damaged"
  verify_archive_rejected "$damaged" damaged
}

test_load_env_clears_exported_values() {
  setup_case load-env
  printf 'CLOUDLEDGER_DISK_WARN=80\n' >"$CLOUDLEDGER_OPS_ENV"
  printf '6\n1\n0\n0\n' \
    | env CLOUDLEDGER_API_DOMAIN=stale-export.example CLOUDLEDGER_RCLONE_REMOTE=stale:remote \
      "$OPS" >"$CASE_ROOT/load-env.out" 2>&1
  assert_contains 'API 域名: 未配置' "$CASE_ROOT/load-env.out"
  assert_not_contains 'stale-export.example' "$CASE_ROOT/load-env.out"

  setup_case load-env-injection
  local marker="$CASE_ROOT/source-executed"
  printf 'CLOUDLEDGER_API_DOMAIN=$(touch %s)\n' "$marker" >"$CLOUDLEDGER_OPS_ENV"
  printf '11\n1\n0\n0\n' | "$OPS" >"$CASE_ROOT/injection.out" 2>&1 || true
  [[ ! -e "$marker" ]] || fail_test 'ops.env executed shell syntax'
  assert_contains '运维配置包含未知键、重复键或无效转义' "$CASE_ROOT/injection.out"
}

test_firewall_status_integrity() {
  local mode
  setup_case firewall-status-valid
  seed_backup_fixture no
  : >"$CLOUDLEDGER_TEST_NFT_STATE"
  "$OPS" --internal health >"$CASE_ROOT/health.out" 2>&1 \
    || fail_test 'complete firewall table was reported unhealthy'

  setup_case firewall-status-pretty-priority
  seed_backup_fixture no
  : >"$CLOUDLEDGER_TEST_NFT_STATE"
  export CLOUDLEDGER_TEST_NFT_PRETTY_PRIORITY=1
  "$OPS" --internal health >"$CASE_ROOT/health.out" 2>&1 \
    || fail_test 'nft pretty-printed priority was reported as unhealthy'

  for mode in missing-sets missing-input missing-forward missing-bridge missing-reject extra-accept non-hook unauthorized-chain; do
    setup_case "firewall-status-$mode"
    seed_backup_fixture no
    : >"$CLOUDLEDGER_TEST_NFT_STATE"
    export CLOUDLEDGER_TEST_NFT_TABLE_MODE=$mode
    if "$OPS" --internal health >"$CASE_ROOT/health.out" 2>&1; then
      fail_test "incomplete firewall table was accepted: $mode"
    fi
  done
}

test_restore_bundle_trust_boundary() {
  local archive baseline tampered payload id stamp
  setup_case restore-trust
  seed_backup_fixture no
  export CLOUDLEDGER_TEST_ASSERT_CLEAN_COMPOSE_ENV=1
  "$OPS" --internal backup >"$CASE_ROOT/backup.out" 2>&1
  assert_contains 'compose:clean-env' "$CLOUDLEDGER_TEST_TRACE"
  archive=$(latest_archive)
  id=${archive##*/}; id=${id#cloudledger-}; id=${id%.tar}; stamp=${id%-*}
  baseline="$CASE_ROOT/trusted-payload"
  mkdir -p "$baseline"
  tar -xf "$archive" -C "$baseline"

  payload="$CASE_ROOT/tampered-server"
  cp -a -- "$baseline" "$payload"
  sed -i 's#https://challenges.cloudflare.com/turnstile/v0/siteverify#https://attacker.invalid/siteverify#' "$payload/server.toml"
  tampered="$CLOUDLEDGER_BACKUP_DIR/cloudledger-$stamp-900201.tar"
  repack_backup_payload "$payload" "$tampered"
  verify_candidate_bundle_rejected "$tampered" tampered-server

  payload="$CASE_ROOT/tampered-owner"
  cp -a -- "$baseline" "$payload"
  sed -i \
    -e 's#CLOUDLEDGER_GHCR_OWNER=cloudledger#CLOUDLEDGER_GHCR_OWNER=attacker#' \
    -e 's#ghcr.io/cloudledger/#ghcr.io/attacker/#g' \
    "$payload/compose.env"
  tampered="$CLOUDLEDGER_BACKUP_DIR/cloudledger-$stamp-900202.tar"
  repack_backup_payload "$payload" "$tampered"
  verify_candidate_bundle_rejected "$tampered" tampered-owner

  payload="$CASE_ROOT/missing-password"
  cp -a -- "$baseline" "$payload"
  sed -i '/^CLOUDLEDGER_RUNTIME_DB_PASSWORD=/d' "$payload/compose.env"
  tampered="$CLOUDLEDGER_BACKUP_DIR/cloudledger-$stamp-900203.tar"
  repack_backup_payload "$payload" "$tampered"
  verify_candidate_bundle_rejected "$tampered" missing-password
}

test_backup_identity_and_remote_freshness() {
  local archive name id stamp mismatch remote_archive payload stale_name checkpoint

  setup_case backup-identity
  seed_backup_fixture no
  "$OPS" --internal backup >"$CASE_ROOT/backup.out" 2>&1
  archive=$(latest_archive)
  name=${archive##*/}; id=${name#cloudledger-}; id=${id%.tar}; stamp=${id%-*}
  mismatch="$CLOUDLEDGER_BACKUP_DIR/cloudledger-$stamp-999991.tar"
  cp -- "$archive" "$mismatch"
  printf '4\n4\n%s\n0\n0\n' "${mismatch##*/}" | "$OPS" >"$CASE_ROOT/mismatch.out" 2>&1 || true
  assert_contains '备份编号、文件名或创建时间不一致' "$CASE_ROOT/mismatch.out"
  assert_not_contains '备份校验通过' "$CASE_ROOT/mismatch.out"

  setup_case remote-stale
  seed_backup_fixture yes
  "$OPS" --internal backup >"$CASE_ROOT/backup.out" 2>&1
  remote_archive=$(find "$CLOUDLEDGER_TEST_REMOTE_DIR" -type f -name 'cloudledger-*.tar' -print | head -n1)
  [[ -s "$remote_archive" ]] || fail_test 'remote backup fixture was not published'
  payload="$CASE_ROOT/stale-payload"
  mkdir -p "$payload"
  tar -xf "$remote_archive" -C "$payload"
  stale_name='cloudledger-20000101-000000-1.tar'
  jq '.id = "20000101-000000-1" | .created_at = "2000-01-01T00:00:00Z"' \
    "$payload/manifest.json" >"$payload/.manifest.json.new"
  mv -f -- "$payload/.manifest.json.new" "$payload/manifest.json"
  find "$CLOUDLEDGER_TEST_REMOTE_DIR" -type f -delete
  rm -f -- "$CLOUDLEDGER_OPS_STATE_DIR/last-remote-backup"
  repack_backup_payload "$payload" "$(dirname "$remote_archive")/$stale_name"
  if "$OPS" --internal restore-test >"$CASE_ROOT/restore.out" 2>&1; then
    fail_test 'stale remote backup passed the restore drill'
  fi
  assert_contains '远端最新备份已超过 72 小时' "$CASE_ROOT/restore.out"
  [[ ! -e "$CLOUDLEDGER_OPS_STATE_DIR/restore-test.log" ]] \
    || fail_test 'stale remote restore drill wrote a success record'

  setup_case remote-rollback
  seed_backup_fixture yes
  "$OPS" --internal backup >"$CASE_ROOT/backup.out" 2>&1
  remote_archive=$(find "$CLOUDLEDGER_TEST_REMOTE_DIR" -type f -name 'cloudledger-*.tar' -print | head -n1)
  name=${remote_archive##*/}; id=${name#cloudledger-}; id=${id%.tar}; stamp=${id%-*}
  checkpoint='cloudledger-20991231-235959-1.tar'
  printf '%s\n' "$checkpoint" >"$CLOUDLEDGER_OPS_STATE_DIR/last-remote-backup"
  chmod 600 "$CLOUDLEDGER_OPS_STATE_DIR/last-remote-backup"
  if "$OPS" --internal restore-test >"$CASE_ROOT/restore.out" 2>&1; then
    fail_test 'rolled-back remote listing passed the restore drill'
  fi
  assert_contains '疑似回滚' "$CASE_ROOT/restore.out"
  [[ ! -e "$CLOUDLEDGER_OPS_STATE_DIR/restore-test.log" ]] \
    || fail_test 'remote rollback drill wrote a success record'
}

test_restore_with_archived_passwords() {
  local archive name
  setup_case restore-archived-passwords
  seed_backup_fixture no
  "$OPS" --internal backup >"$CASE_ROOT/backup.out" 2>&1
  archive=$(latest_archive)
  name=${archive##*/}
  sed -i \
    -e 's/test-bootstrap-password/current-bootstrap-password/g' \
    -e 's/test-migration-password/current-migration-password/g' \
    -e 's/test-runtime-password/current-runtime-password/g' \
    -e 's/CLOUDLEDGER_CLIENT_VERSION=0\.1\.4/CLOUDLEDGER_CLIENT_VERSION=0.1.5/g' \
    -e 's/CLOUDLEDGER_MIN_SUPPORTED_CLIENT_VERSION=0\.1\.4/CLOUDLEDGER_MIN_SUPPORTED_CLIENT_VERSION=0.1.5/g' \
    -e 's/client_version = "0\.1\.4"/client_version = "0.1.5"/g' \
    -e 's/min_supported_client_version = "0\.1\.4"/min_supported_client_version = "0.1.5"/g' \
    "$CLOUDLEDGER_OPS_ENV" "$CLOUDLEDGER_SERVER_CONFIG"
  : >"$CLOUDLEDGER_TEST_NFT_STATE"
  printf '4\n6\n%s\nYES\n%s\n0\n0\n' "$name" "$name" | "$OPS" >"$CASE_ROOT/restore.out" 2>&1
  assert_contains '数据库、配置、角色密码和服务已从同一备份事务恢复' "$CASE_ROOT/restore.out"
  assert_contains 'CLOUDLEDGER_BOOTSTRAP_DB_PASSWORD=test-bootstrap-password' "$CLOUDLEDGER_OPS_ENV"
  assert_contains 'CLOUDLEDGER_MIGRATION_DB_PASSWORD=test-migration-password' "$CLOUDLEDGER_OPS_ENV"
  assert_contains 'CLOUDLEDGER_RUNTIME_DB_PASSWORD=test-runtime-password' "$CLOUDLEDGER_OPS_ENV"
  assert_contains 'CLOUDLEDGER_CLIENT_VERSION=0.1.4' "$CLOUDLEDGER_OPS_ENV"
  assert_contains 'CLOUDLEDGER_MIN_SUPPORTED_CLIENT_VERSION=0.1.4' "$CLOUDLEDGER_OPS_ENV"
  assert_contains 'client_version = "0.1.4"' "$CLOUDLEDGER_SERVER_CONFIG"
  assert_contains 'min_supported_client_version = "0.1.4"' "$CLOUDLEDGER_SERVER_CONFIG"
  assert_trace_order "$CLOUDLEDGER_TEST_TRACE" \
    'restore:dropdb' 'restore:createdb' 'restore:pg-restore' 'postgres:query' 'compose:pull'
}

test_restore_cleanup_failure() {
  setup_case restore-cleanup-failure
  seed_backup_fixture no
  "$OPS" --internal backup >"$CASE_ROOT/backup.out" 2>&1
  export CLOUDLEDGER_TEST_DROPDB_MODE=fail-cleanup
  if "$OPS" --internal restore-test >"$CASE_ROOT/restore.out" 2>&1; then
    fail_test 'restore drill reported success after temporary database cleanup failed'
  fi
  assert_contains '临时数据库清理失败' "$CASE_ROOT/restore.out"
  assert_contains 'restore:audit-temp-database' "$CLOUDLEDGER_TEST_TRACE"
  [[ ! -e "$CLOUDLEDGER_OPS_STATE_DIR/restore-test.log" ]] \
    || fail_test 'failed restore cleanup wrote a success record'
}

test_restore_core_table_validation() {
  setup_case restore-missing-core-table
  seed_backup_fixture no
  "$OPS" --internal backup >"$CASE_ROOT/backup.out" 2>&1
  export CLOUDLEDGER_TEST_MISSING_RESTORE_TABLE=organizations
  if "$OPS" --internal restore-test >"$CASE_ROOT/restore.out" 2>&1; then
    fail_test 'restore drill accepted a database without organizations'
  fi
  assert_contains '恢复演练缺少核心表: organizations' "$CASE_ROOT/restore.out"
  [[ ! -e "$CLOUDLEDGER_OPS_STATE_DIR/restore-test.log" ]] \
    || fail_test 'missing core table restore drill wrote a success record'
}

test_future_migration_compatibility() {
  setup_case future-migrations
  seed_backup_fixture no
  "$OPS" --internal backup >"$CASE_ROOT/backup.out" 2>&1
  export CLOUDLEDGER_TEST_MIGRATION_SUMMARY='1,2,3,4,5,6|6|true'
  "$OPS" --internal restore-test >"$CASE_ROOT/restore.out" 2>&1
  assert_contains '6 个 SQLx migration' "$CASE_ROOT/restore.out"
  [[ -s "$CLOUDLEDGER_OPS_STATE_DIR/restore-test.log" ]] \
    || fail_test 'future migration restore drill did not write success record'
}

test_https_port_preflight() {
  setup_case https-port-owner
  seed_backup_fixture no
  export CLOUDLEDGER_TEST_HTTPS_DOCKER_OWNER=other
  printf '1\n9\n0\n0\n' | "$OPS" >"$CASE_ROOT/deploy.out" 2>&1 || true
  assert_contains '主机 443 已由其他 Docker 容器占用' "$CASE_ROOT/deploy.out"
  assert_not_contains 'compose:pull' "$CLOUDLEDGER_TEST_TRACE"
  assert_not_contains 'firewall:apply' "$CLOUDLEDGER_TEST_TRACE"
}

test_remote_pending_signal_cleanup() {
  setup_case remote-pending-signal
  seed_backup_fixture yes
  export CLOUDLEDGER_TEST_SIGNAL_ON_RCLONE=upload
  if "$OPS" --internal backup >"$CASE_ROOT/backup.out" 2>&1; then
    fail_test 'signal-interrupted remote upload reported success'
  fi
  if find "$CLOUDLEDGER_TEST_REMOTE_DIR" -type f -name '*.new' -print | grep -q .; then
    fail_test 'signal-interrupted upload left a remote .new object'
  fi
  assert_contains 'rclone:delete' "$CLOUDLEDGER_TEST_TRACE"
}

verify_outside_backup_rejected() {
  local requested=$1 label=$2 output
  output="$CASE_ROOT/$label.out"
  : >"$CLOUDLEDGER_TEST_TRACE"
  printf '4\n4\n%s\n0\n0\n' "$requested" | "$OPS" >"$output" 2>&1 || true
  assert_not_contains '备份校验通过' "$output"
  assert_not_contains 'backup:pg-restore-list' "$CLOUDLEDGER_TEST_TRACE"
}

test_backup_path_confinement() {
  local archive outside link
  setup_case backup-path
  seed_backup_fixture no
  "$OPS" --internal backup >"$CASE_ROOT/backup.out" 2>&1
  archive=$(latest_archive)
  outside="$CASE_ROOT/outside-cloudledger.tar"
  cp -- "$archive" "$outside"
  verify_outside_backup_rejected "$outside" outside-direct

  link="$CLOUDLEDGER_BACKUP_DIR/cloudledger-outside-link.tar"
  ln -s "$outside" "$link"
  verify_outside_backup_rejected "$(basename "$link")" outside-symlink
}

run_deploy_verification_menu() {
  local output=$1
  printf '1\n10\n0\n0\n' | "$OPS" >"$output" 2>&1 || true
}

test_turnstile_secret_probe() {
  setup_case turnstile-secret-valid
  seed_backup_fixture no
  run_deploy_verification_menu "$CASE_ROOT/verify.out"
  assert_contains 'turnstile:probe' "$CLOUDLEDGER_TEST_TRACE"
  assert_contains 'Turnstile secret key 验证通过' "$CASE_ROOT/verify.out"
  assert_secret_not_logged "$CLOUDLEDGER_TEST_TURNSTILE_SECRET" \
    "$CASE_ROOT/verify.out" "$CLOUDLEDGER_TEST_TRACE"

  setup_case turnstile-secret-invalid
  seed_backup_fixture no
  export CLOUDLEDGER_TEST_TURNSTILE_PROBE_MODE=invalid-secret
  run_deploy_verification_menu "$CASE_ROOT/verify.out"
  assert_contains 'turnstile:probe' "$CLOUDLEDGER_TEST_TRACE"
  assert_contains 'Turnstile secret key 无效' "$CASE_ROOT/verify.out"
  assert_not_contains 'Turnstile secret key 验证通过' "$CASE_ROOT/verify.out"
  assert_contains '已返回当前菜单' "$CASE_ROOT/verify.out"
  [[ $(grep -Fc 'CloudLedger 云服务运维工具箱' "$CASE_ROOT/verify.out") -ge 3 ]] \
    || fail_test 'failed interactive action did not return to its menu'
  assert_secret_not_logged "$CLOUDLEDGER_TEST_TURNSTILE_SECRET" \
    "$CASE_ROOT/verify.out" "$CLOUDLEDGER_TEST_TRACE"
}

compose_service_section() {
  local service=$1 output=$2
  awk -v header="  $service:" '
    $0 == header { inside = 1; print; next }
    inside && /^  [A-Za-z0-9_-]+:$/ { exit }
    inside { print }
  ' "$ROOT/deploy/docker-compose.yml" >"$output"
}

test_admin_relay_model() {
  setup_case admin-relay
  local anchor="$CASE_ROOT/network-anchor.yml" relay_source="$CASE_ROOT/relay-source.txt" source
  compose_service_section network-anchor "$anchor"
  assert_contains '127.0.0.1:8788:18788' "$anchor"
  assert_not_contains '127.0.0.1:8788:8788' "$anchor"
  assert_not_contains '  admin-relay:' "$ROOT/deploy/docker-compose.yml"
  : >"$relay_source"
  while IFS= read -r source; do
    printf '## %s\n' "$source" >>"$relay_source"
    sed -n '1,240p' "$source" >>"$relay_source"
  done < <(find "$ROOT/deploy" -maxdepth 2 -type f \( -iname '*anchor*' -o -iname '*relay*' \) -print)
  assert_contains 'socat' "$relay_source"
  assert_contains '18788' "$relay_source"
  assert_contains '0.0.0.0' "$relay_source"
  assert_contains '127.0.0.1:8788' "$relay_source"
}

test_systemd_unit_contract() {
  local service="$ROOT/deploy/systemd/cloudledger-ops-firewall-refresh.service" unit
  assert_not_contains 'RemainAfterExit=yes' "$service"
  assert_not_contains 'Before=docker.service' "$service"
  for unit in "$ROOT"/deploy/systemd/*.timer; do
    assert_contains 'Persistent=true' "$unit"
  done
  [[ $(find "$ROOT/deploy/systemd" -maxdepth 1 -type f | wc -l) -eq 8 ]] \
    || fail_test 'expected exactly eight CloudLedger systemd unit files'
}

test_diagnostic_redaction() {
  local report
  setup_case diagnostic-redaction
  seed_backup_fixture no
  sed -i \
    -e 's#^CLOUDLEDGER_ADMIN_PATH=.*#CLOUDLEDGER_ADMIN_PATH=manage-private-diagnostic-path#' \
    -e 's#^CLOUDLEDGER_ADMIN_TOKEN=.*#CLOUDLEDGER_ADMIN_TOKEN=admin-private-diagnostic-token#' \
    -e 's#^CLOUDLEDGER_AUDIT_HMAC_KEY=.*#CLOUDLEDGER_AUDIT_HMAC_KEY=private-diagnostic-hmac#' \
    "$CLOUDLEDGER_OPS_ENV"
  printf '%s\n' 'CLOUDLEDGER_WEBHOOK_URL=https://hooks.example/private-webhook-token' >>"$CLOUDLEDGER_OPS_ENV"
  printf '8\n12\n0\n0\n' | "$OPS" >"$CASE_ROOT/diagnostic.out" 2>&1
  report=$(find "$CLOUDLEDGER_OPS_STATE_DIR" -maxdepth 1 -type f -name 'diagnostic-*.txt' -print | head -n1)
  [[ -s "$report" ]] || fail_test 'diagnostic report was not created'
  assert_contains '<已隐藏>' "$report"
  assert_secret_not_logged 'manage-private-diagnostic-path' "$report"
  assert_secret_not_logged 'admin-private-diagnostic-token' "$report"
  assert_secret_not_logged 'private-diagnostic-hmac' "$report"
  assert_secret_not_logged 'private-webhook-token' "$report"
  assert_secret_not_logged 'test-runtime-password' "$report"
}

test_rclone_display_redaction() {
  setup_case rclone-redaction
  seed_backup_fixture no
  printf '7\n4\n0\n0\n' | "$OPS" >"$CASE_ROOT/rclone.out" 2>&1
  assert_contains '<已隐藏>' "$CASE_ROOT/rclone.out"
  assert_secret_not_logged "$CLOUDLEDGER_TEST_RCLONE_PASSWORD" "$CASE_ROOT/rclone.out"
}

test_upgrade_transaction_order() {
  setup_case upgrade-order
  seed_backup_fixture yes
  printf '3\n3\nv0.1.5\nYES\n0\n0\n' | "$OPS" >"$CASE_ROOT/upgrade.out" 2>&1
  assert_trace_order "$CLOUDLEDGER_TEST_TRACE" \
    'docker:manifest-inspect' \
    'docker:manifest-inspect' \
    'docker:manifest-inspect' \
    'docker:manifest-inspect' \
    'backup:docker-pg-dump' \
    'rclone:upload' \
    'rclone:download' \
    'rclone:publish' \
    'compose:pull' \
    'compose:migration' \
    'compose:audit' \
    'compose:up:backend' \
    'http:local-health' \
    'http:local-ready' \
    'compose:up:caddy'
  assert_contains 'CLOUDLEDGER_RELEASE_TAG=v0.1.5' "$CLOUDLEDGER_OPS_ENV"
  assert_contains 'CLOUDLEDGER_CLIENT_VERSION=0.1.5' "$CLOUDLEDGER_OPS_ENV"
  assert_contains 'CLOUDLEDGER_MIN_SUPPORTED_CLIENT_VERSION=0.1.5' "$CLOUDLEDGER_OPS_ENV"
  assert_contains 'client_version = "0.1.5"' "$CLOUDLEDGER_SERVER_CONFIG"
  assert_contains 'min_supported_client_version = "0.1.5"' "$CLOUDLEDGER_SERVER_CONFIG"
  assert_contains 'client_download_url = "https://github.com/dahai9/CloudLedger/releases/latest"' "$CLOUDLEDGER_SERVER_CONFIG"
  assert_contains '升级完成' "$CASE_ROOT/upgrade.out"
}

test_upgrade_backfills_client_version_config() {
  setup_case upgrade-backfill-client-version
  seed_backup_fixture yes
  sed -i \
    -e '/^CLOUDLEDGER_CLIENT_VERSION=/d' \
    -e '/^CLOUDLEDGER_MIN_SUPPORTED_CLIENT_VERSION=/d' \
    -e '/^CLOUDLEDGER_CLIENT_DOWNLOAD_URL=/d' \
    "$CLOUDLEDGER_OPS_ENV"
  sed -i \
    -e '/^client_version = /d' \
    -e '/^min_supported_client_version = /d' \
    -e '/^client_download_url = /d' \
    "$CLOUDLEDGER_SERVER_CONFIG"
  printf '3\n3\nv0.1.5\nYES\n0\n0\n' | "$OPS" >"$CASE_ROOT/upgrade.out" 2>&1
  assert_contains '升级完成' "$CASE_ROOT/upgrade.out"
  assert_contains 'CLOUDLEDGER_CLIENT_VERSION=0.1.5' "$CLOUDLEDGER_OPS_ENV"
  assert_contains 'CLOUDLEDGER_MIN_SUPPORTED_CLIENT_VERSION=0.1.5' "$CLOUDLEDGER_OPS_ENV"
  assert_contains 'CLOUDLEDGER_CLIENT_DOWNLOAD_URL=https://github.com/dahai9/CloudLedger/releases/latest' "$CLOUDLEDGER_OPS_ENV"
  assert_contains 'client_version = "0.1.5"' "$CLOUDLEDGER_SERVER_CONFIG"
  assert_contains 'min_supported_client_version = "0.1.5"' "$CLOUDLEDGER_SERVER_CONFIG"
  assert_contains 'client_download_url = "https://github.com/dahai9/CloudLedger/releases/latest"' "$CLOUDLEDGER_SERVER_CONFIG"
}

test_upgrade_failure_boundaries() {
  setup_case upgrade-pre-migration-failure
  seed_backup_fixture yes
  export CLOUDLEDGER_TEST_COMPOSE_FAIL_AT=pull
  printf '3\n3\nv0.1.5\nYES\n0\n0\n' | "$OPS" >"$CASE_ROOT/upgrade.out" 2>&1
  assert_contains 'CLOUDLEDGER_RELEASE_TAG=v0.1.4' "$CLOUDLEDGER_OPS_ENV"
  assert_contains 'client_version = "0.1.4"' "$CLOUDLEDGER_SERVER_CONFIG"
  assert_contains 'min_supported_client_version = "0.1.4"' "$CLOUDLEDGER_SERVER_CONFIG"
  assert_contains 'pre-migration-failure' "$CLOUDLEDGER_OPS_STATE_DIR/upgrade.log"
  assert_contains '已恢复旧镜像配置' "$CASE_ROOT/upgrade.out"

  setup_case upgrade-post-migration-failure
  seed_backup_fixture yes
  export CLOUDLEDGER_TEST_COMPOSE_FAIL_AT=migration
  printf '3\n3\nv0.1.5\nYES\n0\n0\n' | "$OPS" >"$CASE_ROOT/upgrade.out" 2>&1
  assert_contains 'CLOUDLEDGER_RELEASE_TAG=v0.1.5' "$CLOUDLEDGER_OPS_ENV"
  assert_contains 'client_version = "0.1.5"' "$CLOUDLEDGER_SERVER_CONFIG"
  assert_contains 'min_supported_client_version = "0.1.5"' "$CLOUDLEDGER_SERVER_CONFIG"
  assert_contains 'post-migration-failure' "$CLOUDLEDGER_OPS_STATE_DIR/upgrade.log"
  assert_contains '禁止盲目降级' "$CASE_ROOT/upgrade.out"
}

test_legacy_upgrade_adoption() {
  local legacy_snapshot secret_name secret_value server_before
  setup_case legacy-upgrade-reject-wrong-tag
  seed_legacy_upgrade_fixture
  sed -i 's/:v0\.1\.3$/:v0.1.4/' "$CLOUDLEDGER_OPS_ENV"
  printf '3\n3\nv0.1.5\nYES\n0\n0\n' | "$OPS" >"$CASE_ROOT/upgrade.out" 2>&1
  assert_contains '仅支持接管 server/PostgreSQL 镜像同为 v0.1.3 的旧部署' "$CASE_ROOT/upgrade.out"
  assert_contains 'CLOUDLEDGER_HTTP_HOST_PORT=18080' "$CLOUDLEDGER_OPS_ENV"
  assert_not_contains 'docker:manifest-inspect' "$CLOUDLEDGER_TEST_TRACE"

  setup_case legacy-upgrade-success
  seed_legacy_upgrade_fixture
  printf '3\n3\nv0.1.5\nYES\n0\n0\n' | "$OPS" >"$CASE_ROOT/upgrade.out" 2>&1
  assert_contains '已识别可受控接管的 CloudLedger v0.1.3 部署' "$CASE_ROOT/upgrade.out"
  assert_contains '配置已在回滚保护下规范化' "$CASE_ROOT/upgrade.out"
  assert_contains '升级完成' "$CASE_ROOT/upgrade.out"
  assert_not_contains 'awk:' "$CASE_ROOT/upgrade.out"
  assert_contains 'CLOUDLEDGER_GHCR_OWNER=cloudledger' "$CLOUDLEDGER_OPS_ENV"
  assert_contains 'CLOUDLEDGER_RELEASE_TAG=v0.1.5' "$CLOUDLEDGER_OPS_ENV"
  assert_contains 'CLOUDLEDGER_CADDY_IMAGE=ghcr.io/cloudledger/cloudledger-caddy:v0.1.5' "$CLOUDLEDGER_OPS_ENV"
  assert_contains 'CLOUDLEDGER_ANCHOR_IMAGE=ghcr.io/cloudledger/cloudledger-network-anchor:v0.1.5' "$CLOUDLEDGER_OPS_ENV"
  assert_contains 'CLOUDLEDGER_CLIENT_VERSION=0.1.5' "$CLOUDLEDGER_OPS_ENV"
  assert_contains 'CLOUDLEDGER_MIN_SUPPORTED_CLIENT_VERSION=0.1.5' "$CLOUDLEDGER_OPS_ENV"
  assert_contains 'client_version = "0.1.5"' "$CLOUDLEDGER_SERVER_CONFIG"
  assert_contains 'min_supported_client_version = "0.1.5"' "$CLOUDLEDGER_SERVER_CONFIG"
  assert_not_contains 'CLOUDLEDGER_HTTP_HOST_PORT=' "$CLOUDLEDGER_OPS_ENV"
  assert_not_contains 'CLOUDLEDGER_HTTPS_HOST_PORT=' "$CLOUDLEDGER_OPS_ENV"
  assert_not_contains 'CLOUDLEDGER_ADMIN_TUNNEL_PORT=' "$CLOUDLEDGER_OPS_ENV"
  cmp -s "$CLOUDLEDGER_DEPLOY_DIR/compose.yml" "$ROOT/deploy/docker-compose.yml" \
    || fail_test 'legacy adoption did not stage the trusted current Compose file'
  legacy_snapshot=$(find "$CLOUDLEDGER_OPS_STATE_DIR" -maxdepth 1 -type f \
    -name 'cloudledger-legacy-pre-upgrade-*.tar' -print | head -n1)
  [[ -s "$legacy_snapshot" ]] || fail_test 'legacy adoption did not preserve the pre-upgrade snapshot'
  assert_file_mode 600 "$legacy_snapshot"
  [[ $(count_final_backups) -eq 1 ]] || fail_test 'legacy adoption did not publish one new-format backup'
  assert_trace_order "$CLOUDLEDGER_TEST_TRACE" \
    'backup:docker-pg-dump' 'backup:docker-pg-dump' 'rclone:upload' 'rclone:download' \
    'rclone:publish' 'compose:pull' 'compose:migration' 'compose:audit' 'compose:up:backend' \
    'http:local-health' 'http:local-ready' 'compose:up:caddy' 'firewall:apply' 'systemd:daemon-reload'
  for secret_name in CLOUDLEDGER_BOOTSTRAP_DB_PASSWORD CLOUDLEDGER_MIGRATION_DB_PASSWORD \
    CLOUDLEDGER_RUNTIME_DB_PASSWORD CLOUDLEDGER_ADMIN_TOKEN CLOUDLEDGER_AUDIT_HMAC_KEY \
    CLOUDLEDGER_AUDIT_IDENTIFIER_HMAC_KEY CLOUDLEDGER_TURNSTILE_SECRET_KEY; do
    secret_value=$(bash -c 'source "$1"; printf "%s" "${!2}"' _ "$CLOUDLEDGER_OPS_ENV" "$secret_name")
    assert_secret_not_logged "$secret_value" "$CASE_ROOT/upgrade.out" "$CLOUDLEDGER_TEST_TRACE"
  done

  setup_case legacy-upgrade-rollback
  seed_legacy_upgrade_fixture
  server_before="$CASE_ROOT/server-before.toml"
  cp -- "$CLOUDLEDGER_SERVER_CONFIG" "$server_before"
  printf '%s\n' legacy-fixed-cert-before-upgrade >"$CLOUDLEDGER_CERT_DIR/origin-cert.pem"
  printf '%s\n' legacy-fixed-key-before-upgrade >"$CLOUDLEDGER_CERT_DIR/origin-key.pem"
  export CLOUDLEDGER_TEST_COMPOSE_FAIL_AT=pull
  printf '3\n3\nv0.1.5\nYES\n0\n0\n' | "$OPS" >"$CASE_ROOT/upgrade.out" 2>&1
  assert_contains '迁移尚未开始，已恢复旧镜像配置和部署资源' "$CASE_ROOT/upgrade.out"
  assert_contains 'CLOUDLEDGER_HTTP_HOST_PORT=18080' "$CLOUDLEDGER_OPS_ENV"
  assert_not_contains 'CLOUDLEDGER_GHCR_OWNER=' "$CLOUDLEDGER_OPS_ENV"
  cmp -s "$CLOUDLEDGER_DEPLOY_DIR/compose.yml" "$ROOT/deploy/legacy/compose-v0.1.3.yml" \
    || fail_test 'legacy pre-migration rollback did not restore the old Compose file'
  cmp -s "$CLOUDLEDGER_SERVER_CONFIG" "$server_before" \
    || fail_test 'legacy pre-migration rollback did not restore server.toml byte-for-byte'
  assert_contains 'legacy-fixed-cert-before-upgrade' "$CLOUDLEDGER_CERT_DIR/origin-cert.pem"
  assert_contains 'legacy-fixed-key-before-upgrade' "$CLOUDLEDGER_CERT_DIR/origin-key.pem"

  setup_case legacy-upgrade-signal
  seed_legacy_upgrade_fixture
  server_before="$CASE_ROOT/server-before.toml"
  cp -- "$CLOUDLEDGER_SERVER_CONFIG" "$server_before"
  printf '%s\n' legacy-installed-ops >"$CLOUDLEDGER_DEPLOY_DIR/cloudledger-ops.sh"
  chmod 755 "$CLOUDLEDGER_DEPLOY_DIR/cloudledger-ops.sh"
  printf '%s\n' legacy-installed-roles >"$CLOUDLEDGER_DEPLOY_DIR/postgres_roles.sql"
  export CLOUDLEDGER_TEST_SIGNAL_ON_COMPOSE=pull
  printf '3\n3\nv0.1.5\nYES\n0\n0\n' | "$OPS" >"$CASE_ROOT/upgrade.out" 2>&1
  assert_contains '迁移尚未开始，已恢复旧镜像配置和部署资源' "$CASE_ROOT/upgrade.out"
  assert_contains '已返回当前菜单' "$CASE_ROOT/upgrade.out"
  assert_contains 'CLOUDLEDGER_SERVER_IMAGE=ghcr.io/cloudledger/cloudledger-server:v0.1.3' "$CLOUDLEDGER_OPS_ENV"
  cmp -s "$CLOUDLEDGER_DEPLOY_DIR/compose.yml" "$ROOT/deploy/legacy/compose-v0.1.3.yml" \
    || fail_test 'legacy signal rollback did not restore the old Compose file'
  cmp -s "$CLOUDLEDGER_SERVER_CONFIG" "$server_before" \
    || fail_test 'legacy signal rollback did not restore server.toml byte-for-byte'
  assert_contains 'legacy-installed-ops' "$CLOUDLEDGER_DEPLOY_DIR/cloudledger-ops.sh"
  assert_contains 'legacy-installed-roles' "$CLOUDLEDGER_DEPLOY_DIR/postgres_roles.sql"

  setup_case legacy-upgrade-alpha-version
  seed_legacy_upgrade_fixture
  printf '3\n3\nv0.1.5.alpha.1\nYES\n0\n0\n' | "$OPS" >"$CASE_ROOT/upgrade.out" 2>&1
  assert_contains 'CLOUDLEDGER_CLIENT_VERSION=0.1.5-alpha.1' "$CLOUDLEDGER_OPS_ENV"
  assert_contains 'CLOUDLEDGER_MIN_SUPPORTED_CLIENT_VERSION=0.1.5-alpha.1' "$CLOUDLEDGER_OPS_ENV"
  assert_contains 'client_version = "0.1.5-alpha.1"' "$CLOUDLEDGER_SERVER_CONFIG"
  assert_contains 'min_supported_client_version = "0.1.5-alpha.1"' "$CLOUDLEDGER_SERVER_CONFIG"
}

test_restore_signal_rollback() {
  local archive name restore_count
  setup_case restore-signal
  seed_backup_fixture yes
  "$OPS" --internal backup >"$CASE_ROOT/backup.out" 2>&1
  archive=$(latest_archive)
  name=${archive##*/}
  : >"$CLOUDLEDGER_TEST_NFT_STATE"
  : >"$CLOUDLEDGER_TEST_TRACE"
  export CLOUDLEDGER_TEST_SIGNAL_ON_PG_RESTORE=1
  printf '4\n6\n%s\nYES\n%s\n0\n0\n' "$name" "$name" | "$OPS" >"$CASE_ROOT/restore.out" 2>&1
  restore_count=$(grep -Fc 'restore:pg-restore' "$CLOUDLEDGER_TEST_TRACE")
  [[ "$restore_count" -ge 2 ]] || fail_test 'signal during restore did not invoke database rollback'
  assert_contains '恢复事务未提交，正在回滚旧数据库' "$CASE_ROOT/restore.out"
  assert_contains '恢复事务已自动回滚到操作前状态' "$CASE_ROOT/restore.out"
  assert_contains '已返回当前菜单' "$CASE_ROOT/restore.out"
  if find "$CLOUDLEDGER_OPS_STATE_DIR" -maxdepth 1 -type d -name 'restore-rollback-failed-*' | grep -q .; then
    fail_test 'successful signal rollback incorrectly preserved a failed snapshot'
  fi
}

test_upgrade_signal_boundaries() {
  local preserved
  setup_case upgrade-signal-pre
  seed_backup_fixture yes
  export CLOUDLEDGER_TEST_SIGNAL_ON_COMPOSE=pull
  printf '3\n3\nv0.1.5\nYES\n0\n0\n' | "$OPS" >"$CASE_ROOT/upgrade.out" 2>&1
  assert_contains 'CLOUDLEDGER_RELEASE_TAG=v0.1.4' "$CLOUDLEDGER_OPS_ENV"
  assert_contains '迁移尚未开始，已恢复旧镜像配置' "$CASE_ROOT/upgrade.out"
  assert_contains '已返回当前菜单' "$CASE_ROOT/upgrade.out"

  setup_case upgrade-signal-post
  seed_backup_fixture yes
  export CLOUDLEDGER_TEST_SIGNAL_ON_COMPOSE=migration
  printf '3\n3\nv0.1.5\nYES\n0\n0\n' | "$OPS" >"$CASE_ROOT/upgrade.out" 2>&1
  assert_contains 'CLOUDLEDGER_RELEASE_TAG=v0.1.5' "$CLOUDLEDGER_OPS_ENV"
  assert_contains '迁移已经开始；禁止自动恢复旧镜像配置' "$CASE_ROOT/upgrade.out"
  preserved=$(find "$CLOUDLEDGER_OPS_STATE_DIR" -maxdepth 1 -type f -name 'upgrade-failed-old-ops-*.env' -print | head -n1)
  [[ -s "$preserved" ]] || fail_test 'post-migration signal did not preserve the old ops.env snapshot'
  assert_file_mode 600 "$preserved"
}

test_fail_closed_safety_primitives() {
  local restricted tool tool_path succeeded=0
  setup_case missing-flock
  restricted="$CASE_ROOT/no-flock-bin"
  mkdir -p "$restricted"
  for tool in dirname mkdir chmod; do
    tool_path=$(PATH="$ORIGINAL_PATH" command -v "$tool")
    ln -s "$tool_path" "$restricted/$tool"
  done
  if PATH="$restricted" "$REAL_BASH" "$OPS" --internal backup >"$CASE_ROOT/missing-flock.out" 2>&1; then
    succeeded=1
  fi
  (( succeeded == 0 )) || fail_test 'internal backup ran without flock in PATH'
  assert_contains '缺少 flock' "$CLOUDLEDGER_OPS_STATE_DIR/backup.log"
  assert_not_contains 'backup:docker-pg-dump' "$CLOUDLEDGER_TEST_TRACE"

  setup_case unsafe-directory-mode
  succeeded=0
  if CLOUDLEDGER_OPS_STATE_DIR=/proc CLOUDLEDGER_BACKUP_DIR=/proc \
    CLOUDLEDGER_FIREWALL_DIR=/proc "$REAL_BASH" "$OPS" </dev/null >"$CASE_ROOT/directory.out" 2>&1; then
    succeeded=1
  fi
  (( succeeded == 0 )) || fail_test 'toolbox started after protected directory chmod failed'
  assert_contains '目录权限设置为 0700' "$CASE_ROOT/directory.out"
  assert_not_contains 'CloudLedger 云服务运维工具箱' "$CASE_ROOT/directory.out"
}

test_sensitive_cleanup_fail_closed() {
  local succeeded=0
  setup_case plaintext-cleanup-failure
  seed_backup_fixture no
  export CLOUDLEDGER_TEST_RM_FAIL_PATTERN='/.tmp-'
  if "$OPS" --internal backup >"$CASE_ROOT/backup.out" 2>&1; then succeeded=1; fi
  unset CLOUDLEDGER_TEST_RM_FAIL_PATTERN
  cleanup_injected_rm_paths
  (( succeeded == 0 )) || fail_test 'backup succeeded after plaintext staging cleanup failed'
  assert_contains '明文备份暂存目录清理失败' "$CLOUDLEDGER_OPS_STATE_DIR/backup.log"
  [[ $(count_final_backups) -eq 0 ]] || fail_test 'plaintext cleanup failure published a final backup'

  setup_case verify-cleanup-failure
  seed_backup_fixture no
  succeeded=0
  export CLOUDLEDGER_TEST_RM_FAIL_PATTERN='/cloudledger-verify.'
  if "$OPS" --internal backup >"$CASE_ROOT/backup.out" 2>&1; then succeeded=1; fi
  unset CLOUDLEDGER_TEST_RM_FAIL_PATTERN
  cleanup_injected_rm_paths
  (( succeeded == 0 )) || fail_test 'backup succeeded after verification temp cleanup failed'
  assert_contains '敏感校验暂存目录清理失败' "$CLOUDLEDGER_OPS_STATE_DIR/backup.log"
  [[ $(count_final_backups) -eq 0 ]] || fail_test 'verification cleanup failure published a final backup'

  setup_case restore-temp-cleanup-failure
  seed_backup_fixture no
  "$OPS" --internal backup >"$CASE_ROOT/backup.out" 2>&1
  succeeded=0
  export CLOUDLEDGER_TEST_RM_FAIL_PATTERN='/cloudledger-restore-test.'
  if "$OPS" --internal restore-test >"$CASE_ROOT/restore.out" 2>&1; then succeeded=1; fi
  unset CLOUDLEDGER_TEST_RM_FAIL_PATTERN
  cleanup_injected_rm_paths
  (( succeeded == 0 )) || fail_test 'restore drill succeeded after sensitive temp cleanup failed'
  assert_contains '敏感临时目录清理失败' "$CASE_ROOT/restore.out"
  [[ ! -e "$CLOUDLEDGER_OPS_STATE_DIR/restore-test.log" ]] \
    || fail_test 'restore temp cleanup failure wrote a success record'
}

run_selected() {
  local name=$1 function=$2 filter=${CLOUDLEDGER_TEST_FILTER:-}
  [[ -z "$filter" || "$filter" == "$name" ]] || return 0
  "$function"
}

main() {
  run_selected numeric-menus test_numeric_menus
  run_selected hidden-pat test_hidden_pat
  run_selected complete-wizard test_complete_wizard
  run_selected backups test_backups
  run_selected restore-prerequisites test_restore_prerequisites
  run_selected firewall-internal test_firewall_internal_mode
  run_selected archive-guards test_archive_extraction_guards
  run_selected load-env test_load_env_clears_exported_values
  run_selected firewall-status test_firewall_status_integrity
  run_selected restore-trust test_restore_bundle_trust_boundary
  run_selected backup-freshness test_backup_identity_and_remote_freshness
  run_selected restore-passwords test_restore_with_archived_passwords
  run_selected restore-cleanup test_restore_cleanup_failure
  run_selected restore-core-tables test_restore_core_table_validation
  run_selected future-migrations test_future_migration_compatibility
  run_selected https-port test_https_port_preflight
  run_selected remote-signal test_remote_pending_signal_cleanup
  run_selected backup-path test_backup_path_confinement
  run_selected turnstile-secret test_turnstile_secret_probe
  run_selected admin-relay test_admin_relay_model
  run_selected systemd-units test_systemd_unit_contract
  run_selected diagnostic-redaction test_diagnostic_redaction
  run_selected rclone-redaction test_rclone_display_redaction
  run_selected upgrade-order test_upgrade_transaction_order
  run_selected upgrade-backfill test_upgrade_backfills_client_version_config
  run_selected upgrade-boundaries test_upgrade_failure_boundaries
  run_selected legacy-upgrade test_legacy_upgrade_adoption
  run_selected restore-signal test_restore_signal_rollback
  run_selected upgrade-signals test_upgrade_signal_boundaries
  run_selected safety-primitives test_fail_closed_safety_primitives
  run_selected cleanup-failures test_sensitive_cleanup_fail_closed
  printf 'cloudledger-ops isolated tests passed\n'
}

main "$@"
