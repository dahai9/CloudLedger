#!/usr/bin/env bash
# CloudLedger interactive operations toolbox.
# Public use is menu-only; --internal is reserved for installed systemd units.
set -Eeuo pipefail
IFS=$'\n\t'

readonly OPS_VERSION="v0.1.5"
SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
readonly SCRIPT_DIR
readonly DEPLOY_DIR="${CLOUDLEDGER_DEPLOY_DIR:-/opt/cloudledger}"
readonly STATE_DIR="${CLOUDLEDGER_OPS_STATE_DIR:-/var/lib/cloudledger-ops}"
readonly OPS_ENV="${CLOUDLEDGER_OPS_ENV:-/etc/cloudledger/ops.env}"
readonly COMPOSE_FILE="${CLOUDLEDGER_COMPOSE_FILE:-${DEPLOY_DIR}/compose.yml}"
readonly SERVER_CONFIG="${CLOUDLEDGER_SERVER_CONFIG:-/etc/cloudledger/server.toml}"
readonly CERT_DIR="${CLOUDLEDGER_CERT_DIR:-/etc/cloudledger/caddy}"
readonly RCLONE_CONFIG="${CLOUDLEDGER_RCLONE_CONFIG:-/etc/cloudledger/rclone.conf}"
readonly SYSTEMD_INSTALL_DIR="${CLOUDLEDGER_SYSTEMD_DIR:-/etc/systemd/system}"
readonly LOCK_FILE="${CLOUDLEDGER_OPS_LOCK:-${STATE_DIR}/ops.lock}"
readonly BACKUP_DIR="${CLOUDLEDGER_BACKUP_DIR:-${STATE_DIR}/backups}"
readonly FIREWALL_DIR="${CLOUDLEDGER_FIREWALL_DIR:-${STATE_DIR}/firewall}"
readonly REMOTE_BACKUP_CHECKPOINT="${STATE_DIR}/last-remote-backup"
readonly DEFAULT_API_URL="${CLOUDLEDGER_API_URL:-http://127.0.0.1:8787}"
readonly DEFAULT_MAX_BACKUP_BYTES=53687091200
readonly DEFAULT_REMOTE_BACKUP_MAX_AGE_HOURS=72
readonly CLOUDFLARE_IPV4_URL='https://www.cloudflare.com/ips-v4'
readonly CLOUDFLARE_IPV6_URL='https://www.cloudflare.com/ips-v6'
readonly -a OPS_CONFIG_KEYS=(
  CLOUDLEDGER_GHCR_OWNER CLOUDLEDGER_RELEASE_TAG
  CLOUDLEDGER_CLIENT_VERSION CLOUDLEDGER_MIN_SUPPORTED_CLIENT_VERSION CLOUDLEDGER_CLIENT_DOWNLOAD_URL
  CLOUDLEDGER_SERVER_IMAGE CLOUDLEDGER_POSTGRES_IMAGE CLOUDLEDGER_CADDY_IMAGE CLOUDLEDGER_ANCHOR_IMAGE
  CLOUDLEDGER_API_DOMAIN CLOUDLEDGER_HTTP_PUBLISH CLOUDLEDGER_HTTPS_PUBLISH
  CLOUDLEDGER_CADDY_ORIGIN_CERT_PATH CLOUDLEDGER_CADDY_ORIGIN_KEY_PATH
  CLOUDLEDGER_BOOTSTRAP_DB_PASSWORD CLOUDLEDGER_MIGRATION_DB_PASSWORD CLOUDLEDGER_RUNTIME_DB_PASSWORD
  CLOUDLEDGER_BOOTSTRAP_DATABASE_URL CLOUDLEDGER_MIGRATION_DATABASE_URL
  CLOUDLEDGER_ADMIN_PATH CLOUDLEDGER_ADMIN_TOKEN
  CLOUDLEDGER_AUDIT_KEY_ID CLOUDLEDGER_AUDIT_HMAC_KEY CLOUDLEDGER_AUDIT_IDENTIFIER_HMAC_KEY
  CLOUDLEDGER_TURNSTILE_SITE_KEY CLOUDLEDGER_TURNSTILE_SECRET_KEY
  CLOUDLEDGER_BACKUP_RETENTION CLOUDLEDGER_MAX_BACKUP_BYTES CLOUDLEDGER_REMOTE_BACKUP_MAX_AGE_HOURS
  CLOUDLEDGER_RCLONE_REMOTE
  CLOUDLEDGER_DISK_WARN CLOUDLEDGER_DISK_CRITICAL CLOUDLEDGER_MEMORY_WARN
  CLOUDLEDGER_CLOUDFLARE_IPV4_URL CLOUDLEDGER_CLOUDFLARE_IPV6_URL CLOUDLEDGER_WEBHOOK_URL
  CLOUDLEDGER_HTTP_HOST_PORT CLOUDLEDGER_HTTPS_HOST_PORT CLOUDLEDGER_ADMIN_TUNNEL_PORT
)
export RCLONE_CONFIG

if [[ -t 1 && -z "${NO_COLOR:-}" ]]; then
  readonly C_RESET=$'\033[0m' C_GREEN=$'\033[32m' C_YELLOW=$'\033[33m' C_RED=$'\033[31m'
else
  readonly C_RESET='' C_GREEN='' C_YELLOW='' C_RED=''
fi

ACTIVE_PLAINTEXT_DIR=''
ACTIVE_RESTORE_DB=''
ACTIVE_RESTORE_ROLLBACK_DIR=''
RESTORE_TRANSACTION_ACTIVE=0
ACTIVE_UPGRADE_ENV_SNAPSHOT=''
ACTIVE_UPGRADE_ASSET_SNAPSHOT=''
UPGRADE_MIGRATION_STARTED=0
ACTIVE_REMOTE_PENDING=''
SENSITIVE_PATHS=()
register_sensitive_path() { SENSITIVE_PATHS+=("$1"); }
unregister_sensitive_path() {
  local target=$1 path kept=()
  for path in "${SENSITIVE_PATHS[@]}"; do [[ "$path" == "$target" ]] || kept+=("$path"); done
  SENSITIVE_PATHS=("${kept[@]}")
}
remove_sensitive_path() {
  local path=$1
  [[ -n "$path" ]] || return 1
  if [[ -e "$path" || -L "$path" ]]; then
    if ! rm -rf -- "$path" 2>/dev/null; then
      fail "无法清理敏感临时路径: $path"
      return 1
    fi
  fi
  unregister_sensitive_path "$path"
}
cleanup_sensitive_paths() {
  local path failed=0 kept=()
  for path in "${SENSITIVE_PATHS[@]}"; do
    case "$path" in
      "$STATE_DIR"/*|"$CERT_DIR"/.cert-rollback.*|"$(dirname "$OPS_ENV")"/.ops.env.*|\
        "$(dirname "$SERVER_CONFIG")"/.server.toml.*|"${TMPDIR:-/tmp}"/cloudledger-*)
        if ! rm -rf -- "$path" 2>/dev/null; then
          kept+=("$path")
          failed=1
          fail "无法清理敏感临时路径: $path"
        fi
        ;;
      *) kept+=("$path"); failed=1; fail "拒绝清理边界外的敏感路径: $path" ;;
    esac
  done
  SENSITIVE_PATHS=("${kept[@]}")
  (( failed == 0 ))
}
cleanup_plaintext() {
  if [[ -n "$ACTIVE_PLAINTEXT_DIR" && -d "$ACTIVE_PLAINTEXT_DIR" ]]; then
    case "$ACTIVE_PLAINTEXT_DIR" in
      "$BACKUP_DIR"/.tmp-*|"${TMPDIR:-/tmp}"/cloudledger-*)
        rm -rf -- "$ACTIVE_PLAINTEXT_DIR" \
          || { fail "无法清理明文暂存目录: $ACTIVE_PLAINTEXT_DIR"; return 1; }
        ;;
      *) fail "拒绝清理边界外的明文目录: $ACTIVE_PLAINTEXT_DIR"; return 1 ;;
    esac
  fi
  ACTIVE_PLAINTEXT_DIR=''
}
cleanup_restore_database() {
  if [[ "$ACTIVE_RESTORE_DB" =~ ^cloudledger_restore_test_[0-9]+_[0-9]+$ ]]; then
    if ! postgres_drop_database "$ACTIVE_RESTORE_DB" >/dev/null 2>&1; then
      fail "无法删除临时恢复数据库: $ACTIVE_RESTORE_DB"
      return 1
    fi
  fi
  ACTIVE_RESTORE_DB=''
}
cleanup_remote_pending() {
  if [[ "$ACTIVE_REMOTE_PENDING" =~ ^[A-Za-z0-9_.-]+:.*/\.cloudledger-[0-9]{8}-[0-9]{6}-[0-9]+\.tar\.new$ ]]; then
    if ! rclone deletefile "$ACTIVE_REMOTE_PENDING" >/dev/null 2>&1; then
      fail "无法清理远程未发布备份对象: $ACTIVE_REMOTE_PENDING"
      return 1
    fi
  fi
  ACTIVE_REMOTE_PENDING=''
}
cleanup_all() {
  if (( RESTORE_TRANSACTION_ACTIVE == 1 )); then rollback_active_restore || true; fi
  if [[ -n "$ACTIVE_UPGRADE_ENV_SNAPSHOT" || -n "$ACTIVE_UPGRADE_ASSET_SNAPSHOT" ]]; then
    handle_active_upgrade_abort || true
  fi
  cleanup_restore_database || true
  cleanup_remote_pending || true
  cleanup_sensitive_paths || true
  cleanup_plaintext || true
}
install_cleanup_traps() {
  trap cleanup_all EXIT
  trap 'cleanup_all; exit 129' HUP
  trap 'cleanup_all; exit 130' INT
  trap 'cleanup_all; exit 143' TERM
}
install_cleanup_traps

log() { printf '%s\n' "$*"; }
ok() { printf '%s%s%s\n' "$C_GREEN" "$*" "$C_RESET"; }
warn() { printf '%s%s%s\n' "$C_YELLOW" "$*" "$C_RESET"; }
fail() { printf '%s%s%s\n' "$C_RED" "$*" "$C_RESET" >&2; }
die() { fail "$*"; exit 1; }
pause() { [[ -t 0 ]] || return 0; read -r -p '按 Enter 返回...' _ || true; }
command_exists() { command -v "$1" >/dev/null 2>&1; }

menu_action() {
  local rc
  ( set -Eeuo pipefail; install_cleanup_traps; "$@" )
  rc=$?
  if (( rc == 0 )); then ok '操作成功。'; else fail "操作失败（退出码 $rc），已返回当前菜单。"; fi
  return 0
}

ensure_dirs() {
  mkdir -p "$STATE_DIR" "$BACKUP_DIR" "$FIREWALL_DIR" \
    || { fail '无法创建运维状态、备份或防火墙目录。'; return 1; }
  chmod 700 "$STATE_DIR" "$BACKUP_DIR" "$FIREWALL_DIR" \
    || { fail '无法将运维状态、备份和防火墙目录权限设置为 0700。'; return 1; }
}

ensure_config_dir() {
  local config_dir
  config_dir=$(dirname "$OPS_ENV") || return 1
  mkdir -p "$config_dir" || { fail "无法创建运维配置目录: $config_dir"; return 1; }
  chmod 700 "$config_dir" || { fail "无法将运维配置目录权限设置为 0700: $config_dir"; return 1; }
}

is_ops_config_key() {
  local requested=$1 key
  for key in "${OPS_CONFIG_KEYS[@]}"; do [[ "$requested" == "$key" ]] && return 0; done
  return 1
}

load_env() {
  local line key raw value seen=' '
  local -A parsed=()
  for key in "${OPS_CONFIG_KEYS[@]}"; do unset "$key"; done
  [[ -r "$OPS_ENV" ]] || return 0
  while IFS= read -r line || [[ -n "$line" ]]; do
    [[ -z "$line" || "$line" == \#* ]] && continue
    if [[ ! "$line" =~ ^(CLOUDLEDGER_[A-Z0-9_]+)=(.*)$ ]]; then
      fail "运维配置包含不安全语法: $OPS_ENV"
      return 1
    fi
    key=${BASH_REMATCH[1]}
    raw=${BASH_REMATCH[2]}
    if ! is_ops_config_key "$key" || [[ "$seen" == *" $key "* ]] || ! decode_ops_env_value value "$raw"; then
      fail "运维配置包含未知键、重复键或无效转义: $OPS_ENV"
      return 1
    fi
    parsed["$key"]=$value
    seen+="$key "
  done <"$OPS_ENV"
  for key in "${OPS_CONFIG_KEYS[@]}"; do
    if [[ -n "${parsed[$key]+present}" ]]; then
      printf -v "$key" '%s' "${parsed[$key]}"
      export "${key?}"
    fi
  done
}

require_root() {
  if [[ ${EUID:-$(id -u)} -ne 0 && ${CLOUDLEDGER_ALLOW_NONROOT:-0} != 1 ]]; then
    die '此操作需要 root。请使用 sudo 运行。'
  fi
}

with_lock() {
  local rc
  ensure_dirs || return 1
  command_exists flock || { fail '缺少 flock，无法安全串行执行运维事务。'; return 1; }
  if ! exec 9>"$LOCK_FILE"; then
    fail "无法打开运维锁文件: $LOCK_FILE"
    return 1
  fi
  if ! flock -n 9; then
    exec 9>&-
    warn '另一个升级、备份、恢复或权限任务正在运行。'
    return 1
  fi
  "$@"
  rc=$?
  if ! flock -u 9; then
    fail '运维事务完成，但释放 flock 锁失败。'
    rc=1
  fi
  exec 9>&-
  return "$rc"
}

press_confirm() {
  local answer
  read -r -p "${1:-确认执行} (输入 YES): " answer || return 1
  [[ "$answer" == YES ]]
}

hidden_read() {
  local variable=$1 prompt=$2 value
  read -r -s -p "$prompt" value || return 1
  printf '\n' >&2
  printf -v "$variable" '%s' "$value"
}

read_choice() {
  local variable=$1 prompt=$2 pattern=$3 value
  while :; do
    read -r -p "$prompt" value || return 1
    if [[ "$value" =~ $pattern ]]; then
      printf -v "$variable" '%s' "$value"
      return 0
    fi
    warn '请输入菜单中的数字。'
  done
}

valid_tag() { [[ "$1" =~ ^v?[0-9]+\.[0-9]+\.[0-9]+([._-][A-Za-z0-9.-]+)?$ && "$1" != latest ]]; }
valid_client_version() { [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z][0-9A-Za-z.-]*)?(\+[0-9A-Za-z][0-9A-Za-z.-]*)?$ ]]; }
client_version_for_tag() {
  local version=${1#v}
  printf '%s' "${version//.alpha./-alpha.}"
}
backfill_client_version_config() {
  load_env || return 1
  local release_version
  release_version=$(client_version_for_tag "${CLOUDLEDGER_RELEASE_TAG:-}")
  valid_client_version "$release_version" || {
    fail '当前 release tag 无法转换为客户端 SemVer，无法兼容升级。'
    return 1
  }
  if [[ -z "${CLOUDLEDGER_CLIENT_VERSION:-}" ]]; then
    set_env_value CLOUDLEDGER_CLIENT_VERSION "$release_version" || return 1
  fi
  if [[ -z "${CLOUDLEDGER_MIN_SUPPORTED_CLIENT_VERSION:-}" ]]; then
    set_env_value CLOUDLEDGER_MIN_SUPPORTED_CLIENT_VERSION "${CLOUDLEDGER_CLIENT_VERSION:-$release_version}" || return 1
  fi
  if [[ -z "${CLOUDLEDGER_CLIENT_DOWNLOAD_URL:-}" ]]; then
    set_env_value CLOUDLEDGER_CLIENT_DOWNLOAD_URL 'https://github.com/dahai9/CloudLedger/releases/latest' || return 1
  fi
  load_env
  render_server_config
}
valid_download_url() { [[ "$1" =~ ^https://[^[:space:]\"\\]+$ ]]; }
valid_domain() { [[ "$1" =~ ^[A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?$ && "$1" == *.* ]]; }
valid_ghcr_image() {
  local image=$1 package=$2 tag=$3
  [[ "$image" =~ ^ghcr\.io/[a-z0-9_.-]+/${package}:[A-Za-z0-9._-]+$ && "${image##*:}" == "$tag" ]]
}

set_env_value() {
  local key=$1 value=$2 temp line
  is_ops_config_key "$key" || { fail '拒绝写入不受支持的配置键。'; return 1; }
  [[ "$value" != *[![:print:]]* ]] || { fail '配置值不能包含控制字符。'; return 1; }
  ensure_config_dir || return 1
  temp=$(mktemp "$(dirname "$OPS_ENV")/.ops.env.XXXXXX") || return 1
  register_sensitive_path "$temp"
  chmod 600 "$temp" || { remove_sensitive_path "$temp"; return 1; }
  if [[ -r "$OPS_ENV" ]]; then
    while IFS= read -r line || [[ -n "$line" ]]; do
      if [[ "${line%%=*}" != "$key" ]]; then
        printf '%s\n' "$line" >>"$temp" || { remove_sensitive_path "$temp"; return 1; }
      fi
    done <"$OPS_ENV" || { remove_sensitive_path "$temp"; return 1; }
  fi
  printf '%s=%q\n' "$key" "$value" >>"$temp" || { remove_sensitive_path "$temp"; return 1; }
  mv -f -- "$temp" "$OPS_ENV" || { remove_sensitive_path "$temp"; return 1; }
  unregister_sensitive_path "$temp"
  chmod 600 "$OPS_ENV" || return 1
}

remove_env_value() {
  local key=$1 temp line
  is_ops_config_key "$key" || return 1
  [[ -r "$OPS_ENV" ]] || return 0
  ensure_config_dir || return 1
  temp=$(mktemp "$(dirname "$OPS_ENV")/.ops.env.XXXXXX") || return 1
  register_sensitive_path "$temp"
  chmod 600 "$temp" || { remove_sensitive_path "$temp"; return 1; }
  while IFS= read -r line || [[ -n "$line" ]]; do
    [[ "${line%%=*}" == "$key" ]] || printf '%s\n' "$line" >>"$temp" \
      || { remove_sensitive_path "$temp"; return 1; }
  done <"$OPS_ENV"
  mv -f -- "$temp" "$OPS_ENV" || { remove_sensitive_path "$temp"; return 1; }
  unregister_sensitive_path "$temp"
  chmod 600 "$OPS_ENV"
}

decode_ops_env_value() {
  local variable=$1 raw=$2 output='' char
  [[ "$raw" == "''" ]] && { printf -v "$variable" '%s' ''; return 0; }
  while [[ -n "$raw" ]]; do
    char=${raw:0:1}
    raw=${raw:1}
    if [[ "$char" == $'\\' ]]; then
      [[ -n "$raw" ]] || return 1
      char=${raw:0:1}
      raw=${raw:1}
      [[ "$char" == [[:print:]] && "$char" != $'\r' ]] || return 1
    else
      case "$char" in [A-Za-z0-9_@%+=:,./-]) ;; *) return 1 ;; esac
    fi
    output+=$char
  done
  printf -v "$variable" '%s' "$output"
}

normalize_ops_env_file() {
  local source=$1 target=$2 line key raw value seen=' ' count=0
  : >"$target" || return 1
  chmod 600 "$target" || return 1
  while IFS= read -r line || [[ -n "$line" ]]; do
    [[ -z "$line" || "$line" == \#* ]] && continue
    [[ "$line" =~ ^(CLOUDLEDGER_[A-Z0-9_]+)=(.*)$ ]] || return 1
    key=${BASH_REMATCH[1]}
    raw=${BASH_REMATCH[2]}
    is_ops_config_key "$key" || return 1
    [[ "$seen" != *" $key "* ]] || return 1
    decode_ops_env_value value "$raw" || return 1
    printf '%s=%q\n' "$key" "$value" >>"$target" || return 1
    seen+="$key "
    count=$((count + 1))
  done <"$source"
  (( count > 0 )) || return 1
}

ops_env_file_value() {
  local source=$1 requested=$2 variable=$3 line key raw decoded
  while IFS= read -r line || [[ -n "$line" ]]; do
    [[ "$line" =~ ^(CLOUDLEDGER_[A-Z0-9_]+)=(.*)$ ]] || continue
    key=${BASH_REMATCH[1]}
    [[ "$key" == "$requested" ]] || continue
    raw=${BASH_REMATCH[2]}
    decode_ops_env_value decoded "$raw" || return 1
    printf -v "$variable" '%s' "$decoded"
    return 0
  done <"$source"
  return 1
}

compose() {
  load_env || return 1
  command_exists docker || { fail '未找到 Docker。'; return 127; }
  docker compose --project-directory "$DEPLOY_DIR" -f "$COMPOSE_FILE" "$@"
}

current_version() {
  printf '%s' "${CLOUDLEDGER_RELEASE_TAG:-$OPS_VERSION}"
}

show_image_versions() {
  local images container repository tag platform image_id size created created_display size_display
  images=$(compose images --format json) || {
    fail '无法读取 Compose 镜像 JSON 信息。'
    return 1
  }
  [[ -n "$images" && "$images" != '[]' ]] || { warn '当前没有可显示的 CloudLedger 镜像。'; return 0; }
  printf '%-36s %-44s %-24s %-16s %-14s %-12s %s\n' \
    'CONTAINER' 'REPOSITORY' 'TAG' 'PLATFORM' 'IMAGE ID' 'SIZE' 'CREATED (UTC)'
  while IFS=$'\t' read -r container repository tag platform image_id size created; do
    [[ -n "$container" ]] || continue
    image_id=${image_id#sha256:}
    image_id=${image_id:0:12}
    created_display=$(date -u -d "$created" '+%Y-%m-%d %H:%M:%S UTC' 2>/dev/null || printf '%s' "$created")
    if command_exists numfmt; then
      size_display=$(numfmt --to=si --suffix=B "$size" 2>/dev/null || printf '%s bytes' "$size")
    else
      size_display=$(printf '%s bytes' "$size")
    fi
    printf '%-36s %-44s %-24s %-16s %-14s %-12s %s\n' \
      "$container" "$repository" "$tag" "$platform" "$image_id" "$size_display" "$created_display"
  done < <(jq -r '.[] | [.ContainerName, .Repository, .Tag, .Platform, .ID, (.Size | tostring), .LastTagTime] | @tsv' <<<"$images")
}

api_base_url() {
  load_env || return 1
  if [[ -n "${CLOUDLEDGER_API_URL:-}" ]]; then
    printf '%s' "$CLOUDLEDGER_API_URL"
  elif [[ -n "${CLOUDLEDGER_API_DOMAIN:-}" ]]; then
    printf 'https://%s' "$CLOUDLEDGER_API_DOMAIN"
  else
    printf '%s' "$DEFAULT_API_URL"
  fi
}

certificate_file() {
  load_env || return 1
  printf '%s' "${CLOUDLEDGER_CADDY_ORIGIN_CERT_PATH:-$CERT_DIR/origin-cert.pem}"
}

certificate_key_file() {
  load_env || return 1
  printf '%s' "${CLOUDLEDGER_CADDY_ORIGIN_KEY_PATH:-$CERT_DIR/origin-key.pem}"
}

valid_backup_name() { [[ "$1" =~ ^cloudledger-[0-9]{8}-[0-9]{6}-[0-9]+\.tar$ ]]; }

backup_id_from_archive() {
  local name=${1##*/}
  if [[ "$name" =~ ^cloudledger-([0-9]{8}-[0-9]{6}-[0-9]+)\.tar$ \
    || "$name" =~ ^\.cloudledger-([0-9]{8}-[0-9]{6}-[0-9]+)\.tar\.new$ ]]; then
    printf '%s' "${BASH_REMATCH[1]}"
    return 0
  fi
  return 1
}

backup_timestamp_from_name() {
  local id
  id=$(backup_id_from_archive "$1") || return 1
  printf '%s' "${id%-*}"
}

current_backup() {
  local _timestamp file name
  while IFS=$'\t' read -r _timestamp file; do
    name=${file##*/}
    if valid_backup_name "$name" && [[ ! -L "$file" ]]; then printf '%s' "$file"; return 0; fi
  done < <(find "$BACKUP_DIR" -maxdepth 1 -type f -name 'cloudledger-*.tar' -printf '%T@\t%p\n' 2>/dev/null | sort -nr)
  return 0
}

disk_percent() { df -P "$STATE_DIR" 2>/dev/null | awk 'NR==2 {gsub(/%/, "", $5); print $5}'; }

health_url() {
  local name=$1 url=$2
  if command_exists curl && curl -fsS --max-time 10 "$url" >/dev/null; then
    ok "$name: 正常"
  else
    fail "$name: 异常"
    return 1
  fi
}

service_state() {
  if command_exists docker && [[ -f "$COMPOSE_FILE" ]]; then
    compose ps --format 'table {{.Name}}\t{{.Service}}\t{{.State}}\t{{.Health}}' 2>/dev/null || true
  else
    warn 'Docker Compose 文件或 Docker 不可用。'
  fi
}

certificate_status() {
  local cert expiry now days
  cert=$(certificate_file)
  [[ -f "$cert" ]] || { warn '未找到 Origin CA 证书。'; return 1; }
  expiry=$(openssl x509 -in "$cert" -enddate -noout | cut -d= -f2) || return 1
  now=$(date +%s)
  days=$(( ($(date -d "$expiry" +%s) - now) / 86400 ))
  if (( days < 30 )); then
    warn "Origin CA 证书剩余 ${days} 天（低于 30 天阈值）。"
  else
    ok "Origin CA 证书剩余 ${days} 天。"
  fi
}

header() {
  load_env || true
  local svc='异常' db='异常' https='异常' backup='无' disk='?'
  if command_exists docker && [[ -f "$COMPOSE_FILE" ]] && compose ps --status running --services 2>/dev/null | grep -qx cloudledger; then svc='正常'; fi
  if command_exists docker && [[ -f "$COMPOSE_FILE" ]] && compose ps --status running --services 2>/dev/null | grep -qx postgres; then db='正常'; fi
  if command_exists curl && curl -fsS --max-time 3 "$(api_base_url)/health" >/dev/null 2>&1; then https='正常'; fi
  if [[ -n "$(current_backup)" ]]; then backup=$(date -r "$(current_backup)" '+%F %H:%M' 2>/dev/null || printf '已创建'); fi
  disk=$(disk_percent || true); [[ -n "$disk" ]] || disk='?'
  printf '%s\n' '====================================================' ' CloudLedger 云服务运维工具箱' '===================================================='
  printf ' 当前版本: %s\n 服务状态: %s\n 数据库: %s\n HTTPS: %s\n 最近备份: %s\n 磁盘使用: %s%%\n' "$(current_version)" "$svc" "$db" "$https" "$backup" "$disk"
  if [[ "$disk" =~ ^[0-9]+$ ]]; then
    if (( disk >= ${CLOUDLEDGER_DISK_CRITICAL:-90} )); then
      fail " 磁盘告警: 已达到严重阈值 ${CLOUDLEDGER_DISK_CRITICAL:-90}%"
    elif (( disk >= ${CLOUDLEDGER_DISK_WARN:-80} )); then
      warn " 磁盘告警: 已达到警告阈值 ${CLOUDLEDGER_DISK_WARN:-80}%"
    fi
  fi
  printf '%s\n' '===================================================='
}

check_requirements() {
  local missing=() tool failed=0
  log '检查部署要求...'
  for tool in bash docker curl jq openssl tar sha256sum flock rclone nft ss systemctl; do
    command_exists "$tool" || missing+=("$tool")
  done
  if [[ ${#missing[@]} -eq 0 ]]; then ok '基础工具齐全。'; else warn "缺少工具: ${missing[*]}"; failed=1; fi
  if command_exists docker && docker compose version >/dev/null 2>&1; then ok 'Docker Compose plugin: 可用'; else warn 'Docker Compose plugin: 不可用'; failed=1; fi
  if command_exists systemctl && [[ "$(ps -p 1 -o comm= 2>/dev/null)" == systemd ]]; then ok 'systemd: 可用'; else warn 'systemd: 不可用'; failed=1; fi
  [[ -f "$COMPOSE_FILE" || -f "$SCRIPT_DIR/docker-compose.yml" ]] || { warn '未找到 Compose 部署资源。'; failed=1; }
  return "$failed"
}

install_dependencies() {
  require_root
  local distro family arch need_helpers=0 docker_missing=0 compose_missing=0 tool
  # shellcheck disable=SC1091
  [[ -r /etc/os-release ]] && source /etc/os-release || return 1
  distro=${ID:-}
  for tool in curl jq openssl tar sha256sum flock rclone nft ss; do
    command_exists "$tool" || need_helpers=1
  done
  command_exists docker || docker_missing=1
  if (( docker_missing == 0 )) && ! docker compose version >/dev/null 2>&1; then compose_missing=1; fi
  if command_exists apt-get; then
    if (( need_helpers == 1 )); then
      apt-get update || return 1
      apt-get install -y ca-certificates curl gnupg postgresql-client rclone jq openssl tar nftables util-linux iproute2 coreutils || return 1
    fi
    if (( docker_missing == 1 || compose_missing == 1 )); then
      if (( docker_missing == 0 )); then
        warn '将为现有 Docker 安装官方 Compose plugin；包管理器可能调整 Docker CLI 软件包，但不会主动重启 Docker daemon。'
        docker ps --format '  {{.Names}} ({{.Image}})' 2>/dev/null || true
        press_confirm '确认配置 Docker 官方源并安装 Compose plugin' || return 1
      else
        warn '系统未检测到 Docker，将安装 Docker Engine 并启动 docker.service。'
      fi
      [[ "$distro" == debian || "$distro" == ubuntu ]] \
        || { fail "Docker 官方 apt 源不支持当前发行版 ID: $distro"; return 1; }
      install -d -m 0755 /etc/apt/keyrings || return 1
      curl -fsSL "https://download.docker.com/linux/$distro/gpg" -o /etc/apt/keyrings/docker.asc || return 1
      chmod 0644 /etc/apt/keyrings/docker.asc || return 1
      arch=$(dpkg --print-architecture) || return 1
      printf 'deb [arch=%s signed-by=%s] https://download.docker.com/linux/%s %s stable\n' \
        "$arch" /etc/apt/keyrings/docker.asc "$distro" "${VERSION_CODENAME:?缺少 VERSION_CODENAME}" \
        > /etc/apt/sources.list.d/docker.list || return 1
      apt-get update || return 1
      if (( docker_missing == 1 )); then
        apt-get install -y docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin || return 1
      else
        apt-get install -y docker-compose-plugin || return 1
      fi
    fi
  elif command_exists dnf; then
    if (( need_helpers == 1 )); then
      dnf install -y ca-certificates curl postgresql rclone jq openssl tar nftables util-linux iproute coreutils || return 1
    fi
    if (( docker_missing == 1 || compose_missing == 1 )); then
      if (( docker_missing == 0 )); then
        warn '将为现有 Docker 安装官方 Compose plugin；包管理器可能调整 Docker CLI 软件包，但不会主动重启 Docker daemon。'
        docker ps --format '  {{.Names}} ({{.Image}})' 2>/dev/null || true
        press_confirm '确认配置 Docker 官方源并安装 Compose plugin' || return 1
      else
        warn '系统未检测到 Docker，将安装 Docker Engine 并启动 docker.service。'
      fi
      dnf install -y dnf-plugins-core || return 1
      case "$distro" in fedora) family=fedora;; *) family=centos;; esac
      dnf config-manager --add-repo "https://download.docker.com/linux/$family/docker-ce.repo" || return 1
      if (( docker_missing == 1 )); then
        dnf install -y docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin || return 1
      else
        dnf install -y docker-compose-plugin || return 1
      fi
    fi
  else
    fail '仅支持 Debian、Ubuntu、RHEL、Rocky Linux 等 systemd 发行版。'
    return 1
  fi
  if (( docker_missing == 1 )); then systemctl enable --now docker || return 1; fi
  command_exists docker && docker compose version >/dev/null 2>&1 \
    || { fail 'Docker Compose plugin 安装后仍不可用。'; return 1; }
  ok '所缺少的 Docker/Compose 或辅助工具已安装；未重启既有 Docker daemon。'
}

atomic_copy() {
  local source=$1 target=$2 mode=$3 temp
  [[ -f "$source" ]] || { fail "缺少部署资源: $source"; return 1; }
  mkdir -p "$(dirname "$target")" || return 1
  temp=$(mktemp "$(dirname "$target")/.asset.XXXXXX") || return 1
  if ! install -m "$mode" "$source" "$temp" || ! mv -f -- "$temp" "$target"; then
    rm -f -- "$temp"
    return 1
  fi
}

stage_assets() {
  require_root
  local compose_source unit
  compose_source="$SCRIPT_DIR/docker-compose.yml"
  [[ -f "$compose_source" ]] || compose_source="$SCRIPT_DIR/compose.yml"
  atomic_copy "$compose_source" "$DEPLOY_DIR/compose.yml" 0644 || return 1
  atomic_copy "$SCRIPT_DIR/Caddyfile" "$DEPLOY_DIR/Caddyfile" 0644 || return 1
  atomic_copy "$SCRIPT_DIR/postgres_roles.sql" "$DEPLOY_DIR/postgres_roles.sql" 0600 || return 1
  atomic_copy "${BASH_SOURCE[0]}" "$DEPLOY_DIR/cloudledger-ops.sh" 0755 || return 1
  mkdir -p "$DEPLOY_DIR/legacy" || return 1
  atomic_copy "$SCRIPT_DIR/legacy/compose-v0.1.3.yml" \
    "$DEPLOY_DIR/legacy/compose-v0.1.3.yml" 0644 || return 1
  mkdir -p "$DEPLOY_DIR/systemd" || return 1
  for unit in "$SCRIPT_DIR"/systemd/cloudledger-ops-*; do
    [[ -f "$unit" ]] || continue
    atomic_copy "$unit" "$DEPLOY_DIR/systemd/$(basename "$unit")" 0644 || return 1
  done
  ok "部署资源已原子暂存到 $DEPLOY_DIR。"
}

ghcr_login() {
  require_root
  local registry user pat
  read -r -p 'GHCR registry [ghcr.io]: ' registry
  registry=${registry:-ghcr.io}
  read -r -p 'GitHub 用户名: ' user
  [[ -n "$user" ]] || { fail 'GitHub 用户名不能为空。'; return 1; }
  hidden_read pat 'GitHub PAT（隐藏）: '
  [[ -n "$pat" ]] || { fail 'PAT 不能为空。'; return 1; }
  if ! printf '%s' "$pat" | docker login "$registry" -u "$user" --password-stdin; then unset pat; return 1; fi
  unset pat
  ok 'GHCR 登录完成。'
}

configure_registry_access() {
  local choice
  printf '%s\n' '1. 使用公开 GHCR 镜像（匿名拉取）' '2. 登录私有 GHCR 镜像' '0. 取消'
  read_choice choice '请选择: ' '^[012]$' || return 1
  case "$choice" in 1) ok '将使用匿名 GHCR 拉取。';; 2) ghcr_login;; 0) return 1;; esac
}

choose_tag() {
  local tag
  read -r -p '输入明确的 Git tag（禁止 latest）: ' tag
  valid_tag "$tag" || { fail 'tag 必须是明确的版本号，且不能使用 latest。'; return 1; }
  printf '%s' "$tag"
}

configure_images() {
  local owner tag client_version min_supported_version download_url
  load_env || return 1
  read -r -p "GHCR 仓库所有者 [${CLOUDLEDGER_GHCR_OWNER:-dahai9}]: " owner
  owner=${owner:-${CLOUDLEDGER_GHCR_OWNER:-dahai9}}
  [[ "$owner" =~ ^[a-z0-9_.-]+$ ]] || { fail 'GHCR 仓库所有者必须使用小写字母、数字、点、下划线或连字符。'; return 1; }
  tag=$(choose_tag) || return 1
  client_version=$(client_version_for_tag "$tag")
  read -r -p "最低支持客户端版本 [${client_version}]: " min_supported_version
  min_supported_version=${min_supported_version:-$client_version}
  min_supported_version=${min_supported_version#v}
  min_supported_version=${min_supported_version//.alpha./-alpha.}
  valid_client_version "$min_supported_version" || { fail '最低支持客户端版本必须是 SemVer。'; return 1; }
  read -r -p '官方下载地址 [https://github.com/dahai9/CloudLedger/releases/latest]: ' download_url
  download_url=${download_url:-https://github.com/dahai9/CloudLedger/releases/latest}
  valid_download_url "$download_url" || { fail '官方下载地址必须是 HTTPS URL，且不能包含空格或引号。'; return 1; }
  set_env_value CLOUDLEDGER_GHCR_OWNER "$owner" || return 1
  set_env_value CLOUDLEDGER_RELEASE_TAG "$tag" || return 1
  set_env_value CLOUDLEDGER_CLIENT_VERSION "$client_version" || return 1
  set_env_value CLOUDLEDGER_MIN_SUPPORTED_CLIENT_VERSION "$min_supported_version" || return 1
  set_env_value CLOUDLEDGER_CLIENT_DOWNLOAD_URL "$download_url" || return 1
  set_env_value CLOUDLEDGER_SERVER_IMAGE "ghcr.io/$owner/cloudledger-server:$tag" || return 1
  set_env_value CLOUDLEDGER_POSTGRES_IMAGE "ghcr.io/$owner/cloudledger-postgres:$tag" || return 1
  set_env_value CLOUDLEDGER_CADDY_IMAGE "ghcr.io/$owner/cloudledger-caddy:$tag" || return 1
  set_env_value CLOUDLEDGER_ANCHOR_IMAGE "ghcr.io/$owner/cloudledger-network-anchor:$tag" || return 1
  ok "已固定镜像版本: $tag"
}

random_config_key() { openssl rand -base64 32 | tr '/+' '_-' | tr -d '=\n'; }

generate_passwords() {
  require_root
  local choice bootstrap migration runtime admin_path admin_token audit_key_id audit_hmac identifier_hmac
  load_env || return 1
  if [[ -n "${CLOUDLEDGER_BOOTSTRAP_DB_PASSWORD:-}" && -n "${CLOUDLEDGER_MIGRATION_DB_PASSWORD:-}" && -n "${CLOUDLEDGER_RUNTIME_DB_PASSWORD:-}" ]]; then
    printf '%s\n' '1. 保留现有部署密钥' '2. 重新生成全部部署密钥' '0. 取消'
    read_choice choice '请选择: ' '^[012]$' || return 1
    [[ "$choice" == 1 ]] && { ok '已保留现有密钥。'; return 0; }
    [[ "$choice" == 0 ]] && return 1
    warn '重新生成数据库密码后，已有数据卷必须同步轮换数据库角色。'
  fi
  bootstrap=$(openssl rand -hex 24) || return 1
  migration=$(openssl rand -hex 24) || return 1
  runtime=$(openssl rand -hex 24) || return 1
  admin_path="manage-$(openssl rand -hex 16)" || return 1
  admin_token="admin_$(openssl rand -hex 32)" || return 1
  audit_key_id="audit-$(openssl rand -hex 12)" || return 1
  audit_hmac=$(random_config_key) || return 1
  identifier_hmac=$(random_config_key) || return 1
  set_env_value CLOUDLEDGER_BOOTSTRAP_DB_PASSWORD "$bootstrap" || return 1
  set_env_value CLOUDLEDGER_MIGRATION_DB_PASSWORD "$migration" || return 1
  set_env_value CLOUDLEDGER_RUNTIME_DB_PASSWORD "$runtime" || return 1
  set_env_value CLOUDLEDGER_BOOTSTRAP_DATABASE_URL "postgres://cloudledger_bootstrap:${bootstrap}@127.0.0.1:5432/cloudledger" || return 1
  set_env_value CLOUDLEDGER_MIGRATION_DATABASE_URL "postgres://cloudledger_migration:${migration}@127.0.0.1:5432/cloudledger" || return 1
  set_env_value CLOUDLEDGER_ADMIN_PATH "$admin_path" || return 1
  set_env_value CLOUDLEDGER_ADMIN_TOKEN "$admin_token" || return 1
  set_env_value CLOUDLEDGER_AUDIT_KEY_ID "$audit_key_id" || return 1
  set_env_value CLOUDLEDGER_AUDIT_HMAC_KEY "$audit_hmac" || return 1
  set_env_value CLOUDLEDGER_AUDIT_IDENTIFIER_HMAC_KEY "$identifier_hmac" || return 1
  unset bootstrap migration runtime admin_path admin_token audit_key_id audit_hmac identifier_hmac
  ok '数据库、管理端和审计密钥已生成并写入权限 0600 的运维配置。'
}

set_domains() {
  require_root
  local api
  load_env || return 1
  read -r -p "API 域名 [${CLOUDLEDGER_API_DOMAIN:-}]: " api
  api=${api:-${CLOUDLEDGER_API_DOMAIN:-}}
  valid_domain "$api" || { fail 'API 域名格式不合法。'; return 1; }
  set_env_value CLOUDLEDGER_API_DOMAIN "$api" || return 1
  set_env_value CLOUDLEDGER_HTTP_PUBLISH '127.0.0.1:18080:80' || return 1
  set_env_value CLOUDLEDGER_HTTPS_PUBLISH '443:443' || return 1
  ok 'API 使用 Cloudflare HTTPS；管理端固定为 127.0.0.1:8788，仅通过 SSH 隧道访问。'
}

configure_turnstile() {
  require_root
  local site secret
  load_env || return 1
  read -r -p "Turnstile site key [${CLOUDLEDGER_TURNSTILE_SITE_KEY:+已配置，回车保留}]: " site
  if [[ -z "$site" && -n "${CLOUDLEDGER_TURNSTILE_SITE_KEY:-}" ]]; then site=$CLOUDLEDGER_TURNSTILE_SITE_KEY; fi
  [[ -n "$site" ]] || { fail 'Turnstile site key 不能为空。'; return 1; }
  hidden_read secret 'Turnstile secret key（隐藏，回车保留现有值）: '
  if [[ -z "$secret" && -n "${CLOUDLEDGER_TURNSTILE_SECRET_KEY:-}" ]]; then secret=$CLOUDLEDGER_TURNSTILE_SECRET_KEY; fi
  [[ -n "$secret" ]] || { fail 'Turnstile secret key 不能为空。'; return 1; }
  set_env_value CLOUDLEDGER_TURNSTILE_SITE_KEY "$site" || { unset secret; return 1; }
  set_env_value CLOUDLEDGER_TURNSTILE_SECRET_KEY "$secret" || { unset secret; return 1; }
  unset secret
  ok 'Turnstile 配置已保存。'
}

validate_certificate_pair() {
  local cert=$1 key=$2 domain=$3 temp cert_hash key_hash
  openssl x509 -in "$cert" -noout >/dev/null || { fail 'Origin CA 证书无法解析。'; return 1; }
  openssl pkey -in "$key" -noout >/dev/null || { fail 'Origin CA 私钥无法解析或带有未提供的口令。'; return 1; }
  openssl x509 -checkend 2592000 -noout -in "$cert" || { fail 'Origin CA 证书有效期不足 30 天。'; return 1; }
  openssl x509 -checkhost "$domain" -noout -in "$cert" >/dev/null || { fail "证书 SAN 不覆盖 $domain。"; return 1; }
  temp=$(mktemp -d "${TMPDIR:-/tmp}/cloudledger-cert.XXXXXX") || return 1
  register_sensitive_path "$temp"
  if ! openssl x509 -in "$cert" -pubkey -noout | openssl pkey -pubin -outform DER >"$temp/cert.der" \
    || ! openssl pkey -in "$key" -pubout -outform DER >"$temp/key.der"; then
    remove_sensitive_path "$temp"
    return 1
  fi
  cert_hash=$(sha256sum "$temp/cert.der" | awk '{print $1}') || { remove_sensitive_path "$temp"; return 1; }
  key_hash=$(sha256sum "$temp/key.der" | awk '{print $1}') || { remove_sensitive_path "$temp"; return 1; }
  if [[ "$cert_hash" != "$key_hash" ]]; then
    remove_sensitive_path "$temp"
    fail 'Origin CA 证书与私钥不匹配。'
    return 1
  fi
  remove_sensitive_path "$temp" || return 1
}

install_cert_pair_locked() {
  local cert=$1 key=$2 cert_target="$CERT_DIR/origin-cert.pem" key_target="$CERT_DIR/origin-key.pem"
  local rollback had_cert=0 had_key=0 failed=0
  mkdir -p "$CERT_DIR" || return 1
  rollback=$(mktemp -d "$CERT_DIR/.cert-rollback.XXXXXX") || return 1
  register_sensitive_path "$rollback"
  if [[ -f "$cert_target" && ! -L "$cert_target" ]]; then
    cp -- "$cert_target" "$rollback/origin-cert.pem" || { remove_sensitive_path "$rollback"; return 1; }
    had_cert=1
  fi
  if [[ -f "$key_target" && ! -L "$key_target" ]]; then
    cp -- "$key_target" "$rollback/origin-key.pem" || { remove_sensitive_path "$rollback"; return 1; }
    had_key=1
  fi
  atomic_copy "$cert" "$cert_target" 0644 || failed=1
  if (( failed == 0 )); then atomic_copy "$key" "$key_target" 0600 || failed=1; fi
  if (( failed == 0 )); then validate_certificate_pair "$cert_target" "$key_target" "$CLOUDLEDGER_API_DOMAIN" || failed=1; fi
  if (( failed == 0 )); then set_env_value CLOUDLEDGER_CADDY_ORIGIN_CERT_PATH "$cert_target" || failed=1; fi
  if (( failed == 0 )); then set_env_value CLOUDLEDGER_CADDY_ORIGIN_KEY_PATH "$key_target" || failed=1; fi
  if (( failed != 0 )); then
    local rollback_failed=0
    if (( had_cert == 1 )); then
      cp -- "$rollback/origin-cert.pem" "$cert_target" || rollback_failed=1
    else
      rm -f -- "$cert_target" || rollback_failed=1
    fi
    if (( had_key == 1 )); then
      cp -- "$rollback/origin-key.pem" "$key_target" || rollback_failed=1
    else
      rm -f -- "$key_target" || rollback_failed=1
    fi
    if (( rollback_failed != 0 )); then
      unregister_sensitive_path "$rollback"
      chmod 700 "$rollback" 2>/dev/null || true
      fail "严重错误: Origin CA 文件回滚失败，旧文件现场保留于 $rollback。"
      return 2
    fi
    remove_sensitive_path "$rollback"
    fail 'Origin CA 证书事务失败，已恢复操作前文件。'
    return 1
  fi
  remove_sensitive_path "$rollback" || return 1
  ok "证书已在运维锁内成对校验并导入 $CERT_DIR。"
}

install_cert() {
  require_root
  local cert key domain staged
  load_env || return 1
  domain=${CLOUDLEDGER_API_DOMAIN:-}
  [[ -n "$domain" ]] || { fail '请先配置 API 域名。'; return 1; }
  read -r -p 'Origin CA 证书路径: ' cert
  read -r -p 'Origin CA 私钥路径: ' key
  [[ -f "$cert" && ! -L "$cert" && -f "$key" && ! -L "$key" ]] \
    || { fail '证书或私钥不存在或不是普通文件。'; return 1; }
  staged=$(mktemp -d "${TMPDIR:-/tmp}/cloudledger-cert-import.XXXXXX") || return 1
  register_sensitive_path "$staged"
  install -m 0644 "$cert" "$staged/origin-cert.pem" \
    || { remove_sensitive_path "$staged"; return 1; }
  install -m 0600 "$key" "$staged/origin-key.pem" \
    || { remove_sensitive_path "$staged"; return 1; }
  validate_certificate_pair "$staged/origin-cert.pem" "$staged/origin-key.pem" "$domain" \
    || { remove_sensitive_path "$staged"; return 1; }
  if ! with_lock install_cert_pair_locked "$staged/origin-cert.pem" "$staged/origin-key.pem"; then
    remove_sensitive_path "$staged"
    return 1
  fi
  remove_sensitive_path "$staged" || return 1
}

require_env_value() {
  local name=$1
  [[ -n "${!name:-}" ]] || { fail "缺少配置: $name"; return 1; }
}

validate_server_config_values() {
  local database_name=$1 api_domain=$2 runtime_password=$3 admin_path=$4 admin_token=$5
  local turnstile_site=$6 turnstile_secret=$7 audit_key_id=$8 audit_hmac=$9 identifier_hmac=${10}
  [[ "$database_name" == cloudledger || "$database_name" =~ ^cloudledger_restore_test_[0-9]+_[0-9]+$ ]] \
    || { fail '后端数据库名称不符合受保护命名约束。'; return 1; }
  valid_domain "$api_domain" || { fail '后端配置中的 API 域名无效。'; return 1; }
  [[ "$runtime_password" =~ ^[A-Za-z0-9_-]{16,128}$ ]] \
    || { fail '后端 runtime 数据库密码格式无效。'; return 1; }
  [[ "$admin_path" =~ ^[A-Za-z0-9_-]{16,128}$ ]] \
    || { fail '后端管理路径格式无效。'; return 1; }
  [[ "$admin_token" =~ ^[A-Za-z0-9_-]{32,256}$ ]] \
    || { fail '后端管理 token 格式无效。'; return 1; }
  [[ "$turnstile_site" =~ ^[A-Za-z0-9_-]{3,256}$ && "$turnstile_secret" =~ ^[A-Za-z0-9_-]{3,256}$ ]] \
    || { fail '后端 Turnstile 配置格式无效。'; return 1; }
  [[ "$audit_key_id" =~ ^[A-Za-z0-9_-]{8,128}$ ]] \
    || { fail '后端审计 key id 格式无效。'; return 1; }
  [[ "$audit_hmac" =~ ^[A-Za-z0-9_-]{32,256}$ && "$identifier_hmac" =~ ^[A-Za-z0-9_-]{32,256}$ ]] \
    || { fail '后端审计 HMAC 密钥格式无效。'; return 1; }
}

write_server_config_file() {
  local target=$1 database_name=$2 api_domain=$3 runtime_password=$4 admin_path=$5 admin_token=$6
  local turnstile_site=$7 turnstile_secret=$8 audit_key_id=$9
  local audit_hmac=${10} identifier_hmac=${11} runtime_url client_version min_supported_client_version download_url
  local configured_client_version=${12:-${CLOUDLEDGER_CLIENT_VERSION:-}}
  local configured_min_supported_version=${13:-${CLOUDLEDGER_MIN_SUPPORTED_CLIENT_VERSION:-}}
  local configured_download_url=${14:-${CLOUDLEDGER_CLIENT_DOWNLOAD_URL:-}}
  client_version=${configured_client_version:-$(client_version_for_tag "${CLOUDLEDGER_RELEASE_TAG:-}")}
  client_version=${client_version:-0.0.0}
  min_supported_client_version=${configured_min_supported_version:-$client_version}
  min_supported_client_version=$(client_version_for_tag "$min_supported_client_version")
  download_url=${configured_download_url:-https://github.com/dahai9/CloudLedger/releases/latest}
  valid_client_version "$client_version" || { fail '客户端版本必须是 SemVer。'; return 1; }
  valid_client_version "$min_supported_client_version" || { fail '最低支持客户端版本必须是 SemVer。'; return 1; }
  valid_download_url "$download_url" || { fail '官方下载地址必须是 HTTPS URL，且不能包含空格或引号。'; return 1; }
  validate_server_config_values "$database_name" "$api_domain" "$runtime_password" "$admin_path" "$admin_token" \
    "$turnstile_site" "$turnstile_secret" "$audit_key_id" "$audit_hmac" "$identifier_hmac" || return 1
  runtime_url="postgres://cloudledger_runtime:${runtime_password}@127.0.0.1:5432/${database_name}"
  cat >"$target" <<EOF
[server]
mode = "reverse_proxy"
api_bind_addr = "127.0.0.1:8787"
admin_bind_addr = "127.0.0.1:8788"
public_api_url = "https://${api_domain}"
public_admin_url = "https://${api_domain}"
allow_insecure_lan = false
web_login_enabled = false
client_version = "${client_version}"
min_supported_client_version = "${min_supported_client_version}"
client_download_url = "${download_url}"
data_dir = "/var/lib/cloudledger"

[database]
url = "$runtime_url"
auto_migrate = false
max_connections = 10
connect_timeout_seconds = 10

[admin]
path = "${admin_path}"
token = "${admin_token}"

[security.login]
turnstile_after_failures = 3
max_failures_per_login = 5
max_failures_per_ip = 20
window_seconds = 900
lockout_seconds = 900

[security.turnstile]
site_key = "${turnstile_site}"
secret_key = "${turnstile_secret}"
verify_url = "https://challenges.cloudflare.com/turnstile/v0/siteverify"

[security.network]
trusted_proxy_cidrs = ["127.0.0.1/32", "::1/128"]
cors_allowed_origins = ["tauri://localhost", "https://tauri.localhost"]

[security.audit]
key_id = "${audit_key_id}"
hmac_key = "${audit_hmac}"
identifier_hmac_key = "${identifier_hmac}"
EOF
}

write_server_config_from_env_file() {
  local source=$1 target=$2 database_name=$3
  local api_domain runtime_password admin_path admin_token turnstile_site turnstile_secret
  local audit_key_id audit_hmac identifier_hmac client_version min_supported_client_version download_url
  ops_env_file_value "$source" CLOUDLEDGER_API_DOMAIN api_domain || return 1
  ops_env_file_value "$source" CLOUDLEDGER_RUNTIME_DB_PASSWORD runtime_password || return 1
  ops_env_file_value "$source" CLOUDLEDGER_ADMIN_PATH admin_path || return 1
  ops_env_file_value "$source" CLOUDLEDGER_ADMIN_TOKEN admin_token || return 1
  ops_env_file_value "$source" CLOUDLEDGER_TURNSTILE_SITE_KEY turnstile_site || return 1
  ops_env_file_value "$source" CLOUDLEDGER_TURNSTILE_SECRET_KEY turnstile_secret || return 1
  ops_env_file_value "$source" CLOUDLEDGER_AUDIT_KEY_ID audit_key_id || return 1
  ops_env_file_value "$source" CLOUDLEDGER_AUDIT_HMAC_KEY audit_hmac || return 1
  ops_env_file_value "$source" CLOUDLEDGER_AUDIT_IDENTIFIER_HMAC_KEY identifier_hmac || return 1
  ops_env_file_value "$source" CLOUDLEDGER_CLIENT_VERSION client_version || return 1
  ops_env_file_value "$source" CLOUDLEDGER_MIN_SUPPORTED_CLIENT_VERSION min_supported_client_version || return 1
  ops_env_file_value "$source" CLOUDLEDGER_CLIENT_DOWNLOAD_URL download_url || return 1
  write_server_config_file "$target" "$database_name" "$api_domain" "$runtime_password" "$admin_path" \
    "$admin_token" "$turnstile_site" "$turnstile_secret" "$audit_key_id" "$audit_hmac" "$identifier_hmac" \
    "$client_version" "$min_supported_client_version" "$download_url"
}

render_server_config() {
  require_root
  load_env || return 1
  local name temp config_dir
  for name in CLOUDLEDGER_API_DOMAIN CLOUDLEDGER_RUNTIME_DB_PASSWORD CLOUDLEDGER_ADMIN_PATH \
    CLOUDLEDGER_ADMIN_TOKEN CLOUDLEDGER_TURNSTILE_SITE_KEY CLOUDLEDGER_TURNSTILE_SECRET_KEY \
    CLOUDLEDGER_AUDIT_KEY_ID CLOUDLEDGER_AUDIT_HMAC_KEY CLOUDLEDGER_AUDIT_IDENTIFIER_HMAC_KEY \
    CLOUDLEDGER_CLIENT_VERSION CLOUDLEDGER_MIN_SUPPORTED_CLIENT_VERSION CLOUDLEDGER_CLIENT_DOWNLOAD_URL; do
    require_env_value "$name" || return 1
  done
  config_dir=$(dirname "$SERVER_CONFIG") || return 1
  mkdir -p "$config_dir" || return 1
  chmod 700 "$config_dir" || { fail "无法将后端配置目录权限设置为 0700: $config_dir"; return 1; }
  temp=$(mktemp "$config_dir/.server.toml.XXXXXX") || return 1
  register_sensitive_path "$temp"
  chmod 600 "$temp" || { remove_sensitive_path "$temp"; return 1; }
  write_server_config_file "$temp" cloudledger "$CLOUDLEDGER_API_DOMAIN" "$CLOUDLEDGER_RUNTIME_DB_PASSWORD" \
    "$CLOUDLEDGER_ADMIN_PATH" "$CLOUDLEDGER_ADMIN_TOKEN" "$CLOUDLEDGER_TURNSTILE_SITE_KEY" \
    "$CLOUDLEDGER_TURNSTILE_SECRET_KEY" "$CLOUDLEDGER_AUDIT_KEY_ID" "$CLOUDLEDGER_AUDIT_HMAC_KEY" \
    "$CLOUDLEDGER_AUDIT_IDENTIFIER_HMAC_KEY" "$CLOUDLEDGER_CLIENT_VERSION" \
    "$CLOUDLEDGER_MIN_SUPPORTED_CLIENT_VERSION" "$CLOUDLEDGER_CLIENT_DOWNLOAD_URL" \
    || { remove_sensitive_path "$temp"; return 1; }
  if [[ ${EUID:-$(id -u)} -eq 0 ]]; then
    chown 10001:10001 "$temp" || { remove_sensitive_path "$temp"; return 1; }
  fi
  mv -f -- "$temp" "$SERVER_CONFIG" || { remove_sensitive_path "$temp"; return 1; }
  unregister_sensitive_path "$temp"
  chmod 600 "$SERVER_CONFIG" || return 1
  ok "已原子生成完整后端配置: $SERVER_CONFIG"
}

normalize_ops_env() {
  load_env || return 1
  local name
  for name in CLOUDLEDGER_GHCR_OWNER CLOUDLEDGER_SERVER_IMAGE CLOUDLEDGER_POSTGRES_IMAGE CLOUDLEDGER_CADDY_IMAGE \
    CLOUDLEDGER_ANCHOR_IMAGE CLOUDLEDGER_RELEASE_TAG CLOUDLEDGER_CLIENT_VERSION \
    CLOUDLEDGER_MIN_SUPPORTED_CLIENT_VERSION CLOUDLEDGER_CLIENT_DOWNLOAD_URL \
    CLOUDLEDGER_API_DOMAIN CLOUDLEDGER_BOOTSTRAP_DB_PASSWORD CLOUDLEDGER_MIGRATION_DB_PASSWORD \
    CLOUDLEDGER_RUNTIME_DB_PASSWORD CLOUDLEDGER_BOOTSTRAP_DATABASE_URL CLOUDLEDGER_MIGRATION_DATABASE_URL \
    CLOUDLEDGER_CADDY_ORIGIN_CERT_PATH CLOUDLEDGER_CADDY_ORIGIN_KEY_PATH; do
    require_env_value "$name" || return 1
  done
  set_env_value CLOUDLEDGER_HTTP_PUBLISH '127.0.0.1:18080:80' || return 1
  set_env_value CLOUDLEDGER_HTTPS_PUBLISH '443:443' || return 1
  set_env_value CLOUDLEDGER_BACKUP_RETENTION "${CLOUDLEDGER_BACKUP_RETENTION:-30}" || return 1
  set_env_value CLOUDLEDGER_MAX_BACKUP_BYTES "${CLOUDLEDGER_MAX_BACKUP_BYTES:-$DEFAULT_MAX_BACKUP_BYTES}" || return 1
  set_env_value CLOUDLEDGER_REMOTE_BACKUP_MAX_AGE_HOURS \
    "${CLOUDLEDGER_REMOTE_BACKUP_MAX_AGE_HOURS:-$DEFAULT_REMOTE_BACKUP_MAX_AGE_HOURS}" || return 1
  set_env_value CLOUDLEDGER_DISK_WARN "${CLOUDLEDGER_DISK_WARN:-80}" || return 1
  set_env_value CLOUDLEDGER_DISK_CRITICAL "${CLOUDLEDGER_DISK_CRITICAL:-90}" || return 1
  set_env_value CLOUDLEDGER_MEMORY_WARN "${CLOUDLEDGER_MEMORY_WARN:-85}" || return 1
  set_env_value CLOUDLEDGER_CLOUDFLARE_IPV4_URL "$CLOUDFLARE_IPV4_URL" || return 1
  set_env_value CLOUDLEDGER_CLOUDFLARE_IPV6_URL "$CLOUDFLARE_IPV6_URL" || return 1
  chmod 600 "$OPS_ENV" || return 1
}

validate_deployment_config() {
  load_env || return 1
  local allow_missing_client_version=${1:-0}
  local name owner image_owner temp
  for name in CLOUDLEDGER_HTTP_HOST_PORT CLOUDLEDGER_HTTPS_HOST_PORT CLOUDLEDGER_ADMIN_TUNNEL_PORT; do
    [[ -z "${!name:-}" ]] || { fail '当前配置仍包含旧版端口键，请通过升级向导执行受控接管。'; return 1; }
  done
  for name in CLOUDLEDGER_GHCR_OWNER CLOUDLEDGER_SERVER_IMAGE CLOUDLEDGER_POSTGRES_IMAGE CLOUDLEDGER_CADDY_IMAGE \
    CLOUDLEDGER_ANCHOR_IMAGE CLOUDLEDGER_RELEASE_TAG CLOUDLEDGER_API_DOMAIN \
    CLOUDLEDGER_CLIENT_VERSION CLOUDLEDGER_MIN_SUPPORTED_CLIENT_VERSION CLOUDLEDGER_CLIENT_DOWNLOAD_URL \
    CLOUDLEDGER_BOOTSTRAP_DB_PASSWORD CLOUDLEDGER_MIGRATION_DB_PASSWORD CLOUDLEDGER_RUNTIME_DB_PASSWORD \
    CLOUDLEDGER_BOOTSTRAP_DATABASE_URL CLOUDLEDGER_MIGRATION_DATABASE_URL \
    CLOUDLEDGER_ADMIN_PATH CLOUDLEDGER_ADMIN_TOKEN CLOUDLEDGER_TURNSTILE_SITE_KEY CLOUDLEDGER_TURNSTILE_SECRET_KEY \
    CLOUDLEDGER_AUDIT_KEY_ID CLOUDLEDGER_AUDIT_HMAC_KEY CLOUDLEDGER_AUDIT_IDENTIFIER_HMAC_KEY \
    CLOUDLEDGER_CADDY_ORIGIN_CERT_PATH CLOUDLEDGER_CADDY_ORIGIN_KEY_PATH; do
    if (( allow_missing_client_version == 1 )) && [[ "$name" == CLOUDLEDGER_CLIENT_VERSION \
      || "$name" == CLOUDLEDGER_MIN_SUPPORTED_CLIENT_VERSION \
      || "$name" == CLOUDLEDGER_CLIENT_DOWNLOAD_URL ]]; then
      continue
    fi
    require_env_value "$name" || return 1
  done
  [[ "$CLOUDLEDGER_GHCR_OWNER" =~ ^[a-z0-9_.-]+$ ]] \
    || { fail '可信 GHCR owner 格式无效。'; return 1; }
  valid_tag "$CLOUDLEDGER_RELEASE_TAG" || { fail '发布版本必须是明确 tag。'; return 1; }
  valid_ghcr_image "$CLOUDLEDGER_SERVER_IMAGE" cloudledger-server "$CLOUDLEDGER_RELEASE_TAG" || { fail '后端镜像必须是匹配 release tag 的 GHCR cloudledger-server。'; return 1; }
  valid_ghcr_image "$CLOUDLEDGER_POSTGRES_IMAGE" cloudledger-postgres "$CLOUDLEDGER_RELEASE_TAG" || { fail 'PostgreSQL 镜像必须是匹配 release tag 的 GHCR cloudledger-postgres。'; return 1; }
  valid_ghcr_image "$CLOUDLEDGER_CADDY_IMAGE" cloudledger-caddy "$CLOUDLEDGER_RELEASE_TAG" || { fail 'Caddy 镜像必须是匹配 release tag 的 GHCR cloudledger-caddy。'; return 1; }
  valid_ghcr_image "$CLOUDLEDGER_ANCHOR_IMAGE" cloudledger-network-anchor "$CLOUDLEDGER_RELEASE_TAG" || { fail 'network-anchor 镜像必须是匹配 release tag 的 GHCR cloudledger-network-anchor。'; return 1; }
  owner=${CLOUDLEDGER_SERVER_IMAGE#ghcr.io/}; owner=${owner%%/*}
  [[ "$owner" == "$CLOUDLEDGER_GHCR_OWNER" ]] \
    || { fail '镜像 owner 与当前可信 GHCR owner 不一致。'; return 1; }
  for name in CLOUDLEDGER_POSTGRES_IMAGE CLOUDLEDGER_CADDY_IMAGE CLOUDLEDGER_ANCHOR_IMAGE; do
    image_owner=${!name}; image_owner=${image_owner#ghcr.io/}; image_owner=${image_owner%%/*}
    [[ "$image_owner" == "$owner" ]] || { fail '四个生产镜像必须来自同一个 GHCR 所有者。'; return 1; }
  done
  valid_domain "$CLOUDLEDGER_API_DOMAIN" || { fail 'API 域名格式无效。'; return 1; }
  [[ "${CLOUDLEDGER_HTTP_PUBLISH:-}" == '127.0.0.1:18080:80' ]] || { fail 'HTTP 必须固定监听 127.0.0.1:18080。'; return 1; }
  [[ "${CLOUDLEDGER_HTTPS_PUBLISH:-}" == '443:443' ]] || { fail 'HTTPS 必须发布主机 443。'; return 1; }
  [[ "$CLOUDLEDGER_CADDY_ORIGIN_CERT_PATH" == "$CERT_DIR/origin-cert.pem" \
    && "$CLOUDLEDGER_CADDY_ORIGIN_KEY_PATH" == "$CERT_DIR/origin-key.pem" ]] \
    || { fail 'Origin CA 文件必须位于固定的受保护目录。'; return 1; }
  [[ -s "$SERVER_CONFIG" && -s "$CLOUDLEDGER_CADDY_ORIGIN_CERT_PATH" && -s "$CLOUDLEDGER_CADDY_ORIGIN_KEY_PATH" ]] || { fail '后端配置或 Origin CA 文件缺失。'; return 1; }
  for name in CLOUDLEDGER_BOOTSTRAP_DB_PASSWORD CLOUDLEDGER_MIGRATION_DB_PASSWORD CLOUDLEDGER_RUNTIME_DB_PASSWORD; do
    [[ "${!name}" =~ ^[A-Za-z0-9_-]{16,128}$ ]] \
      || { fail "数据库密码格式无效: $name"; return 1; }
  done
  [[ "$CLOUDLEDGER_BOOTSTRAP_DATABASE_URL" == "postgres://cloudledger_bootstrap:${CLOUDLEDGER_BOOTSTRAP_DB_PASSWORD}@127.0.0.1:5432/cloudledger" \
    && "$CLOUDLEDGER_MIGRATION_DATABASE_URL" == "postgres://cloudledger_migration:${CLOUDLEDGER_MIGRATION_DB_PASSWORD}@127.0.0.1:5432/cloudledger" ]] \
    || { fail '数据库运维 URL 与受保护角色或密码不一致。'; return 1; }
  ensure_dirs || return 1
  temp=$(mktemp "$STATE_DIR/.server-expected.XXXXXX") || return 1
  register_sensitive_path "$temp"
  chmod 600 "$temp" || { remove_sensitive_path "$temp"; return 1; }
  if ! write_server_config_file "$temp" cloudledger "$CLOUDLEDGER_API_DOMAIN" "$CLOUDLEDGER_RUNTIME_DB_PASSWORD" \
    "$CLOUDLEDGER_ADMIN_PATH" "$CLOUDLEDGER_ADMIN_TOKEN" "$CLOUDLEDGER_TURNSTILE_SITE_KEY" \
    "$CLOUDLEDGER_TURNSTILE_SECRET_KEY" "$CLOUDLEDGER_AUDIT_KEY_ID" "$CLOUDLEDGER_AUDIT_HMAC_KEY" \
    "$CLOUDLEDGER_AUDIT_IDENTIFIER_HMAC_KEY" "${CLOUDLEDGER_CLIENT_VERSION:-}" \
    "${CLOUDLEDGER_MIN_SUPPORTED_CLIENT_VERSION:-}" "${CLOUDLEDGER_CLIENT_DOWNLOAD_URL:-}" \
    || { (( allow_missing_client_version == 0 )) && ! cmp -s "$SERVER_CONFIG" "$temp"; }; then
    remove_sensitive_path "$temp"
    fail '当前 server.toml 不符合工具生成的安全配置模板。'
    return 1
  fi
  if [[ -n "${CLOUDLEDGER_CLIENT_VERSION:-}" ]]; then
    valid_client_version "$CLOUDLEDGER_CLIENT_VERSION" || { fail '客户端版本必须是 SemVer。'; return 1; }
  elif (( allow_missing_client_version == 0 )); then
    fail '客户端版本缺失。'
    return 1
  fi
  if [[ -n "${CLOUDLEDGER_MIN_SUPPORTED_CLIENT_VERSION:-}" ]]; then
    valid_client_version "$CLOUDLEDGER_MIN_SUPPORTED_CLIENT_VERSION" || { fail '最低支持客户端版本必须是 SemVer。'; return 1; }
  elif (( allow_missing_client_version == 0 )); then
    fail '最低支持客户端版本缺失。'
    return 1
  fi
  if [[ -n "${CLOUDLEDGER_CLIENT_DOWNLOAD_URL:-}" ]]; then
    valid_download_url "$CLOUDLEDGER_CLIENT_DOWNLOAD_URL" || { fail '官方下载地址必须是 HTTPS URL，且不能包含空格或引号。'; return 1; }
  elif (( allow_missing_client_version == 0 )); then
    fail '官方下载地址缺失。'
    return 1
  fi
  remove_sensitive_path "$temp" || return 1
  validate_certificate_pair "$CLOUDLEDGER_CADDY_ORIGIN_CERT_PATH" "$CLOUDLEDGER_CADDY_ORIGIN_KEY_PATH" \
    "$CLOUDLEDGER_API_DOMAIN" || return 1
  compose --profile migration config --quiet || return 1
  ok '部署配置和 Compose 模型验证通过。'
}

wait_for_postgres() {
  for _ in $(seq 1 60); do
    if compose exec -T postgres pg_isready --host 127.0.0.1 --username cloudledger_bootstrap --dbname cloudledger >/dev/null 2>&1; then
      ok 'PostgreSQL 健康检查通过。'
      return 0
    fi
    sleep 2
  done
  fail 'PostgreSQL 在 120 秒内未就绪。'
  compose logs --tail=100 postgres || true
  return 1
}

postgres_psql() {
  compose exec -T postgres \
    psql --username cloudledger_bootstrap --dbname "${1:-cloudledger}" \
    --set ON_ERROR_STOP=1 "${@:2}"
}

verify_database_roles() {
  local output
  output=$(postgres_psql cloudledger -Atqc \
    "SELECT rolname || ':' || rolsuper::text || ':' || rolcreatedb::text || ':' || rolcreaterole::text || ':' || rolreplication::text || ':' || rolbypassrls::text FROM pg_roles WHERE rolname IN ('cloudledger_bootstrap','cloudledger_migration','cloudledger_runtime') ORDER BY rolname") || return 1
  grep -q '^cloudledger_bootstrap:true:' <<<"$output" || { fail 'bootstrap 账号缺少预期管理权限。'; return 1; }
  grep -q '^cloudledger_migration:false:false:false:false:false$' <<<"$output" || { fail 'migration 账号权限不符合非超级用户约束。'; return 1; }
  grep -q '^cloudledger_runtime:false:false:false:false:false$' <<<"$output" || { fail 'runtime 账号权限不符合最小权限约束。'; return 1; }
  ok 'PostgreSQL 三类账号权限验证通过。'
}

verify_network_bridge() {
  local bridge
  bridge=$(docker network inspect --format '{{index .Options "com.docker.network.bridge.name"}}' cloudledger-origin) || return 1
  [[ "$bridge" == cld-origin0 ]] || { fail "Docker 网络桥接名为 $bridge，不是 cld-origin0。"; return 1; }
  if [[ ${CLOUDLEDGER_ALLOW_NONROOT:-0} != 1 ]] && command_exists ip; then
    ip link show cld-origin0 >/dev/null 2>&1 || { fail '宿主机不存在 cld-origin0 网桥，防火墙无法可靠匹配。'; return 1; }
  fi
  ok 'CloudLedger Docker 网桥 cld-origin0 已验证。'
}

run_migration() { compose --profile migration run --rm migration; }

verify_audit() {
  compose --profile migration run --rm --no-deps migration audit verify --config /etc/cloudledger/server.toml
}

migration_status() {
  postgres_psql cloudledger -P pager=off -c \
    'SELECT version, description, installed_on, success FROM _sqlx_migrations ORDER BY version;'
}

verify_migrations_exact() {
  local result versions count succeeded required
  result=$(postgres_psql cloudledger -Atqc \
    "SELECT coalesce(string_agg(version::text, ',' ORDER BY version), '') || '|' || count(*)::text || '|' || coalesce(bool_and(success), false)::text FROM _sqlx_migrations") || return 1
  IFS='|' read -r versions count succeeded <<<"$result"
  [[ "$versions" =~ ^[0-9]+(,[0-9]+)*$ && "$count" =~ ^[0-9]+$ && "$succeeded" == true ]] \
    || { fail "数据库迁移状态无效: $result"; return 1; }
  (( count >= 5 )) || { fail "数据库迁移数量不足: $count"; return 1; }
  for required in 1 2 3 4 5; do
    [[ ",$versions," == *",$required,"* ]] \
      || { fail "数据库缺少基线 migration $required: $versions"; return 1; }
  done
  ok "全部 $count 个 SQLx migration 成功，且包含基线 1..5。"
}

harden_runtime_metadata_permissions() {
  postgres_psql cloudledger <<'SQL'
REVOKE INSERT, UPDATE, DELETE, TRUNCATE ON _sqlx_migrations FROM cloudledger_runtime;
GRANT SELECT ON _sqlx_migrations TO cloudledger_runtime;
SQL
  local privileges
  privileges=$(postgres_psql cloudledger -Atqc \
    "SELECT has_table_privilege('cloudledger_runtime', '_sqlx_migrations', 'SELECT')::text || ':' || has_table_privilege('cloudledger_runtime', '_sqlx_migrations', 'INSERT')::text || ':' || has_table_privilege('cloudledger_runtime', '_sqlx_migrations', 'UPDATE')::text || ':' || has_table_privilege('cloudledger_runtime', '_sqlx_migrations', 'DELETE')::text || ':' || has_table_privilege('cloudledger_runtime', '_sqlx_migrations', 'TRUNCATE')::text") || return 1
  [[ "$privileges" == 'true:false:false:false:false' ]] || { fail "runtime 对迁移元数据的权限异常: $privileges"; return 1; }
  ok 'runtime 仅可读取 SQLx 迁移元数据。'
}

check_local_backend() {
  local path=$1
  for _ in $(seq 1 30); do
    if compose exec -T network-anchor wget -qO- "http://127.0.0.1:8787$path" >/dev/null 2>&1; then
      ok "本地后端 $path: 正常"
      return 0
    fi
    sleep 2
  done
  fail "本地后端 $path: 异常"
  return 1
}

verify_turnstile() {
  local api response client_version
  api=$(api_base_url)
  load_env || return 1
  client_version=${CLOUDLEDGER_CLIENT_VERSION:-}
  if [[ -n "$client_version" ]]; then
    valid_client_version "$client_version" \
      || { fail '当前客户端版本配置无效，无法读取 Turnstile 状态。'; return 1; }
    response=$(curl -fsS --max-time 10 \
      -H "X-CloudLedger-Client-Version: $client_version" "$api/auth/security") \
      || { fail '无法读取 Turnstile 状态。'; return 1; }
  else
    # Older deployments predate the client-version gate. Keep their public
    # verification path working until the first controlled upgrade backfills
    # the version fields.
    response=$(curl -fsS --max-time 10 "$api/auth/security") \
      || { fail '无法读取 Turnstile 状态。'; return 1; }
  fi
  jq -e '.turnstileEnabled == true and (.turnstileSiteKey | length > 0)' <<<"$response" >/dev/null \
    || { fail 'API 未启用 Turnstile 或 site key 为空。'; return 1; }
  verify_turnstile_credentials || return 1
  ok 'Turnstile 服务端状态与 secret 凭据验证通过。'
}

verify_turnstile_credentials() {
  local response
  load_env || return 1
  [[ "${CLOUDLEDGER_TURNSTILE_SECRET_KEY:-}" =~ ^[A-Za-z0-9_-]+$ ]] \
    || { fail 'Turnstile secret key 格式无效。'; return 1; }
  response=$(printf 'secret=%s&response=cloudledger-configuration-probe' "$CLOUDLEDGER_TURNSTILE_SECRET_KEY" \
    | curl -fsS --proto '=https' --tlsv1.2 --max-time 15 -H 'Content-Type: application/x-www-form-urlencoded' \
        --data-binary @- 'https://challenges.cloudflare.com/turnstile/v0/siteverify') \
    || { fail '无法向 Cloudflare 验证 Turnstile secret。'; return 1; }
  jq -e '. as $result | ($result["error-codes"] // []) as $errors |
    ($result.success == false) and
    (($errors | index("invalid-input-response")) != null) and
    (($errors | index("invalid-input-secret")) == null) and
    (($errors | index("missing-input-secret")) == null)' <<<"$response" >/dev/null \
    || { fail 'Turnstile secret key 无效或 Cloudflare 返回了非预期验证结果。'; return 1; }
  ok 'Turnstile secret key 验证通过。'
}

verify_deploy() {
  local api failed=0
  api=$(api_base_url)
  service_state
  health_url 'API /health' "$api/health" || failed=1
  health_url 'API /ready' "$api/ready" || failed=1
  verify_turnstile || failed=1
  verify_audit || failed=1
  [[ "$failed" -eq 0 ]]
}

validate_cloudflare_ranges() {
  local file=$1 family=$2 line count=0
  while IFS= read -r line || [[ -n "$line" ]]; do
    [[ -z "$line" ]] && continue
    if [[ "$family" == 4 ]]; then
      [[ "$line" =~ ^[0-9]{1,3}(\.[0-9]{1,3}){3}/[0-9]{1,2}$ ]] || return 1
    else
      [[ "$line" =~ ^[0-9A-Fa-f:]+/[0-9]{1,3}$ ]] || return 1
    fi
    count=$((count + 1))
  done <"$file"
  (( count > 0 ))
}

fetch_cloudflare_ranges() {
  local url=$1 target=$2 cache=$3 family=$4
  if curl -fsS --proto '=https' --tlsv1.2 --max-time 30 "$url" -o "$target" \
    && validate_cloudflare_ranges "$target" "$family"; then
    chmod 600 "$target"
  else
    rm -f -- "$target"
    if [[ -s "$cache" ]] && validate_cloudflare_ranges "$cache" "$family"; then
      warn "无法刷新 Cloudflare IPv${family} 列表，使用上次验证缓存。"
      cp -- "$cache" "$target"
    else
      fail "无法取得有效的 Cloudflare IPv${family} 官方列表。"
      return 1
    fi
  fi
}

render_firewall_rules() {
  local ipv4=$1 ipv6=$2 target=$3 has_table=$4 v4_elements v6_elements
  v4_elements=$(paste -sd, "$ipv4") || return 1
  v6_elements=$(paste -sd, "$ipv6") || return 1
  # nftables rejects `elements = { }`.  The initial fail-closed baseline
  # deliberately renders empty sets before the Cloudflare ranges are fetched,
  # so only emit an elements clause when the corresponding file has entries.
  if [[ -n "$v4_elements" ]]; then
    v4_elements="    elements = { $v4_elements }"
  else
    v4_elements=''
  fi
  if [[ -n "$v6_elements" ]]; then
    v6_elements="    elements = { $v6_elements }"
  else
    v6_elements=''
  fi
  : >"$target" || return 1
  if [[ "$has_table" == 1 ]]; then
    printf '%s\n' 'delete table inet cloudledger_origin' >>"$target" || return 1
  fi
  cat >>"$target" <<EOF || return 1
table inet cloudledger_origin {
  set cloudflare_ipv4 {
    type ipv4_addr
    flags interval
${v4_elements}
  }
  set cloudflare_ipv6 {
    type ipv6_addr
    flags interval
${v6_elements}
  }
  chain input {
    type filter hook input priority -10; policy accept;
    iifname "lo" tcp dport 443 accept
    ip saddr @cloudflare_ipv4 tcp dport 443 accept
    ip6 saddr @cloudflare_ipv6 tcp dport 443 accept
    tcp dport 443 reject with tcp reset
  }
  chain forward {
    type filter hook forward priority -10; policy accept;
    oifname "cld-origin0" ip saddr @cloudflare_ipv4 tcp dport 443 accept
    oifname "cld-origin0" ip6 saddr @cloudflare_ipv6 tcp dport 443 accept
    oifname "cld-origin0" tcp dport 443 reject with tcp reset
  }
}
EOF
  chmod 600 "$target" || return 1
}

internal_firewall_refresh() {
  require_root
  ensure_dirs || return 1
  load_env || return 1
  command_exists nft || { fail '未安装 nftables。'; return 1; }
  local ipv4="$FIREWALL_DIR/cloudflare-ipv4.txt" ipv6="$FIREWALL_DIR/cloudflare-ipv6.txt"
  local next4="$FIREWALL_DIR/cloudflare-ipv4.txt.new" next6="$FIREWALL_DIR/cloudflare-ipv6.txt.new"
  local candidate="$FIREWALL_DIR/cloudledger-origin.nft.new" active="$FIREWALL_DIR/cloudledger-origin.nft"
  local empty4="$FIREWALL_DIR/.empty-v4" empty6="$FIREWALL_DIR/.empty-v6" has_table=0
  : >"$empty4" || return 1
  : >"$empty6" || { rm -f -- "$empty4"; return 1; }
  rm -f -- "$next4" "$next6" \
    || { rm -f -- "$empty4" "$empty6"; return 1; }
  if ! nft list table inet cloudledger_origin >/dev/null 2>&1; then
    render_firewall_rules "$empty4" "$empty6" "$candidate" 0 \
      || { rm -f -- "$candidate" "$next4" "$next6" "$empty4" "$empty6"; return 1; }
    if ! nft --check --file "$candidate" || ! nft --file "$candidate"; then
      rm -f -- "$candidate" "$next4" "$next6" "$empty4" "$empty6"
      fail '无法建立 fail-closed 443 基线规则。'
      return 1
    fi
  fi
  fetch_cloudflare_ranges "$CLOUDFLARE_IPV4_URL" "$next4" "$ipv4" 4 \
    || { rm -f -- "$candidate" "$next4" "$next6" "$empty4" "$empty6"; return 1; }
  fetch_cloudflare_ranges "$CLOUDFLARE_IPV6_URL" "$next6" "$ipv6" 6 \
    || { rm -f -- "$candidate" "$next4" "$next6" "$empty4" "$empty6"; return 1; }
  if nft list table inet cloudledger_origin >/dev/null 2>&1; then has_table=1; fi
  render_firewall_rules "$next4" "$next6" "$candidate" "$has_table" \
    || { rm -f -- "$candidate" "$next4" "$next6" "$empty4" "$empty6"; return 1; }
  nft --check --file "$candidate" \
    || { rm -f -- "$candidate" "$next4" "$next6" "$empty4" "$empty6"; fail 'nftables 新规则语法验证失败。'; return 1; }
  if ! nft --file "$candidate"; then
    rm -f -- "$candidate" "$next4" "$next6" "$empty4" "$empty6"
    fail 'nftables 事务应用失败，内核中的上一版规则保持不变。'
    return 1
  fi
  mv -f -- "$candidate" "$active" || return 1
  mv -f -- "$next4" "$ipv4" || return 1
  mv -f -- "$next6" "$ipv6" || return 1
  rm -f -- "$empty4" "$empty6" || return 1
  ok 'Cloudflare-only 443 防火墙已覆盖主机 INPUT 与 Docker FORWARD；SSH、xray 和其他端口未修改。'
}

firewall_status() {
  local table set4 set6 chains input_body forward_body
  command_exists nft || { warn '未安装 nftables。'; return 1; }
  table=$(nft list table inet cloudledger_origin 2>/dev/null) \
    || { warn 'CloudLedger Cloudflare 防火墙尚未应用。'; return 1; }
  set4=$(nft list set inet cloudledger_origin cloudflare_ipv4 2>/dev/null) \
    || { warn 'Cloudflare IPv4 防火墙集合缺失。'; return 1; }
  set6=$(nft list set inet cloudledger_origin cloudflare_ipv6 2>/dev/null) \
    || { warn 'Cloudflare IPv6 防火墙集合缺失。'; return 1; }
  set4=$(tr '\n' ' ' <<<"$set4")
  set6=$(tr '\n' ' ' <<<"$set6")
  [[ "$set4" =~ elements[[:space:]]*=[[:space:]]*\{[^}]*[0-9][^}]*\} ]] \
    || { warn 'Cloudflare IPv4 防火墙集合为空。'; return 1; }
  [[ "$set6" =~ elements[[:space:]]*=[[:space:]]*\{[^}]*[0-9A-Fa-f:][^}]*\} ]] \
    || { warn 'Cloudflare IPv6 防火墙集合为空。'; return 1; }
  chains=$(awk '$1 == "chain" { print $2 }' <<<"$table")
  [[ "$chains" == $'input\nforward' ]] \
    || { warn 'CloudLedger 防火墙包含缺失或未授权的链。'; return 1; }
  input_body=$(awk '
    $1 == "chain" && $2 == "input" { active=1; next }
    active && /^[[:space:]]*}/ { exit }
    active {
      # `nft list` may render a numeric priority as `priority filter - 10`
      # even though the rule was loaded as `priority -10`.
      gsub(/priority filter - 10/, "priority -10")
      gsub(/[[:space:]]+/, " ")
      sub(/^ /, "")
      sub(/ $/, "")
      if (length) print
    }
  ' <<<"$table")
  forward_body=$(awk '
    $1 == "chain" && $2 == "forward" { active=1; next }
    active && /^[[:space:]]*}/ { exit }
    active {
      gsub(/priority filter - 10/, "priority -10")
      gsub(/[[:space:]]+/, " ")
      sub(/^ /, "")
      sub(/ $/, "")
      if (length) print
    }
  ' <<<"$table")
  [[ "$input_body" == $'type filter hook input priority -10; policy accept;\niifname "lo" tcp dport 443 accept\nip saddr @cloudflare_ipv4 tcp dport 443 accept\nip6 saddr @cloudflare_ipv6 tcp dport 443 accept\ntcp dport 443 reject with tcp reset' ]] \
    || { warn 'CloudLedger INPUT 链顺序、hook 或规则集合不符合 fail-closed 模型。'; return 1; }
  [[ "$forward_body" == $'type filter hook forward priority -10; policy accept;\noifname "cld-origin0" ip saddr @cloudflare_ipv4 tcp dport 443 accept\noifname "cld-origin0" ip6 saddr @cloudflare_ipv6 tcp dport 443 accept\noifname "cld-origin0" tcp dport 443 reject with tcp reset' ]] \
    || { warn 'CloudLedger FORWARD 链顺序、hook 或规则集合不符合 fail-closed 模型。'; return 1; }
  ok 'Cloudflare-only 443 防火墙规则完整。'
}

install_systemd_units() {
  require_root
  stage_assets || return 1
  local unit legacy_dropin="$SYSTEMD_INSTALL_DIR/docker.service.d/cloudledger-firewall.conf"
  mkdir -p "$SYSTEMD_INSTALL_DIR" || return 1
  for unit in "$DEPLOY_DIR"/systemd/cloudledger-ops-*; do
    [[ -f "$unit" ]] || continue
    atomic_copy "$unit" "$SYSTEMD_INSTALL_DIR/$(basename "$unit")" 0644 || return 1
  done
  if [[ -e "$legacy_dropin" || -L "$legacy_dropin" ]]; then
    rm -f -- "$legacy_dropin" || return 1
    rmdir --ignore-fail-on-non-empty "$(dirname "$legacy_dropin")" 2>/dev/null || true
    warn '已移除旧版 Docker 全局防火墙依赖，其他容器不再受 CloudLedger 单元失败影响。'
  fi
  systemctl daemon-reload || return 1
  ok 'systemd 运维单元已安装。'
}

enable_base_timers() {
  systemctl enable cloudledger-ops-firewall-refresh.service || return 1
  systemctl enable --now cloudledger-ops-firewall-refresh.timer cloudledger-ops-health.timer || return 1
}

enable_backup_timers() {
  systemctl enable --now cloudledger-ops-backup.timer cloudledger-ops-restore-test.timer || return 1
}

rclone_remote_name() { local remote=$1; printf '%s' "${remote%%:*}"; }

validate_rclone_crypt_remote() {
  local remote=$1 name line in_section=0 found=0 type='' type_seen=0
  [[ "$remote" =~ ^[A-Za-z0-9_.-]+:.+$ ]] || { fail '远程目录格式应为 cryptRemote:path。'; return 1; }
  name=$(rclone_remote_name "$remote")
  [[ -f "$RCLONE_CONFIG" && ! -L "$RCLONE_CONFIG" ]] \
    || { fail "未找到 rclone 配置文件: $RCLONE_CONFIG"; return 1; }
  # Read the local config directly instead of relying on `rclone config show`.
  # Older rclone versions may fail that command while still exposing the
  # plaintext `type` field (for example when an encrypted config cannot be
  # decrypted), which previously produced a false "not crypt" rejection.
  while IFS= read -r line || [[ -n "$line" ]]; do
    line=${line%$'\r'}
    if [[ "$line" == "[$name]" ]]; then
      in_section=1
      found=$((found + 1))
      continue
    fi
    if [[ "$line" == \[*\] ]]; then
      in_section=0
      continue
    fi
    if (( in_section == 1 )) && [[ "$line" =~ ^[[:space:]]*type[[:space:]]*=[[:space:]]*(.*)$ ]]; then
      type=${BASH_REMATCH[1]}
      type=${type#"${type%%[![:space:]]*}"}
      type=${type%"${type##*[![:space:]]}"}
      type_seen=$((type_seen + 1))
    fi
  done <"$RCLONE_CONFIG"
  if (( found != 1 )); then
    fail "rclone 配置中找不到唯一 remote '$name'（配置文件: $RCLONE_CONFIG）。"
    return 1
  fi
  if (( type_seen != 1 )) || [[ "$type" != crypt ]]; then
    fail "所选 remote '$name' 的 type 不是 crypt；禁止明文上传备份。"
    return 1
  fi
}

require_remote_backup_configuration() {
  load_env || return 1
  [[ -n "${CLOUDLEDGER_RCLONE_REMOTE:-}" ]] || { fail '升级前必须配置 rclone crypt 远程备份。'; return 1; }
  command_exists rclone || { fail '升级前必须安装 rclone。'; return 1; }
  validate_rclone_crypt_remote "$CLOUDLEDGER_RCLONE_REMOTE"
}

rclone_test_transfer() {
  load_env || return 1
  local remote=${CLOUDLEDGER_RCLONE_REMOTE:-} local_file downloaded remote_file
  [[ -n "$remote" ]] || { fail '未配置 rclone crypt 备份目录。'; return 1; }
  validate_rclone_crypt_remote "$remote" || return 1
  ensure_dirs || return 1
  local_file=$(mktemp "$STATE_DIR/.rclone-upload.XXXXXX") || return 1
  register_sensitive_path "$local_file"
  downloaded=$(mktemp "$STATE_DIR/.rclone-download.XXXXXX") \
    || { remove_sensitive_path "$local_file"; return 1; }
  register_sensitive_path "$downloaded"
  chmod 600 "$local_file" "$downloaded" \
    || { remove_sensitive_path "$local_file"; remove_sensitive_path "$downloaded"; return 1; }
  openssl rand -hex 32 >"$local_file" \
    || { remove_sensitive_path "$local_file"; remove_sensitive_path "$downloaded"; return 1; }
  remote_file="${remote%/}/.cloudledger-ops-test-$$"
  if ! rclone copyto "$local_file" "$remote_file" \
    || ! rclone copyto "$remote_file" "$downloaded" \
    || ! cmp -s "$local_file" "$downloaded"; then
    remove_sensitive_path "$local_file"
    remove_sensitive_path "$downloaded"
    rclone deletefile "$remote_file" >/dev/null 2>&1 || true
    fail 'rclone crypt 上传/下载测试失败。'
    return 1
  fi
  if ! rclone deletefile "$remote_file"; then
    remove_sensitive_path "$local_file"
    remove_sensitive_path "$downloaded"
    fail 'rclone 测试对象无法清理。'
    return 1
  fi
  if ! remove_sensitive_path "$local_file"; then
    remove_sensitive_path "$downloaded" || true
    return 1
  fi
  remove_sensitive_path "$downloaded" || return 1
  ok 'rclone crypt 上传、下载和内容比对通过。'
}

configure_rclone_wizard() {
  require_root
  local choice remote
  command_exists rclone || { fail '未安装 rclone。'; return 1; }
  warn 'rclone crypt 密码不会进入备份包；必须在服务器外单独保存恢复密码。'
  printf '%s\n' '1. 使用现有 rclone 配置' '2. 打开 rclone 数字配置向导' '0. 取消'
  read_choice choice '请选择: ' '^[012]$' || return 1
  case "$choice" in
    1) [[ -s "$RCLONE_CONFIG" ]] || { fail "未找到 $RCLONE_CONFIG"; return 1; } ;;
    2) mkdir -p "$(dirname "$RCLONE_CONFIG")" || return 1; rclone config --config "$RCLONE_CONFIG" || return 1 ;;
    0) return 1 ;;
  esac
  chmod 600 "$RCLONE_CONFIG" || return 1
  load_env || return 1
  read -r -p "crypt 远程备份目录 [${CLOUDLEDGER_RCLONE_REMOTE:-cloudledger:backups}]: " remote
  remote=${remote:-${CLOUDLEDGER_RCLONE_REMOTE:-cloudledger:backups}}
  validate_rclone_crypt_remote "$remote" || return 1
  rclone mkdir "$remote" || return 1
  set_env_value CLOUDLEDGER_RCLONE_REMOTE "$remote" || return 1
  rclone_test_transfer || return 1
}

preflight_https_port() {
  local anchor_id publishers id name listeners line
  anchor_id=$(compose ps -q network-anchor 2>/dev/null || true)
  publishers=$(docker ps --filter publish=443 --format '{{.ID}}\t{{.Names}}' 2>/dev/null || true)
  while IFS=$'\t' read -r id name; do
    [[ -n "$id" ]] || continue
    if [[ -z "$anchor_id" || ( "$anchor_id" != "$id" && "$anchor_id" != "$id"* && "$id" != "$anchor_id"* ) ]]; then
      fail "主机 443 已由其他 Docker 容器占用: ${name:-$id}。未修改防火墙或服务。"
      return 1
    fi
  done <<<"$publishers"
  if command_exists ss; then
    listeners=$(ss -H -ltnp 'sport = :443' 2>/dev/null || true)
    if [[ -n "$listeners" && -z "$publishers" ]]; then
      fail '主机 443 已被非 CloudLedger 进程占用。未修改防火墙或服务。'
      return 1
    fi
    while IFS= read -r line; do
      [[ -n "$line" && "$line" == *'users:'* && "$line" != *docker-proxy* && "$line" != *rootlesskit* ]] || continue
      fail "主机 443 存在非 Docker 监听进程: $line"
      return 1
    done <<<"$listeners"
  fi
  ok '主机 443 占用预检通过。'
}

deploy_locked() {
  validate_deployment_config || return 1
  log '1/12 预检主机 443 端口所有者...'
  preflight_https_port || return 1
  log '2/12 拉取明确版本镜像...'
  compose pull network-anchor postgres migration cloudledger caddy || return 1
  log '3/12 启动网络锚点和 PostgreSQL...'
  compose up -d network-anchor postgres || return 1
  verify_network_bridge || { compose stop caddy network-anchor postgres 2>/dev/null || true; return 1; }
  log '4/12 建立并刷新 Cloudflare-only 443 防火墙...'
  internal_firewall_refresh || { compose stop caddy network-anchor postgres 2>/dev/null || true; return 1; }
  log '5/12 等待 PostgreSQL 健康并验证账号...'
  wait_for_postgres && verify_database_roles || return 1
  log '6/12 使用专用 migration profile 执行数据库迁移...'
  run_migration || return 1
  log '7/12 验证迁移记录和审计链...'
  verify_migrations_exact && harden_runtime_metadata_permissions && verify_audit || return 1
  log '8/12 启动后端...'
  compose up -d --no-deps cloudledger || return 1
  log '9/12 检查后端 /health 和 /ready...'
  check_local_backend /health && check_local_backend /ready || return 1
  log '10/12 验证并启动 Caddy...'
  compose run --rm --no-deps caddy caddy validate --config /etc/caddy/Caddyfile || return 1
  compose up -d --no-deps caddy || return 1
  log '11/12 验证 Cloudflare HTTPS 和 Turnstile...'
  health_url 'Cloudflare /health' "$(api_base_url)/health" \
    && health_url 'Cloudflare /ready' "$(api_base_url)/ready" \
    && verify_turnstile || return 1
  log '12/12 复核 Cloudflare-only 防火墙状态...'
  firewall_status || return 1
  printf '%s %s success\n' "$(date -u +%FT%TZ)" "${CLOUDLEDGER_RELEASE_TAG:-unknown}" >"$STATE_DIR/deploy.log"
  ok '首次部署流水线完成。'
}

first_deploy() {
  require_root
  stage_assets || return 1
  [[ -f "$COMPOSE_FILE" ]] || { fail '缺少 Compose 文件。'; return 1; }
  with_lock deploy_locked
}

install_wizard() {
  require_root
  stage_assets || return 1
  if ! check_requirements; then
    warn '部署依赖不完整，将进入自动安装。'
    install_dependencies || return 1
    check_requirements || return 1
  fi
  configure_registry_access || return 1
  configure_images || return 1
  generate_passwords || return 1
  set_domains || return 1
  configure_turnstile || return 1
  install_cert || return 1
  render_server_config || return 1
  normalize_ops_env || return 1
  configure_rclone_wizard || return 1
  first_deploy || return 1
  install_systemd_units || return 1
  enable_base_timers || return 1
  with_lock make_backup || return 1
  with_lock internal_restore_test || return 1
  enable_backup_timers || return 1
  verify_deploy || return 1
  ok '全部安装向导完成：部署、迁移、HTTPS、防火墙、远程备份、恢复演练和定时任务均已验证。'
}

pg_dump_to_file() {
  local target=$1
  if ! compose exec -T postgres \
    pg_dump --format=custom --username cloudledger_migration --dbname cloudledger >"$target"; then
    rm -f -- "$target"
    fail 'pg_dump 返回失败状态。'
    return 1
  fi
  [[ -s "$target" ]] || { fail 'pg_dump 生成了空文件，拒绝创建不完整备份。'; return 1; }
  compose exec -T postgres pg_restore --list <"$target" >/dev/null || return 1
}

copy_backup_source() {
  local source=$1 target=$2
  [[ -s "$source" ]] || { fail "备份所需文件缺失: $source"; return 1; }
  cp -- "$source" "$target"
}

make_backup() {
  require_root
  ensure_dirs || return 1
  local id tmp archive candidate db_state
  id="$(date -u +%Y%m%d-%H%M%S)-$$"
  tmp="$BACKUP_DIR/.tmp-$id"
  archive="$BACKUP_DIR/cloudledger-$id.tar"
  candidate="$BACKUP_DIR/.cloudledger-$id.tar.new"
  rm -f -- "$candidate"
  register_sensitive_path "$candidate"
  mkdir -m 700 "$tmp" || { remove_sensitive_path "$candidate"; return 1; }
  ACTIVE_PLAINTEXT_DIR=$tmp
  load_env || { cleanup_plaintext; remove_sensitive_path "$candidate"; return 1; }
  log '创建一致性 pg_dump -Fc...'
  pg_dump_to_file "$tmp/postgres.dump" || { cleanup_plaintext; fail '数据库导出失败，旧备份保持不变。'; return 1; }
  copy_backup_source "$SERVER_CONFIG" "$tmp/server.toml" || { cleanup_plaintext; return 1; }
  copy_backup_source "$OPS_ENV" "$tmp/compose.env" || { cleanup_plaintext; return 1; }
  copy_backup_source "$DEPLOY_DIR/compose.yml" "$tmp/compose.yml" || { cleanup_plaintext; return 1; }
  copy_backup_source "$DEPLOY_DIR/Caddyfile" "$tmp/Caddyfile" || { cleanup_plaintext; return 1; }
  copy_backup_source "$(certificate_file)" "$tmp/origin-cert.pem" || { cleanup_plaintext; return 1; }
  copy_backup_source "$(certificate_key_file)" "$tmp/origin-key.pem" || { cleanup_plaintext; return 1; }
  db_state=exported
  if ! printf '{"id":"%s","created_at":"%s","version":"%s","database":"%s","format":"pg_dump-custom","files":["postgres.dump","server.toml","compose.env","compose.yml","Caddyfile","origin-cert.pem","origin-key.pem"]}\n' \
    "$id" "$(date -u +%FT%TZ)" "$OPS_VERSION" "$db_state" >"$tmp/manifest.json"; then
    cleanup_plaintext; remove_sensitive_path "$candidate"; return 1
  fi
  if ! (cd "$tmp" && sha256sum postgres.dump server.toml compose.env compose.yml Caddyfile origin-cert.pem origin-key.pem manifest.json >SHA256SUMS); then
    cleanup_plaintext; fail '无法生成备份校验清单。'; return 1
  fi
  if ! tar -C "$tmp" -cf "$candidate" .; then
    remove_sensitive_path "$candidate"; cleanup_plaintext; fail '无法生成备份归档。'; return 1
  fi
  chmod 600 "$candidate" || { remove_sensitive_path "$candidate"; cleanup_plaintext; return 1; }
  if ! cleanup_plaintext; then
    remove_sensitive_path "$candidate" || true
    fail '明文备份暂存目录清理失败，备份不会发布。'
    return 1
  fi
  if ! verify_backup_archive "$candidate"; then
    remove_sensitive_path "$candidate"
    return 1
  fi
  if ! upload_backup "$candidate" "$(basename "$archive")"; then
    remove_sensitive_path "$candidate"
    fail '远程上传或下载回验失败，旧备份不会被清理。'
    return 1
  fi
  if ! mv -f -- "$candidate" "$archive"; then
    remove_sensitive_path "$candidate"
    fail '已验证备份无法发布到本地正式文件名。'
    return 1
  fi
  unregister_sensitive_path "$candidate"
  prune_remote_backups || return 1
  prune_backups || return 1
  printf '%s\n' "$archive" >"$STATE_DIR/last-backup" || return 1
  ok "完整备份完成: $archive"
}

prune_backups() {
  load_env || return 1
  local retention=${CLOUDLEDGER_BACKUP_RETENTION:-30} count=0 file
  [[ "$retention" =~ ^[1-9][0-9]*$ ]] || retention=30
  while IFS= read -r file; do
    valid_backup_name "${file##*/}" || continue
    count=$((count + 1))
    if (( count > retention )) && ! rm -f -- "$file"; then
      fail "无法删除超过保留数量的本地备份: $file"
      return 1
    fi
  done < <(find "$BACKUP_DIR" -maxdepth 1 -type f -name 'cloudledger-*.tar' -printf '%T@ %p\n' 2>/dev/null | sort -nr | cut -d' ' -f2-)
  return 0
}

prune_remote_backups() {
  load_env || return 1
  local remote=${CLOUDLEDGER_RCLONE_REMOTE:-} retention=${CLOUDLEDGER_BACKUP_RETENTION:-30}
  local listing file count=0
  [[ -n "$remote" ]] || return 0
  [[ "$retention" =~ ^[1-9][0-9]*$ ]] || retention=30
  listing=$(mktemp "$STATE_DIR/.remote-list.XXXXXX")
  if ! rclone lsf --files-only "$remote" >"$listing"; then
    rm -f -- "$listing"
    fail '无法列出远端备份，未执行任何远端清理。'
    return 1
  fi
  while IFS= read -r file; do
    [[ "$file" =~ ^cloudledger-[0-9]{8}-[0-9]{6}-[0-9]+\.tar$ ]] || continue
    count=$((count + 1))
    if (( count > retention )); then
      rclone deletefile "${remote%/}/$file" || { rm -f -- "$listing"; return 1; }
    fi
  done < <(sort -r "$listing")
  rm -f -- "$listing"
}

list_backups() {
  ensure_dirs || return 1
  find "$BACKUP_DIR" -maxdepth 1 -type f -name 'cloudledger-*.tar' \
    -printf '%TY-%Tm-%Td %TH:%TM %s bytes %f\n' 2>/dev/null | sort -r || true
}

download_remote_backup() {
  local name=$1 remote candidate final
  require_root
  ensure_dirs || return 1
  load_env || return 1
  remote=${CLOUDLEDGER_RCLONE_REMOTE:-}
  valid_backup_name "$name" || { fail '远程备份文件名格式无效。'; return 1; }
  [[ -n "$remote" ]] || { fail '未配置 rclone crypt remote。'; return 1; }
  validate_rclone_crypt_remote "$remote" || return 1
  candidate="$BACKUP_DIR/.${name}.new"
  final="$BACKUP_DIR/$name"
  rm -f -- "$candidate"
  register_sensitive_path "$candidate"
  if ! rclone copyto "${remote%/}/$name" "$candidate" || ! verify_backup_archive "$candidate"; then
    remove_sensitive_path "$candidate"
    fail '远程备份下载或校验失败，现有本地备份保持不变。'
    return 1
  fi
  if ! mv -f -- "$candidate" "$final"; then
    remove_sensitive_path "$candidate"
    return 1
  fi
  unregister_sensitive_path "$candidate"
  chmod 600 "$final" || return 1
  ok "远程备份已校验并原子保存: $final"
}

backup_file() {
  local id=$1 file name
  if [[ "$id" == "$BACKUP_DIR/"* ]]; then
    name=${id##*/}
    [[ "$id" == "$BACKUP_DIR/$name" ]] || { fail '备份路径不在受保护的备份目录中。'; return 1; }
  else
    [[ "$id" != */* ]] || { fail '备份只能从受保护的备份目录中选择。'; return 1; }
    if valid_backup_name "$id"; then name=$id
    elif [[ "$id" =~ ^[0-9]{8}-[0-9]{6}-[0-9]+$ ]]; then name="cloudledger-$id.tar"
    else fail '备份编号格式无效。'; return 1
    fi
  fi
  valid_backup_name "$name" || { fail '备份文件名格式无效。'; return 1; }
  file="$BACKUP_DIR/$name"
  [[ -f "$file" && ! -L "$file" ]] || { fail '找不到备份或备份不是普通文件。'; return 1; }
  printf '%s' "$file"
}

verify_backup() {
  local requested=$1 archive
  archive=$(backup_file "$requested") || return 1
  verify_backup_archive "$archive"
}

verify_backup_archive() {
  local archive=$1 temp root requested error=''
  [[ -f "$archive" && ! -L "$archive" ]] || { fail '备份归档不是普通文件。'; return 1; }
  temp=$(mktemp -d "${TMPDIR:-/tmp}/cloudledger-verify.XXXXXX")
  register_sensitive_path "$temp"
  if ! root=$(extract_backup "$archive" "$temp"); then
    error='备份归档成员不安全、超限或损坏。'
  elif [[ ! -s "$root/manifest.json" || ! -s "$root/SHA256SUMS" ]]; then
    error='备份缺少 manifest.json 或 SHA256SUMS。'
  fi
  if [[ -z "$error" ]]; then
    for requested in postgres.dump server.toml compose.env compose.yml Caddyfile origin-cert.pem origin-key.pem; do
      [[ -s "$root/$requested" ]] || { error="备份缺少文件: $requested"; break; }
    done
  fi
  if [[ -z "$error" ]] && ! validate_checksum_manifest "$root/SHA256SUMS"; then
    error='SHA256SUMS 包含未知、重复或不完整成员。'
  fi
  if [[ -z "$error" ]] && ! jq -e \
    '.database == "exported" and .format == "pg_dump-custom" and
     .files == ["postgres.dump","server.toml","compose.env","compose.yml","Caddyfile","origin-cert.pem","origin-key.pem"]' \
    "$root/manifest.json" >/dev/null; then
    error='manifest.json 内容无效。'
  fi
  if [[ -z "$error" ]] && ! validate_backup_identity "$root" "$archive"; then
    error='manifest.json 的备份编号、文件名或创建时间不一致。'
  fi
  if [[ -z "$error" ]] && ! (cd "$root" && sha256sum -c SHA256SUMS >/dev/null); then
    error='备份 SHA-256 校验失败。'
  fi
  if [[ -z "$error" ]] && ! compose exec -T postgres pg_restore --list <"$root/postgres.dump" >/dev/null; then
    error='postgres.dump 不是有效 pg_dump -Fc。'
  fi
  if [[ -z "$error" ]] && ! validate_candidate_bundle "$root"; then
    error='备份部署配置未通过当前工具的信任校验。'
  fi
  if ! remove_sensitive_path "$temp"; then
    [[ -n "$error" ]] || error='备份内容已验证，但敏感校验暂存目录清理失败。'
  fi
  if [[ -n "$error" ]]; then fail "$error"; return 1; fi
  ok "备份校验通过: $archive"
}

validate_backup_identity() {
  local root=$1 archive=$2 expected_id manifest_id created_at id_epoch created_epoch now
  expected_id=$(backup_id_from_archive "$archive") || return 1
  manifest_id=$(jq -er '.id | select(type == "string")' "$root/manifest.json") || return 1
  created_at=$(jq -er '.created_at | select(type == "string")' "$root/manifest.json") || return 1
  [[ "$manifest_id" == "$expected_id" ]] || return 1
  id_epoch=$(date -u -d \
    "${expected_id:0:4}-${expected_id:4:2}-${expected_id:6:2} ${expected_id:9:2}:${expected_id:11:2}:${expected_id:13:2}Z" \
    +%s 2>/dev/null) || return 1
  created_epoch=$(date -u -d "$created_at" +%s 2>/dev/null) || return 1
  now=$(date -u +%s)
  (( created_epoch >= id_epoch && created_epoch <= now + 300 ))
}

write_remote_backup_checkpoint() {
  local name=$1 current='' candidate name_timestamp current_timestamp
  valid_backup_name "$name" || return 1
  name_timestamp=$(backup_timestamp_from_name "$name") || return 1
  if [[ -e "$REMOTE_BACKUP_CHECKPOINT" ]]; then
    [[ -f "$REMOTE_BACKUP_CHECKPOINT" && ! -L "$REMOTE_BACKUP_CHECKPOINT" ]] || return 1
    IFS= read -r current <"$REMOTE_BACKUP_CHECKPOINT" || return 1
    valid_backup_name "$current" || return 1
    current_timestamp=$(backup_timestamp_from_name "$current") || return 1
    [[ "$name_timestamp" < "$current_timestamp" ]] && return 1
  fi
  candidate=$(mktemp "$STATE_DIR/.remote-backup-checkpoint.XXXXXX") || return 1
  register_sensitive_path "$candidate"
  chmod 600 "$candidate" || { remove_sensitive_path "$candidate"; return 1; }
  printf '%s\n' "$name" >"$candidate" || { remove_sensitive_path "$candidate"; return 1; }
  mv -f -- "$candidate" "$REMOTE_BACKUP_CHECKPOINT" || { remove_sensitive_path "$candidate"; return 1; }
  unregister_sensitive_path "$candidate"
}

validate_remote_backup_freshness() {
  local root=$1 name=$2 checkpoint='' created_at created_epoch now max_hours name_timestamp checkpoint_timestamp
  valid_backup_name "$name" || return 1
  name_timestamp=$(backup_timestamp_from_name "$name") || return 1
  created_at=$(jq -er '.created_at | select(type == "string")' "$root/manifest.json") || return 1
  created_epoch=$(date -u -d "$created_at" +%s 2>/dev/null) || return 1
  now=$(date -u +%s)
  max_hours=${CLOUDLEDGER_REMOTE_BACKUP_MAX_AGE_HOURS:-$DEFAULT_REMOTE_BACKUP_MAX_AGE_HOURS}
  [[ "$max_hours" =~ ^[1-9][0-9]{0,4}$ ]] && (( max_hours <= 8760 )) \
    || { fail '远端备份最大年龄必须是 1 到 8760 小时。'; return 1; }
  if (( now - created_epoch > max_hours * 3600 )); then
    fail "远端最新备份已超过 ${max_hours} 小时，拒绝记录恢复演练成功。"
    return 1
  fi
  if [[ -e "$REMOTE_BACKUP_CHECKPOINT" ]]; then
    [[ -f "$REMOTE_BACKUP_CHECKPOINT" && ! -L "$REMOTE_BACKUP_CHECKPOINT" ]] \
      || { fail '远端备份单调检查点不是受保护的普通文件。'; return 1; }
    IFS= read -r checkpoint <"$REMOTE_BACKUP_CHECKPOINT" || return 1
    valid_backup_name "$checkpoint" || { fail '远端备份单调检查点已损坏。'; return 1; }
    checkpoint_timestamp=$(backup_timestamp_from_name "$checkpoint") || return 1
    if [[ "$name_timestamp" < "$checkpoint_timestamp" ]]; then
      fail "远端最新备份早于本机已验证检查点，疑似回滚: $checkpoint"
      return 1
    fi
  else
    warn '尚无远端备份单调检查点；本次演练成功后将建立。'
  fi
}

validate_checksum_manifest() {
  local file=$1 line hash name count=0 seen=' '
  while IFS= read -r line || [[ -n "$line" ]]; do
    hash=${line%%  *}
    name=${line#*  }
    [[ "$hash" =~ ^[0-9a-f]{64}$ ]] || return 1
    case "$name" in
      postgres.dump|server.toml|compose.env|compose.yml|Caddyfile|origin-cert.pem|origin-key.pem|manifest.json) ;;
      *) return 1 ;;
    esac
    [[ "$seen" != *" $name "* ]] || return 1
    seen+="$name "
    count=$((count + 1))
  done <"$file"
  [[ "$count" -eq 8 ]]
}

upload_backup() {
  load_env || return 1
  local archive=$1 remote_name=${2:-$(basename "$1")} remote=${CLOUDLEDGER_RCLONE_REMOTE:-}
  local downloaded pending_remote final_remote
  [[ -n "$remote" ]] || { warn '未配置 rclone crypt remote，仅保留本地备份。'; return 0; }
  command_exists rclone || { fail '已配置远程备份但未安装 rclone。'; return 1; }
  validate_rclone_crypt_remote "$remote" || return 1
  valid_backup_name "$remote_name" || { fail '远程备份文件名格式无效。'; return 1; }
  downloaded=$(mktemp "$STATE_DIR/.remote-download.XXXXXX")
  register_sensitive_path "$downloaded"
  chmod 600 "$downloaded" || { remove_sensitive_path "$downloaded"; return 1; }
  pending_remote="${remote%/}/.${remote_name}.new"
  final_remote="${remote%/}/${remote_name}"
  ACTIVE_REMOTE_PENDING=$pending_remote
  if ! rclone copyto "$archive" "$pending_remote" \
    || ! rclone copyto "$pending_remote" "$downloaded" \
    || ! cmp -s "$archive" "$downloaded"; then
    remove_sensitive_path "$downloaded"
    cleanup_remote_pending
    return 1
  fi
  if ! remove_sensitive_path "$downloaded"; then
    cleanup_remote_pending || true
    return 1
  fi
  if ! rclone moveto "$pending_remote" "$final_remote"; then
    cleanup_remote_pending
    return 1
  fi
  ACTIVE_REMOTE_PENDING=''
  if ! write_remote_backup_checkpoint "$remote_name"; then
    fail '远端备份已发布，但无法推进本机单调检查点；任务按失败处理且不会清理旧备份。'
    return 1
  fi
  ok 'rclone crypt 上传和下载回验通过。'
}

extract_backup() {
  local archive=$1 directory=$2 listing mode owner size _date_field _time_field member extra
  local name allowed seen=' ' regular_count=0 directory_count=0 total_size=0 max_bytes max_small_bytes=4194304
  allowed=' postgres.dump server.toml compose.env compose.yml Caddyfile origin-cert.pem origin-key.pem manifest.json SHA256SUMS '
  max_bytes=${CLOUDLEDGER_MAX_BACKUP_BYTES:-$DEFAULT_MAX_BACKUP_BYTES}
  [[ "$max_bytes" =~ ^[1-9][0-9]*$ ]] || return 1
  [[ -f "$archive" && ! -L "$archive" ]] || return 1
  size=$(stat -c '%s' "$archive") || return 1
  (( size > 0 && size <= max_bytes )) || return 1
  listing="$directory/.tar-listing"
  tar --list --verbose --numeric-owner --file "$archive" >"$listing" || return 1
  while IFS=' ' read -r mode owner size _date_field _time_field member extra; do
    [[ -n "$mode" && -n "$member" && -z "${extra:-}" ]] || return 1
    if [[ "$mode" == d* && "$member" == './' ]]; then
      directory_count=$((directory_count + 1))
      (( directory_count == 1 )) || return 1
      continue
    fi
    [[ "$mode" == -* && "$size" =~ ^[0-9]+$ && "$member" == ./* ]] || return 1
    name=${member#./}
    [[ "$name" != */* && "$allowed" == *" $name "* ]] || return 1
    [[ "$seen" != *" $name "* ]] || return 1
    (( size > 0 )) || return 1
    if [[ "$name" == postgres.dump ]]; then (( size <= max_bytes )) || return 1
    else (( size <= max_small_bytes )) || return 1
    fi
    total_size=$((total_size + size))
    (( total_size <= max_bytes )) || return 1
    seen+="$name "
    regular_count=$((regular_count + 1))
  done <"$listing"
  (( regular_count == 9 )) || return 1
  umask 077
  for name in postgres.dump server.toml compose.env compose.yml Caddyfile origin-cert.pem origin-key.pem manifest.json SHA256SUMS; do
    tar -xOf "$archive" "./$name" >"$directory/$name" || return 1
  done
  rm -f -- "$listing"
  printf '%s' "$directory"
}

postgres_drop_database() {
  local db=$1
  [[ "$db" == cloudledger || "$db" =~ ^cloudledger_restore_test_[0-9]+_[0-9]+$ ]] \
    || { fail '拒绝删除不受 CloudLedger 管理的数据库。'; return 1; }
  compose exec -T postgres dropdb --if-exists --force --maintenance-db postgres \
    --username cloudledger_bootstrap "$db"
}

postgres_create_database() {
  local db=$1
  [[ "$db" == cloudledger || "$db" =~ ^cloudledger_restore_test_[0-9]+_[0-9]+$ ]] \
    || { fail '拒绝创建不受 CloudLedger 管理的数据库。'; return 1; }
  compose exec -T postgres createdb --maintenance-db postgres --username cloudledger_bootstrap "$db"
}

restore_dump_to_database() {
  local dump=$1 db=$2
  [[ -s "$dump" ]] || { fail '拒绝从空数据库 dump 恢复。'; return 1; }
  postgres_drop_database "$db" || return 1
  postgres_create_database "$db" || return 1
  postgres_psql "$db" -c 'GRANT USAGE, CREATE ON SCHEMA public TO cloudledger_migration;' || return 1
  compose exec -T postgres \
    pg_restore --single-transaction --exit-on-error --no-owner --role cloudledger_migration \
      --username cloudledger_bootstrap --dbname "$db" <"$dump"
}

copy_current_bundle() {
  local target=$1 cert key
  cert=$(certificate_file) || return 1
  key=$(certificate_key_file) || return 1
  mkdir -p "$target" || return 1
  chmod 700 "$target" || return 1
  copy_backup_source "$SERVER_CONFIG" "$target/server.toml" || return 1
  copy_backup_source "$OPS_ENV" "$target/compose.env" || return 1
  copy_backup_source "$DEPLOY_DIR/compose.yml" "$target/compose.yml" || return 1
  copy_backup_source "$DEPLOY_DIR/Caddyfile" "$target/Caddyfile" || return 1
  copy_backup_source "$cert" "$target/origin-cert.pem" || return 1
  copy_backup_source "$key" "$target/origin-key.pem" || return 1
  normalize_ops_env_file "$target/compose.env" "$target/compose.env.normalized" \
    || { fail '当前 ops.env 无法规范化，拒绝创建恢复回滚快照。'; return 1; }
}

validate_candidate_bundle() {
  local root=$1 normalized trusted_compose trusted_caddy expected_server
  local key value expected_image release_tag api_domain cert_path key_path owner image_owner trusted_owner
  local bootstrap_password migration_password runtime_password bootstrap_url migration_url
  local -a clean_env=(env)
  load_env || return 1
  trusted_owner=${CLOUDLEDGER_GHCR_OWNER:-}
  if [[ -z "$trusted_owner" && "${CLOUDLEDGER_SERVER_IMAGE:-}" == ghcr.io/*/*:* ]]; then
    trusted_owner=${CLOUDLEDGER_SERVER_IMAGE#ghcr.io/}
    trusted_owner=${trusted_owner%%/*}
  fi
  [[ "$trusted_owner" =~ ^[a-z0-9_.-]+$ ]] \
    || { fail '当前部署缺少可信 GHCR owner；请先通过镜像仓库配置菜单设置。'; return 1; }
  normalized="$root/compose.env.normalized"
  trusted_compose="$SCRIPT_DIR/docker-compose.yml"
  [[ -f "$trusted_compose" ]] || trusted_compose=$COMPOSE_FILE
  trusted_caddy="$SCRIPT_DIR/Caddyfile"
  [[ -f "$trusted_caddy" ]] || trusted_caddy="$DEPLOY_DIR/Caddyfile"
  [[ -f "$trusted_compose" && -f "$trusted_caddy" ]] || { fail '缺少当前版本的可信 Compose 或 Caddy 模板。'; return 1; }
  cmp -s "$root/compose.yml" "$trusted_compose" \
    || { fail '备份 Compose 与当前运维工具版本不匹配。'; return 1; }
  cmp -s "$root/Caddyfile" "$trusted_caddy" \
    || { fail '备份 Caddyfile 与当前运维工具版本不匹配。'; return 1; }
  normalize_ops_env_file "$root/compose.env" "$normalized" \
    || { fail '备份 compose.env 包含未知键、重复键或不安全语法。'; return 1; }
  for key in CLOUDLEDGER_HTTP_HOST_PORT CLOUDLEDGER_HTTPS_HOST_PORT CLOUDLEDGER_ADMIN_TUNNEL_PORT; do
    if ops_env_file_value "$normalized" "$key" value; then
      fail '备份包含已废弃的 v0.1.3 端口配置。'
      return 1
    fi
  done
  if ! ops_env_file_value "$normalized" CLOUDLEDGER_GHCR_OWNER owner \
    || [[ ! "$owner" =~ ^[a-z0-9_.-]+$ || "$owner" != "$trusted_owner" ]]; then
    fail "备份 GHCR owner 不匹配当前可信 owner: $trusted_owner"
    return 1
  fi
  if ! ops_env_file_value "$normalized" CLOUDLEDGER_RELEASE_TAG release_tag || ! valid_tag "$release_tag"; then
    fail '备份缺少有效的明确 release tag。'
    return 1
  fi
  for key in CLOUDLEDGER_SERVER_IMAGE CLOUDLEDGER_POSTGRES_IMAGE CLOUDLEDGER_CADDY_IMAGE CLOUDLEDGER_ANCHOR_IMAGE; do
    ops_env_file_value "$normalized" "$key" value || { fail "备份缺少镜像配置: $key"; return 1; }
    case "$key" in
      CLOUDLEDGER_SERVER_IMAGE) expected_image=cloudledger-server ;;
      CLOUDLEDGER_POSTGRES_IMAGE) expected_image=cloudledger-postgres ;;
      CLOUDLEDGER_CADDY_IMAGE) expected_image=cloudledger-caddy ;;
      CLOUDLEDGER_ANCHOR_IMAGE) expected_image=cloudledger-network-anchor ;;
    esac
    valid_ghcr_image "$value" "$expected_image" "$release_tag" \
      || { fail "备份镜像不符合固定 GHCR tag 约束: $key"; return 1; }
    image_owner=${value#ghcr.io/}
    image_owner=${image_owner%%/*}
    [[ "$image_owner" == "$trusted_owner" ]] \
      || { fail "备份镜像不来自当前可信 GHCR owner: $key"; return 1; }
  done
  if ! ops_env_file_value "$normalized" CLOUDLEDGER_API_DOMAIN api_domain || ! valid_domain "$api_domain"; then
    fail '备份 API 域名无效.'
    return 1
  fi
  if ! ops_env_file_value "$normalized" CLOUDLEDGER_HTTP_PUBLISH value || [[ "$value" != '127.0.0.1:18080:80' ]]; then
    fail '备份 HTTP 发布地址不符合本机监听约束。'
    return 1
  fi
  if ! ops_env_file_value "$normalized" CLOUDLEDGER_HTTPS_PUBLISH value || [[ "$value" != '443:443' ]]; then
    fail '备份 HTTPS 发布地址不符合固定约束。'
    return 1
  fi
  if ! ops_env_file_value "$normalized" CLOUDLEDGER_CADDY_ORIGIN_CERT_PATH cert_path \
    || [[ "$cert_path" != "$CERT_DIR/origin-cert.pem" ]]; then
    fail '备份 Origin CA 证书目标路径不安全。'
    return 1
  fi
  if ! ops_env_file_value "$normalized" CLOUDLEDGER_CADDY_ORIGIN_KEY_PATH key_path \
    || [[ "$key_path" != "$CERT_DIR/origin-key.pem" ]]; then
    fail '备份 Origin CA 私钥目标路径不安全。'
    return 1
  fi
  ops_env_file_value "$normalized" CLOUDLEDGER_BOOTSTRAP_DB_PASSWORD bootstrap_password \
    || { fail '备份缺少 bootstrap 数据库密码。'; return 1; }
  ops_env_file_value "$normalized" CLOUDLEDGER_MIGRATION_DB_PASSWORD migration_password \
    || { fail '备份缺少 migration 数据库密码。'; return 1; }
  ops_env_file_value "$normalized" CLOUDLEDGER_RUNTIME_DB_PASSWORD runtime_password \
    || { fail '备份缺少 runtime 数据库密码。'; return 1; }
  for value in "$bootstrap_password" "$migration_password" "$runtime_password"; do
    [[ "$value" =~ ^[A-Za-z0-9_-]{16,128}$ ]] \
      || { fail '备份数据库密码格式不符合安全约束。'; return 1; }
  done
  ops_env_file_value "$normalized" CLOUDLEDGER_BOOTSTRAP_DATABASE_URL bootstrap_url \
    || { fail '备份缺少 bootstrap 数据库 URL。'; return 1; }
  ops_env_file_value "$normalized" CLOUDLEDGER_MIGRATION_DATABASE_URL migration_url \
    || { fail '备份缺少 migration 数据库 URL。'; return 1; }
  [[ "$bootstrap_url" == "postgres://cloudledger_bootstrap:${bootstrap_password}@127.0.0.1:5432/cloudledger" \
    && "$migration_url" == "postgres://cloudledger_migration:${migration_password}@127.0.0.1:5432/cloudledger" ]] \
    || { fail '备份数据库 URL 与受保护角色或密码不一致。'; return 1; }
  expected_server=$(mktemp "$root/.server.expected.XXXXXX") || return 1
  chmod 600 "$expected_server" || { rm -f -- "$expected_server"; return 1; }
  if ! write_server_config_from_env_file "$normalized" "$expected_server" cloudledger \
    || ! cmp -s "$root/server.toml" "$expected_server"; then
    rm -f -- "$expected_server"
    fail '备份 server.toml 不符合当前工具的安全配置模板。'
    return 1
  fi
  rm -f -- "$expected_server"
  validate_certificate_pair "$root/origin-cert.pem" "$root/origin-key.pem" "$api_domain" || return 1
  for key in "${OPS_CONFIG_KEYS[@]}"; do clean_env+=(-u "$key"); done
  "${clean_env[@]}" docker compose --env-file "$normalized" --project-directory "$root" -f "$root/compose.yml" \
    --profile migration config --quiet || return 1
}

install_restore_bundle() {
  local root=$1 cert_target key_target normalized
  normalized="$root/compose.env.normalized"
  [[ -s "$normalized" ]] || { fail '恢复候选缺少已规范化的 compose.env。'; return 1; }
  atomic_copy "$root/server.toml" "$SERVER_CONFIG" 0600 || return 1
  if [[ ${EUID:-$(id -u)} -eq 0 ]]; then
    chown 10001:10001 "$SERVER_CONFIG" || { fail '无法设置恢复后 server.toml 的运行用户所有权。'; return 1; }
  fi
  atomic_copy "$normalized" "$OPS_ENV" 0600 || return 1
  atomic_copy "$root/compose.yml" "$DEPLOY_DIR/compose.yml" 0644 || return 1
  atomic_copy "$root/Caddyfile" "$DEPLOY_DIR/Caddyfile" 0644 || return 1
  load_env || return 1
  cert_target=$(certificate_file); key_target=$(certificate_key_file)
  atomic_copy "$root/origin-cert.pem" "$cert_target" 0644 || return 1
  atomic_copy "$root/origin-key.pem" "$key_target" 0600 || return 1
}

apply_deployment_role_passwords() {
  load_env || return 1
  local sql
  for value in "$CLOUDLEDGER_BOOTSTRAP_DB_PASSWORD" "$CLOUDLEDGER_MIGRATION_DB_PASSWORD" "$CLOUDLEDGER_RUNTIME_DB_PASSWORD"; do
    [[ "$value" =~ ^[A-Za-z0-9_-]{16,}$ ]] || { fail '备份中的数据库密码格式不符合安全约束。'; return 1; }
  done
  sql=$(mktemp "$STATE_DIR/.role-passwords.XXXXXX")
  register_sensitive_path "$sql"
  chmod 600 "$sql" || { remove_sensitive_path "$sql"; return 1; }
  {
    printf "\\set bootstrap_password '%s'\n" "$CLOUDLEDGER_BOOTSTRAP_DB_PASSWORD"
    printf "\\set migration_password '%s'\n" "$CLOUDLEDGER_MIGRATION_DB_PASSWORD"
    printf "\\set runtime_password '%s'\n" "$CLOUDLEDGER_RUNTIME_DB_PASSWORD"
    printf '%s\n' "ALTER ROLE cloudledger_bootstrap LOGIN SUPERUSER CREATEDB CREATEROLE PASSWORD :'bootstrap_password';"
    sed '/^--   psql -v /d' "$DEPLOY_DIR/postgres_roles.sql"
  } >"$sql" || { remove_sensitive_path "$sql"; return 1; }
  if ! compose exec -T postgres psql --username cloudledger_bootstrap --dbname cloudledger \
    --single-transaction --set ON_ERROR_STOP=1 --set database_name=cloudledger <"$sql"; then
    remove_sensitive_path "$sql"; return 1
  fi
  remove_sensitive_path "$sql" || return 1
}

start_and_verify_restored_stack() {
  compose pull network-anchor postgres migration cloudledger caddy || return 1
  compose up -d network-anchor postgres || return 1
  wait_for_postgres && verify_network_bridge && verify_database_roles || return 1
  verify_migrations_exact && harden_runtime_metadata_permissions && verify_audit || return 1
  compose up -d --no-deps cloudledger || return 1
  check_local_backend /health && check_local_backend /ready || return 1
  compose run --rm --no-deps caddy caddy validate --config /etc/caddy/Caddyfile || return 1
  compose up -d --no-deps caddy || return 1
  health_url '恢复后 /health' "$(api_base_url)/health" && health_url '恢复后 /ready' "$(api_base_url)/ready" \
    && verify_turnstile && firewall_status
}

preserve_failed_restore_snapshot() {
  local source=$ACTIVE_PLAINTEXT_DIR target
  [[ -n "$source" && -d "$source" ]] || return 1
  ensure_dirs || return 1
  target="$STATE_DIR/restore-rollback-failed-$(date -u +%Y%m%d-%H%M%S)-$$"
  if mv -- "$source" "$target"; then
    chmod 700 "$target" 2>/dev/null || true
    unregister_sensitive_path "$source"
    ACTIVE_PLAINTEXT_DIR=''
    fail "自动回滚未完全成功；数据库和配置快照已保留: $target"
    return 0
  fi
  unregister_sensitive_path "$source"
  ACTIVE_PLAINTEXT_DIR=''
  fail "自动回滚未完全成功；禁止清理现场，请立即保护快照目录: $source"
  return 1
}

rollback_active_restore() {
  local rollback=$ACTIVE_RESTORE_ROLLBACK_DIR failed=0
  (( RESTORE_TRANSACTION_ACTIVE == 1 )) || return 0
  RESTORE_TRANSACTION_ACTIVE=0
  ACTIVE_RESTORE_ROLLBACK_DIR=''
  trap '' HUP INT TERM
  fail '恢复事务未提交，正在回滚旧数据库、配置和角色密码。'
  compose stop caddy cloudledger >/dev/null 2>&1 || true
  if [[ ! -s "$rollback/postgres.dump" ]]; then
    fail '严重错误: 恢复回滚数据库快照缺失。'
    failed=1
  elif ! restore_dump_to_database "$rollback/postgres.dump" cloudledger; then
    fail '严重错误: 旧数据库自动回滚失败。'
    failed=1
  fi
  if [[ ! -s "$rollback/compose.env.normalized" ]] || ! install_restore_bundle "$rollback"; then
    fail '严重错误: 旧配置自动回滚失败。'
    failed=1
  fi
  if (( failed == 0 )) && ! apply_deployment_role_passwords; then
    fail '严重错误: 旧数据库角色密码自动回滚失败。'
    failed=1
  fi
  if (( failed == 0 )) && ! start_and_verify_restored_stack; then
    fail '严重错误: 旧部署已回滚但健康验证失败。'
    failed=1
  fi
  if (( failed != 0 )); then
    preserve_failed_restore_snapshot || true
    install_cleanup_traps
    return 1
  fi
  install_cleanup_traps
  ok '恢复事务已自动回滚到操作前状态。'
}

restore_locked() {
  local archive=$1 temp root dump rollback failed=0
  verify_backup "$archive" || return 1
  temp=$(mktemp -d "${TMPDIR:-/tmp}/cloudledger-restore.XXXXXX")
  register_sensitive_path "$temp"
  ACTIVE_PLAINTEXT_DIR=$temp
  root=$(extract_backup "$archive" "$temp") || { cleanup_plaintext; unregister_sensitive_path "$temp"; fail '无法解压备份。'; return 1; }
  dump="$root/postgres.dump"
  [[ -s "$dump" ]] || { cleanup_plaintext; unregister_sensitive_path "$temp"; fail '备份中没有可恢复的数据库 dump。'; return 1; }
  validate_candidate_bundle "$root" || { cleanup_plaintext; unregister_sensitive_path "$temp"; fail '备份中的部署配置无效或与当前工具版本不匹配。'; return 1; }
  rollback="$temp/rollback"
  mkdir -m 700 "$rollback" || { cleanup_plaintext; unregister_sensitive_path "$temp"; return 1; }
  pg_dump_to_file "$rollback/postgres.dump" || { cleanup_plaintext; unregister_sensitive_path "$temp"; fail '无法创建恢复前数据库回滚快照。'; return 1; }
  copy_current_bundle "$rollback" || { cleanup_plaintext; unregister_sensitive_path "$temp"; fail '无法创建恢复前配置回滚快照。'; return 1; }
  log "影响范围: 当前 CloudLedger 数据库、后端配置、Compose 和 Origin CA 文件将被覆盖。"
  ACTIVE_RESTORE_ROLLBACK_DIR=$rollback
  RESTORE_TRANSACTION_ACTIVE=1
  compose stop caddy cloudledger || true
  restore_dump_to_database "$dump" cloudledger || failed=1
  if (( failed == 0 )); then install_restore_bundle "$root" || failed=1; fi
  if (( failed == 0 )); then apply_deployment_role_passwords || failed=1; fi
  if (( failed == 0 )); then start_and_verify_restored_stack || failed=1; fi
  if (( failed != 0 )); then
    fail '目标备份恢复验证失败。'
    rollback_active_restore || true
    cleanup_plaintext
    unregister_sensitive_path "$temp"
    return 1
  fi
  RESTORE_TRANSACTION_ACTIVE=0
  ACTIVE_RESTORE_ROLLBACK_DIR=''
  ACTIVE_PLAINTEXT_DIR=''
  remove_sensitive_path "$temp" || return 1
  ok '数据库、配置、角色密码和服务已从同一备份事务恢复。'
}

restore_backup() {
  require_root
  local id=$1 archive confirmation again
  archive=$(backup_file "$id") || return 1
  verify_backup "$archive" || return 1
  log "将覆盖当前 CloudLedger 数据和配置: $archive"
  press_confirm '确认执行不可逆恢复' || { warn '已取消。'; return 0; }
  read -r -p '再次输入完整备份文件名以确认: ' again
  confirmation=$(basename "$archive")
  [[ "$again" == "$confirmation" ]] || { fail '备份编号不匹配，已取消。'; return 1; }
  with_lock restore_locked "$archive"
}

latest_remote_backup() {
  load_env || return 1
  local remote=${CLOUDLEDGER_RCLONE_REMOTE:-} latest
  [[ -n "$remote" ]] || return 1
  latest=$(rclone lsf --files-only "$remote" 2>/dev/null | grep -E '^cloudledger-[0-9]{8}-[0-9]{6}-[0-9]+\.tar$' | sort | tail -n1)
  [[ -n "$latest" ]] || return 1
  printf '%s/%s' "${remote%/}" "$latest"
}

internal_restore_test() {
  require_root
  ensure_dirs || return 1
  load_env || return 1
  local archive temp root dump db test_config table count remote_archive remote_name='' test_runtime_password
  archive=$(current_backup)
  if [[ -n "${CLOUDLEDGER_RCLONE_REMOTE:-}" ]]; then
    remote_archive=$(latest_remote_backup) || { fail '无法找到远程最新备份，恢复演练失败。'; return 1; }
    temp=$(mktemp -d "${TMPDIR:-/tmp}/cloudledger-restore-test-download.XXXXXX")
    register_sensitive_path "$temp"
    ACTIVE_PLAINTEXT_DIR=$temp
    remote_name=$(basename "$remote_archive")
    archive="$temp/$remote_name"
    rclone copyto "$remote_archive" "$archive" || { cleanup_plaintext; unregister_sensitive_path "$temp"; fail '无法下载远程最新备份。'; return 1; }
  else
    [[ -n "$archive" && -f "$archive" ]] || { fail '没有可用于恢复演练的本地备份。'; return 1; }
    temp=$(mktemp -d "${TMPDIR:-/tmp}/cloudledger-restore-test.XXXXXX")
    register_sensitive_path "$temp"
    ACTIVE_PLAINTEXT_DIR=$temp
  fi
  verify_backup_archive "$archive" || { cleanup_plaintext; unregister_sensitive_path "$temp"; return 1; }
  root=$(extract_backup "$archive" "$temp") || { cleanup_plaintext; unregister_sensitive_path "$temp"; return 1; }
  if [[ -n "$remote_name" ]] && ! validate_remote_backup_freshness "$root" "$remote_name"; then
    cleanup_plaintext; unregister_sensitive_path "$temp"; return 1
  fi
  dump="$root/postgres.dump"
  [[ -s "$dump" ]] || { cleanup_plaintext; unregister_sensitive_path "$temp"; fail '恢复演练归档中的 postgres.dump 为空。'; return 1; }
  validate_candidate_bundle "$root" || { cleanup_plaintext; unregister_sensitive_path "$temp"; fail '恢复演练归档中的部署配置不可信。'; return 1; }
  ops_env_file_value "$root/compose.env.normalized" CLOUDLEDGER_RUNTIME_DB_PASSWORD test_runtime_password \
    || { cleanup_plaintext; unregister_sensitive_path "$temp"; return 1; }
  db="cloudledger_restore_test_$(date +%s)_$$"
  [[ "$db" =~ ^cloudledger_restore_test_[0-9]+_[0-9]+$ ]] || { cleanup_plaintext; unregister_sensitive_path "$temp"; return 1; }
  ACTIVE_RESTORE_DB=$db
  if ! restore_dump_to_database "$dump" "$db"; then
    cleanup_restore_database || true
    cleanup_plaintext
    unregister_sensitive_path "$temp"
    fail '临时数据库 pg_restore 失败。'
    return 1
  fi
  count=$(postgres_psql "$db" -Atqc "SELECT coalesce(string_agg(version::text, ',' ORDER BY version), '') || '|' || count(*)::text || '|' || coalesce(bool_and(success), false)::text FROM _sqlx_migrations") || count=invalid
  if [[ ! "$count" =~ ^[0-9]+(,[0-9]+)*\|[0-9]+\|true$ ]]; then
    cleanup_restore_database || true; cleanup_plaintext; unregister_sensitive_path "$temp"
    fail "恢复演练迁移状态无效: $count"
    return 1
  fi
  local restored_versions=${count%%|*} restored_count=${count#*|} required
  restored_count=${restored_count%%|*}
  if (( restored_count < 5 )); then
    cleanup_restore_database || true; cleanup_plaintext; unregister_sensitive_path "$temp"
    fail "恢复演练迁移数量不足: $restored_count"
    return 1
  fi
  for required in 1 2 3 4 5; do
    if [[ ",$restored_versions," != *",$required,"* ]]; then
      cleanup_restore_database || true; cleanup_plaintext; unregister_sensitive_path "$temp"
      fail "恢复演练缺少基线 migration $required"
      return 1
    fi
  done
  for table in organizations ledgers financial_accounts categories transactions audit_events; do
    postgres_psql "$db" -Atqc "SELECT (to_regclass('public.$table') IS NOT NULL)::text" | grep -qx true \
      || { cleanup_restore_database || true; cleanup_plaintext; unregister_sensitive_path "$temp"; fail "恢复演练缺少核心表: $table"; return 1; }
  done
  test_config="$temp/restore-test.toml"
  if ! write_server_config_from_env_file "$root/compose.env.normalized" "$test_config" "$db"; then
    cleanup_restore_database || true
    cleanup_plaintext
    unregister_sensitive_path "$temp"
    fail '无法生成恢复演练专用配置。'
    return 1
  fi
  if [[ ${EUID:-$(id -u)} -eq 0 ]] && ! chown 10001:10001 "$test_config"; then
    cleanup_restore_database || true; cleanup_plaintext; unregister_sensitive_path "$temp"; return 1
  fi
  if ! chmod 600 "$test_config" \
    || [[ $(grep -Fxc "url = \"postgres://cloudledger_runtime:${test_runtime_password}@127.0.0.1:5432/${db}\"" "$test_config") -ne 1 ]]; then
    cleanup_restore_database || true; cleanup_plaintext; unregister_sensitive_path "$temp"
    fail '恢复演练配置未唯一指向临时数据库。'
    return 1
  fi
  compose --profile migration run --rm --no-deps -v "$test_config:/etc/cloudledger/restore-test.toml:ro" \
    migration audit verify --config /etc/cloudledger/restore-test.toml || {
      cleanup_restore_database || true
      cleanup_plaintext
      unregister_sensitive_path "$temp"
      fail '恢复演练审计链验证失败。'
      return 1
    }
  if ! cleanup_restore_database; then
    cleanup_plaintext
    unregister_sensitive_path "$temp"
    fail '恢复演练验证完成，但临时数据库清理失败；任务不会记录成功。'
    return 1
  fi
  ACTIVE_PLAINTEXT_DIR=''
  if ! remove_sensitive_path "$temp"; then
    fail '恢复演练验证完成，但敏感临时目录清理失败；任务不会记录成功。'
    return 1
  fi
  if [[ -n "$remote_name" ]] && ! write_remote_backup_checkpoint "$remote_name"; then
    fail '恢复演练通过，但无法原子更新远端备份单调检查点；任务不会记录成功。'
    return 1
  fi
  printf '%s restore-test success\n' "$(date -u +%FT%TZ)" >>"$STATE_DIR/restore-test.log"
  ok "临时数据库恢复演练通过：$restored_count 个 SQLx migration、核心业务表和审计链均已验证。"
}

legacy_deployment_detected() {
  [[ -n "${CLOUDLEDGER_HTTP_HOST_PORT:-}" || -n "${CLOUDLEDGER_HTTPS_HOST_PORT:-}" \
    || -n "${CLOUDLEDGER_ADMIN_TUNNEL_PORT:-}" ]]
}

server_config_value() {
  local variable=$1 section=$2 key=$3
  local -a values=()
  mapfile -t values < <(awk -v section="[$section]" -v key="$key" '
    /^\[/ { active = ($0 == section); next }
    active && $0 ~ "^[[:space:]]*" key "[[:space:]]*=" {
      line = $0
      sub("^[[:space:]]*" key "[[:space:]]*=[[:space:]]*\"", "", line)
      sub("\"[[:space:]]*$", "", line)
      print line
    }
  ' "$SERVER_CONFIG")
  [[ ${#values[@]} -eq 1 && "${values[0]}" =~ ^[A-Za-z0-9_-]+$ ]] || return 1
  printf -v "$variable" '%s' "${values[0]}"
}

validate_legacy_deployment() {
  local legacy_compose="$SCRIPT_DIR/legacy/compose-v0.1.3.yml" owner tag cert key
  [[ -f "$legacy_compose" && -f "$SCRIPT_DIR/Caddyfile" ]] \
    || { fail '缺少 v0.1.3 接管信任模板。'; return 1; }
  cmp -s "$COMPOSE_FILE" "$legacy_compose" \
    || { fail '旧 Compose 不等于受支持的 v0.1.3 模板，拒绝自动接管。'; return 1; }
  cmp -s "$DEPLOY_DIR/Caddyfile" "$SCRIPT_DIR/Caddyfile" \
    || { fail '旧 Caddyfile 不等于当前可信模板，拒绝自动接管。'; return 1; }
  [[ "${CLOUDLEDGER_HTTP_HOST_PORT:-}" == 18080 \
    && "${CLOUDLEDGER_HTTPS_HOST_PORT:-}" == 443 \
    && "${CLOUDLEDGER_ADMIN_TUNNEL_PORT:-}" == 8788 ]] \
    || { fail '旧部署端口不符合受支持的 v0.1.3 接管边界。'; return 1; }
  [[ "${CLOUDLEDGER_SERVER_IMAGE:-}" == ghcr.io/*/cloudledger-server:* \
    && "${CLOUDLEDGER_POSTGRES_IMAGE:-}" == ghcr.io/*/cloudledger-postgres:* ]] \
    || { fail '旧部署的 server/PostgreSQL 镜像来源无效。'; return 1; }
  owner=${CLOUDLEDGER_SERVER_IMAGE#ghcr.io/}; owner=${owner%%/*}
  tag=${CLOUDLEDGER_SERVER_IMAGE##*:}
  [[ "$owner" =~ ^[a-z0-9_.-]+$ && "$tag" == v0.1.3 ]] \
    && valid_ghcr_image "$CLOUDLEDGER_POSTGRES_IMAGE" cloudledger-postgres "$tag" \
    || { fail '仅支持接管 server/PostgreSQL 镜像同为 v0.1.3 的旧部署。'; return 1; }
  cert=${CLOUDLEDGER_CADDY_ORIGIN_CERT_PATH:-}
  key=${CLOUDLEDGER_CADDY_ORIGIN_KEY_PATH:-}
  [[ -f "$cert" && ! -L "$cert" && -f "$key" && ! -L "$key" ]] \
    || { fail '旧部署 Origin CA 文件缺失或不是普通文件。'; return 1; }
  validate_certificate_pair "$cert" "$key" "$CLOUDLEDGER_API_DOMAIN" || return 1
  ok "已识别可受控接管的 CloudLedger $tag 部署。"
}

create_legacy_upgrade_snapshot() {
  local id temp archive cert key name
  id=$(date -u +%Y%m%d-%H%M%S)-$$
  temp=$(mktemp -d "$STATE_DIR/.legacy-upgrade-$id.XXXXXX") || return 1
  register_sensitive_path "$temp"
  chmod 700 "$temp" || { remove_sensitive_path "$temp"; return 1; }
  cert=${CLOUDLEDGER_CADDY_ORIGIN_CERT_PATH:-}
  key=${CLOUDLEDGER_CADDY_ORIGIN_KEY_PATH:-}
  pg_dump_to_file "$temp/postgres.dump" || { remove_sensitive_path "$temp"; return 1; }
  copy_backup_source "$SERVER_CONFIG" "$temp/server.toml" || { remove_sensitive_path "$temp"; return 1; }
  copy_backup_source "$OPS_ENV" "$temp/ops.env" || { remove_sensitive_path "$temp"; return 1; }
  copy_backup_source "$COMPOSE_FILE" "$temp/compose.yml" || { remove_sensitive_path "$temp"; return 1; }
  copy_backup_source "$DEPLOY_DIR/Caddyfile" "$temp/Caddyfile" || { remove_sensitive_path "$temp"; return 1; }
  copy_backup_source "$DEPLOY_DIR/cloudledger-ops.sh" "$temp/cloudledger-ops.sh" || { remove_sensitive_path "$temp"; return 1; }
  copy_backup_source "$DEPLOY_DIR/postgres_roles.sql" "$temp/postgres_roles.sql" || { remove_sensitive_path "$temp"; return 1; }
  copy_backup_source "$cert" "$temp/origin-cert.pem" || { remove_sensitive_path "$temp"; return 1; }
  copy_backup_source "$key" "$temp/origin-key.pem" || { remove_sensitive_path "$temp"; return 1; }
  if ! (cd "$temp" && sha256sum postgres.dump server.toml ops.env compose.yml Caddyfile \
    cloudledger-ops.sh postgres_roles.sql origin-cert.pem origin-key.pem >SHA256SUMS \
    && sha256sum -c SHA256SUMS >/dev/null); then
    remove_sensitive_path "$temp"
    return 1
  fi
  archive="$STATE_DIR/cloudledger-legacy-pre-upgrade-$id.tar"
  tar -C "$temp" -cf "$archive.new" . || { rm -f -- "$archive.new"; remove_sensitive_path "$temp"; return 1; }
  chmod 600 "$archive.new" || { rm -f -- "$archive.new"; remove_sensitive_path "$temp"; return 1; }
  name=$(tar -tf "$archive.new" | wc -l)
  [[ "$name" -eq 11 ]] || { rm -f -- "$archive.new"; remove_sensitive_path "$temp"; return 1; }
  mv -f -- "$archive.new" "$archive" || { rm -f -- "$archive.new"; remove_sensitive_path "$temp"; return 1; }
  remove_sensitive_path "$temp" || return 1
  ok "旧版数据库与配置已保存为权限 0600 的接管快照: $archive"
}

snapshot_upgrade_assets() {
  local variable=$1 target name source marker
  target=$(mktemp -d "${TMPDIR:-/tmp}/cloudledger-upgrade-assets.XXXXXX") || return 1
  register_sensitive_path "$target"
  chmod 700 "$target" || { remove_sensitive_path "$target"; return 1; }
  for name in compose.yml Caddyfile postgres_roles.sql cloudledger-ops.sh; do
    copy_backup_source "$DEPLOY_DIR/$name" "$target/$name" \
      || { remove_sensitive_path "$target"; return 1; }
  done
  copy_backup_source "$SERVER_CONFIG" "$target/server.toml" \
    || { remove_sensitive_path "$target"; return 1; }
  for name in origin-cert.pem origin-key.pem; do
    source="$CERT_DIR/$name"
    marker="$target/had-$name"
    if [[ -e "$source" || -L "$source" ]]; then
      [[ -f "$source" && ! -L "$source" ]] \
        || { fail "固定 Origin CA 目标必须是普通文件: $source"; remove_sensitive_path "$target"; return 1; }
      copy_backup_source "$source" "$target/$name" \
        || { remove_sensitive_path "$target"; return 1; }
      : >"$marker" || { remove_sensitive_path "$target"; return 1; }
    fi
  done
  printf -v "$variable" '%s' "$target"
}

restore_upgrade_assets() {
  local source=$1 name target mode
  [[ -d "$source" ]] || return 1
  atomic_copy "$source/compose.yml" "$DEPLOY_DIR/compose.yml" 0644 || return 1
  atomic_copy "$source/Caddyfile" "$DEPLOY_DIR/Caddyfile" 0644 || return 1
  atomic_copy "$source/postgres_roles.sql" "$DEPLOY_DIR/postgres_roles.sql" 0600 || return 1
  atomic_copy "$source/cloudledger-ops.sh" "$DEPLOY_DIR/cloudledger-ops.sh" 0755 || return 1
  atomic_copy "$source/server.toml" "$SERVER_CONFIG" 0600 || return 1
  if [[ ${EUID:-$(id -u)} -eq 0 ]]; then chown 10001:10001 "$SERVER_CONFIG" || return 1; fi
  for name in origin-cert.pem origin-key.pem; do
    target="$CERT_DIR/$name"
    case "$name" in origin-cert.pem) mode=0644 ;; origin-key.pem) mode=0600 ;; esac
    if [[ -f "$source/had-$name" ]]; then
      atomic_copy "$source/$name" "$target" "$mode" || return 1
    else
      rm -f -- "$target" || return 1
    fi
  done
}

prepare_legacy_deployment_for_upgrade() {
  local tag=$1 owner old_cert old_key admin_path admin_token audit_key_id audit_hmac identifier_hmac target_client_version
  local turnstile_site turnstile_secret
  owner=${CLOUDLEDGER_SERVER_IMAGE#ghcr.io/}; owner=${owner%%/*}
  old_cert=$CLOUDLEDGER_CADDY_ORIGIN_CERT_PATH
  old_key=$CLOUDLEDGER_CADDY_ORIGIN_KEY_PATH
  server_config_value admin_path admin path \
    && server_config_value admin_token admin token \
    && server_config_value audit_key_id security.audit key_id \
    && server_config_value audit_hmac security.audit hmac_key \
    && server_config_value identifier_hmac security.audit identifier_hmac_key \
    && server_config_value turnstile_site security.turnstile site_key \
    && server_config_value turnstile_secret security.turnstile secret_key \
    || { fail '无法从旧 server.toml 唯一提取管理端、审计或 Turnstile 配置。'; return 1; }
  [[ "$turnstile_site" == "$CLOUDLEDGER_TURNSTILE_SITE_KEY" \
    && "$turnstile_secret" == "$CLOUDLEDGER_TURNSTILE_SECRET_KEY" ]] \
    || { fail '旧 server.toml 与 ops.env 的 Turnstile 配置不一致。'; return 1; }
  validate_server_config_values cloudledger "$CLOUDLEDGER_API_DOMAIN" "$CLOUDLEDGER_RUNTIME_DB_PASSWORD" \
    "$admin_path" "$admin_token" "$turnstile_site" "$turnstile_secret" "$audit_key_id" "$audit_hmac" \
    "$identifier_hmac" || return 1
  install_cert_pair_locked "$old_cert" "$old_key" || return 1
  set_env_value CLOUDLEDGER_GHCR_OWNER "$owner" || return 1
  set_env_value CLOUDLEDGER_RELEASE_TAG "$tag" || return 1
  target_client_version=$(client_version_for_tag "$tag")
  valid_client_version "$target_client_version" || { fail '目标 tag 无法转换为客户端 SemVer。'; return 1; }
  set_env_value CLOUDLEDGER_CLIENT_VERSION "$target_client_version" || return 1
  set_env_value CLOUDLEDGER_MIN_SUPPORTED_CLIENT_VERSION "$target_client_version" || return 1
  set_env_value CLOUDLEDGER_CLIENT_DOWNLOAD_URL 'https://github.com/dahai9/CloudLedger/releases/latest' || return 1
  set_env_value CLOUDLEDGER_SERVER_IMAGE "ghcr.io/$owner/cloudledger-server:$tag" || return 1
  set_env_value CLOUDLEDGER_POSTGRES_IMAGE "ghcr.io/$owner/cloudledger-postgres:$tag" || return 1
  set_env_value CLOUDLEDGER_CADDY_IMAGE "ghcr.io/$owner/cloudledger-caddy:$tag" || return 1
  set_env_value CLOUDLEDGER_ANCHOR_IMAGE "ghcr.io/$owner/cloudledger-network-anchor:$tag" || return 1
  set_env_value CLOUDLEDGER_ADMIN_PATH "$admin_path" || return 1
  set_env_value CLOUDLEDGER_ADMIN_TOKEN "$admin_token" || return 1
  set_env_value CLOUDLEDGER_AUDIT_KEY_ID "$audit_key_id" || return 1
  set_env_value CLOUDLEDGER_AUDIT_HMAC_KEY "$audit_hmac" || return 1
  set_env_value CLOUDLEDGER_AUDIT_IDENTIFIER_HMAC_KEY "$identifier_hmac" || return 1
  remove_env_value CLOUDLEDGER_HTTP_HOST_PORT || return 1
  remove_env_value CLOUDLEDGER_HTTPS_HOST_PORT || return 1
  remove_env_value CLOUDLEDGER_ADMIN_TUNNEL_PORT || return 1
  normalize_ops_env || return 1
  render_server_config || return 1
  stage_assets || return 1
  validate_deployment_config || return 1
  ok 'v0.1.3 配置已在回滚保护下规范化为当前四镜像部署模型。'
}

preserve_upgrade_snapshot() {
  local snapshot=$1 target
  ensure_dirs || return 1
  target="$STATE_DIR/upgrade-failed-old-ops-$(date -u +%Y%m%d-%H%M%S)-$$.env"
  if mv -- "$snapshot" "$target"; then
    chmod 600 "$target" 2>/dev/null || true
    unregister_sensitive_path "$snapshot"
    warn "迁移前配置快照已保留用于诊断: $target"
    return 0
  fi
  unregister_sensitive_path "$snapshot"
  warn "无法移动迁移前配置快照；已原地保留: $snapshot"
  return 1
}

preserve_upgrade_asset_snapshot() {
  local snapshot=$1 target
  [[ -d "$snapshot" ]] || return 0
  target="$STATE_DIR/upgrade-failed-old-assets-$(date -u +%Y%m%d-%H%M%S)-$$"
  if mv -- "$snapshot" "$target"; then
    chmod 700 "$target" 2>/dev/null || true
    unregister_sensitive_path "$snapshot"
    warn "迁移前部署资源快照已保留用于诊断: $target"
    return 0
  fi
  unregister_sensitive_path "$snapshot"
  warn "无法移动迁移前部署资源快照；已原地保留: $snapshot"
  return 1
}

handle_active_upgrade_abort() {
  local snapshot=$ACTIVE_UPGRADE_ENV_SNAPSHOT assets=$ACTIVE_UPGRADE_ASSET_SNAPSHOT
  local migration_started=$UPGRADE_MIGRATION_STARTED failed=0
  [[ -n "$snapshot" || -n "$assets" ]] || return 0
  ACTIVE_UPGRADE_ENV_SNAPSHOT=''
  ACTIVE_UPGRADE_ASSET_SNAPSHOT=''
  UPGRADE_MIGRATION_STARTED=0
  trap '' HUP INT TERM
  if (( migration_started == 0 )); then
    if [[ -n "$assets" ]] && ! restore_upgrade_assets "$assets"; then failed=1; fi
    if [[ -s "$snapshot" ]] && mv -f -- "$snapshot" "$OPS_ENV"; then
      unregister_sensitive_path "$snapshot"
    else
      unregister_sensitive_path "$snapshot"
      failed=1
    fi
    if (( failed == 0 )); then
      [[ -z "$assets" ]] || remove_sensitive_path "$assets" || failed=1
    fi
    if (( failed == 0 )); then
      warn '迁移尚未开始，已恢复旧镜像配置和部署资源。'
      compose up -d network-anchor postgres cloudledger caddy >/dev/null 2>&1 \
        || warn '旧镜像配置已恢复，但旧服务自动启动失败。'
    else
      [[ -z "$assets" ]] || preserve_upgrade_asset_snapshot "$assets" || true
      fail "迁移前升级中断且旧配置或部署资源自动恢复失败；请保护快照: $snapshot"
    fi
  else
    fail '迁移已经开始；禁止自动恢复旧镜像配置。'
    preserve_upgrade_snapshot "$snapshot" || true
    [[ -z "$assets" ]] || preserve_upgrade_asset_snapshot "$assets" || true
  fi
  install_cleanup_traps
}

upgrade_locked() {
  compose pull network-anchor postgres migration cloudledger caddy || return 10
  compose stop caddy cloudledger || return 10
  compose up -d network-anchor postgres || return 10
  wait_for_postgres && verify_database_roles || return 10
  UPGRADE_MIGRATION_STARTED=1
  run_migration || return 20
  verify_migrations_exact && harden_runtime_metadata_permissions && verify_audit || return 20
  compose up -d --no-deps cloudledger || return 20
  check_local_backend /health && check_local_backend /ready || return 20
  compose run --rm --no-deps caddy caddy validate --config /etc/caddy/Caddyfile || return 20
  compose up -d --no-deps caddy || return 20
  health_url '升级后 /health' "$(api_base_url)/health" || return 20
  health_url '升级后 /ready' "$(api_base_url)/ready" || return 20
  verify_turnstile || return 20
  internal_firewall_refresh || return 20
  install_systemd_units || return 20
  enable_base_timers || return 20
}

upgrade_transaction() {
  local tag=$1 new_server new_postgres new_caddy new_anchor snapshot asset_snapshot rc legacy=0 owner target_client_version
  local old_server old_postgres old_caddy old_anchor image allow_missing_client_version=0
  load_env || return 1
  if legacy_deployment_detected; then
    legacy=1
    validate_legacy_deployment || return 1
  else
    if [[ -z "${CLOUDLEDGER_CLIENT_VERSION:-}" || -z "${CLOUDLEDGER_MIN_SUPPORTED_CLIENT_VERSION:-}" \
      || -z "${CLOUDLEDGER_CLIENT_DOWNLOAD_URL:-}" ]]; then
      allow_missing_client_version=1
    fi
    validate_deployment_config "$allow_missing_client_version" || return 1
  fi
  old_server=${CLOUDLEDGER_SERVER_IMAGE:-}
  old_postgres=${CLOUDLEDGER_POSTGRES_IMAGE:-}
  old_caddy=${CLOUDLEDGER_CADDY_IMAGE:-}
  old_anchor=${CLOUDLEDGER_ANCHOR_IMAGE:-}
  if (( legacy == 1 )); then
    owner=${old_server#ghcr.io/}; owner=${owner%%/*}
    new_server="ghcr.io/$owner/cloudledger-server:$tag"
    new_postgres="ghcr.io/$owner/cloudledger-postgres:$tag"
    new_caddy="ghcr.io/$owner/cloudledger-caddy:$tag"
    new_anchor="ghcr.io/$owner/cloudledger-network-anchor:$tag"
  else
    for image in "$old_server" "$old_postgres" "$old_caddy" "$old_anchor"; do
      [[ -n "$image" && "$image" == *:* ]] \
        || { fail '当前镜像配置不完整，无法计算升级 tag。'; return 1; }
    done
    new_server="${old_server%:*}:$tag"
    new_postgres="${old_postgres%:*}:$tag"
    new_caddy="${old_caddy%:*}:$tag"
    new_anchor="${old_anchor%:*}:$tag"
  fi
  target_client_version=$(client_version_for_tag "$tag")
  valid_client_version "$target_client_version" || { fail '目标 tag 无法转换为客户端 SemVer。'; return 1; }
  log '检查四个目标 GHCR 镜像 manifest...'
  for image in "$new_server" "$new_postgres" "$new_caddy" "$new_anchor"; do
    docker manifest inspect "$image" >/dev/null \
      || { fail "目标镜像不存在或当前 GHCR 凭据无权读取: $image"; return 1; }
  done
  require_remote_backup_configuration || return 1
  snapshot=$(mktemp "${TMPDIR:-/tmp}/cloudledger-upgrade-env.XXXXXX")
  register_sensitive_path "$snapshot"
  chmod 600 "$snapshot" || { remove_sensitive_path "$snapshot"; return 1; }
  cp -- "$OPS_ENV" "$snapshot" || { remove_sensitive_path "$snapshot"; return 1; }
  snapshot_upgrade_assets asset_snapshot || { remove_sensitive_path "$snapshot"; return 1; }
  ACTIVE_UPGRADE_ENV_SNAPSHOT=$snapshot
  ACTIVE_UPGRADE_ASSET_SNAPSHOT=$asset_snapshot
  UPGRADE_MIGRATION_STARTED=0
  if (( legacy == 1 )); then
    create_legacy_upgrade_snapshot \
      && prepare_legacy_deployment_for_upgrade "$tag" \
      && make_backup \
      || { handle_active_upgrade_abort; return 1; }
  else
    { (( allow_missing_client_version == 0 )) || backfill_client_version_config; } \
      && make_backup \
      && set_env_value CLOUDLEDGER_SERVER_IMAGE "$new_server" \
      && set_env_value CLOUDLEDGER_POSTGRES_IMAGE "$new_postgres" \
      && set_env_value CLOUDLEDGER_CADDY_IMAGE "$new_caddy" \
      && set_env_value CLOUDLEDGER_ANCHOR_IMAGE "$new_anchor" \
      && set_env_value CLOUDLEDGER_RELEASE_TAG "$tag" \
      && set_env_value CLOUDLEDGER_CLIENT_VERSION "$target_client_version" \
      && set_env_value CLOUDLEDGER_MIN_SUPPORTED_CLIENT_VERSION "$target_client_version" \
      && set_env_value CLOUDLEDGER_CLIENT_DOWNLOAD_URL "${CLOUDLEDGER_CLIENT_DOWNLOAD_URL:-https://github.com/dahai9/CloudLedger/releases/latest}" \
      && render_server_config \
      || { handle_active_upgrade_abort; return 1; }
  fi
  if upgrade_locked; then
    ACTIVE_UPGRADE_ENV_SNAPSHOT=''
    ACTIVE_UPGRADE_ASSET_SNAPSHOT=''
    UPGRADE_MIGRATION_STARTED=0
    remove_sensitive_path "$snapshot" || return 1
    remove_sensitive_path "$asset_snapshot" || return 1
    printf '%s %s success\n' "$(date -u +%FT%TZ)" "$tag" >>"$STATE_DIR/upgrade.log"
    ok '升级完成，/health、/ready、Caddy 和审计链均已验证。'
    return 0
  else
    rc=$?
  fi
  if (( rc == 10 )); then
    handle_active_upgrade_abort
    printf '%s %s pre-migration-failure\n' "$(date -u +%FT%TZ)" "$tag" >>"$STATE_DIR/upgrade.log"
  else
    fail '迁移已经开始但升级后验证失败；禁止盲目降级，请从匹配的数据库、配置和审计密钥备份恢复。'
    printf '%s %s post-migration-failure\n' "$(date -u +%FT%TZ)" "$tag" >>"$STATE_DIR/upgrade.log"
    handle_active_upgrade_abort
  fi
  return 1
}

upgrade() {
  require_root
  local tag
  tag=$(choose_tag) || return 1
  press_confirm '升级将创建并验证备份、执行不可逆数据库迁移并重启入口，继续' || return 0
  with_lock upgrade_transaction "$tag"
}

harden_roles_transaction() {
  require_root
  load_env || return 1
  local current_user migration runtime bootstrap_password sql
  require_env_value CLOUDLEDGER_MIGRATION_DB_PASSWORD || return 1
  require_env_value CLOUDLEDGER_RUNTIME_DB_PASSWORD || return 1
  require_env_value CLOUDLEDGER_BOOTSTRAP_DB_PASSWORD || return 1
  require_remote_backup_configuration || return 1
  make_backup || return 1
  read -r -p '当前具备管理员权限的数据库账号 [cloudledger_bootstrap]: ' current_user
  current_user=${current_user:-cloudledger_bootstrap}
  [[ "$current_user" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]] || { fail '数据库账号格式不合法。'; return 1; }
  migration=$CLOUDLEDGER_MIGRATION_DB_PASSWORD
  runtime=$CLOUDLEDGER_RUNTIME_DB_PASSWORD
  bootstrap_password=$CLOUDLEDGER_BOOTSTRAP_DB_PASSWORD
  sql=$(mktemp "$STATE_DIR/.harden-roles.XXXXXX") || return 1
  register_sensitive_path "$sql"
  chmod 600 "$sql" || { remove_sensitive_path "$sql"; return 1; }
  cat >"$sql" <<'SQL' || { remove_sensitive_path "$sql"; return 1; }
SELECT format('CREATE ROLE cloudledger_bootstrap LOGIN SUPERUSER PASSWORD %L', :'bootstrap_password')
WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'cloudledger_bootstrap') \gexec
ALTER ROLE cloudledger_bootstrap LOGIN SUPERUSER CREATEDB CREATEROLE PASSWORD :'bootstrap_password';
ALTER DATABASE cloudledger OWNER TO cloudledger_bootstrap;
SQL
  cat "$DEPLOY_DIR/postgres_roles.sql" >>"$sql" || { remove_sensitive_path "$sql"; return 1; }
  if ! compose exec -T postgres psql --username "$current_user" --dbname cloudledger \
    --single-transaction --set ON_ERROR_STOP=1 --set database_name=cloudledger \
    --set "bootstrap_password=$bootstrap_password" --set "migration_password=$migration" \
    --set "runtime_password=$runtime" <"$sql"; then
    remove_sensitive_path "$sql"
    unset migration runtime bootstrap_password
    fail '数据库账号加固事务失败，数据库修改已回滚。'
    return 1
  fi
  if ! remove_sensitive_path "$sql"; then
    unset migration runtime bootstrap_password
    return 1
  fi
  unset migration runtime bootstrap_password
  verify_database_roles || return 1
}

harden_roles() {
  log '影响范围: 将先创建完整备份，再创建/验证 bootstrap 账号、迁移对象所有权并将 migration/runtime 降为非超级用户。'
  press_confirm '确认执行一次性账号加固' || return 0
  with_lock harden_roles_transaction
}

select_service() {
  local choice
  while :; do
    printf '%s\n' '1. CloudLedger 后端' '2. PostgreSQL' '3. Caddy' '4. Network Anchor' '0. 返回'
    read_choice choice '请选择: ' '^[0-4]$' || return 1
    case "$choice" in
      1) SELECTED_SERVICE=cloudledger; return 0 ;;
      2) SELECTED_SERVICE=postgres; return 0 ;;
      3) SELECTED_SERVICE=caddy; return 0 ;;
      4) SELECTED_SERVICE=network-anchor; return 0 ;;
      0) return 1 ;;
    esac
  done
}

service_action() {
  local action=$1
  select_service || return 0
  with_lock compose "$action" "$SELECTED_SERVICE"
}

service_menu() {
  local choice
  while :; do
    header
    printf '%s\n' '1. 查看所有服务状态' '2. 启动全部服务' '3. 停止全部服务' '4. 重启全部服务' \
      '5. 启动单个服务' '6. 停止单个服务' '7. 重启单个服务' '8. 查看容器详细信息' \
      '9. 查看当前镜像版本' '10. 拉取当前配置的镜像' '0. 返回上一级'
    read_choice choice '请选择: ' '^(0|[1-9]|10)$' || return
    case "$choice" in
      1) menu_action service_state; pause ;;
      2) menu_action with_lock compose up -d; pause ;;
      3) log '影响范围: CloudLedger 的后端、数据库、Caddy 和 network-anchor 将停止。'; if press_confirm '确认停止全部服务'; then menu_action with_lock compose down; fi; pause ;;
      4) if press_confirm '确认重启全部 CloudLedger 服务'; then menu_action with_lock compose restart; fi; pause ;;
      5) menu_action service_action start; pause ;;
      6) menu_action service_action stop; pause ;;
      7) menu_action service_action restart; pause ;;
      8) menu_action compose ps; menu_action show_image_versions; pause ;;
      9) menu_action show_image_versions; pause ;;
      10) menu_action with_lock compose pull; pause ;;
      0) return ;;
    esac
  done
}

backup_menu() {
  local choice id schedule retention archive
  while :; do
    header
    printf '%s\n' '1. 立即创建完整备份' '2. 查看 OneDrive 远程备份' '3. 查看某个备份详情' \
      '4. 校验指定备份' '5. 下载指定备份' '6. 恢复指定备份' '7. 执行临时数据库恢复演练' \
      '8. 查看最近一次备份结果' '9. 清理超过保留数量的备份' '10. 修改备份时间和保留数量' \
      '11. 测试备份失败告警' '0. 返回上一级'
    read_choice choice '请选择: ' '^(0|[1-9]|10|11)$' || return
    case "$choice" in
      1) menu_action with_lock make_backup; pause ;;
      2) load_env; [[ -n "${CLOUDLEDGER_RCLONE_REMOTE:-}" ]] && rclone lsf "$CLOUDLEDGER_RCLONE_REMOTE" || warn 'rclone remote 未配置。'; pause ;;
      3) read -r -p '输入备份文件名或编号: ' id; archive=$(backup_file "$id") && tar -tvf "$archive"; pause ;;
      4) read -r -p '输入备份文件名或编号: ' id; menu_action verify_backup "$id"; pause ;;
      5) read -r -p '输入远程备份文件名: ' id; menu_action with_lock download_remote_backup "$id"; pause ;;
      6) read -r -p '输入备份文件名或编号: ' id; [[ -n "$id" ]] && menu_action restore_backup "$id"; pause ;;
      7) menu_action with_lock internal_restore_test; pause ;;
      8) archive=$(current_backup); [[ -n "$archive" ]] && { stat -c '%y %s bytes %n' "$archive"; verify_backup "$archive"; } || warn '暂无备份。'; pause ;;
      9) log '影响范围: 仅删除超过配置保留数量的本地旧备份，新备份和远端备份不受影响。'; press_confirm '确认清理旧备份' && prune_backups; pause ;;
      10)
        read -r -p '每日备份时间（systemd OnCalendar，例如 *-*-* 03:00:00）: ' schedule
        systemd-analyze calendar "$schedule" >/dev/null 2>&1 || { fail 'OnCalendar 格式无效。'; pause; continue; }
        read -r -p '本地保留数量 [30]: ' retention
        retention=${retention:-30}
        [[ "$retention" =~ ^[1-9][0-9]*$ ]] || { fail '保留数量无效。'; pause; continue; }
        require_root
        sed -i "s#^OnCalendar=.*#OnCalendar=$schedule#" "$SYSTEMD_INSTALL_DIR/cloudledger-ops-backup.timer"
        set_env_value CLOUDLEDGER_BACKUP_RETENTION "$retention"
        systemctl daemon-reload
        pause
        ;;
      11) menu_action test_backup_alert; pause ;;
      0) return ;;
    esac
  done
}

test_backup_alert() {
  load_env || return 1
  [[ -n "${CLOUDLEDGER_WEBHOOK_URL:-}" ]] || { warn '未配置 webhook。'; return 1; }
  curl -fsS -X POST -H 'Content-Type: application/json' -d '{"event":"backup_failure_test","service":"cloudledger"}' "$CLOUDLEDGER_WEBHOOK_URL" >/dev/null
  ok '备份失败测试告警已发送。'
}

query_releases() {
  curl -fsS 'https://api.github.com/repos/dahai9/CloudLedger/releases?per_page=30' | jq -r '.[].tag_name'
}

migration_menu() {
  local choice
  while :; do
    header
    printf '%s\n' '1. 查看当前版本' '2. 查询可用 GitHub Release 版本' '3. 升级到指定版本' \
      '4. 检查数据库迁移状态' '5. 手动执行数据库迁移' '6. 验证审计链' '7. 查看最近一次升级记录' \
      '8. 迁移前回滚' '9. 查看升级失败现场' '10. 加固已有数据库账号权限' '0. 返回上一级'
    read_choice choice '请选择: ' '^(0|[1-9]|10)$' || return
    case "$choice" in
      1) load_env; printf '配置 tag: %s\n' "${CLOUDLEDGER_RELEASE_TAG:-未知}"; show_image_versions; pause ;;
      2) query_releases || warn '无法查询 GitHub Releases。'; pause ;;
      3) menu_action upgrade; pause ;;
      4) menu_action migration_status; pause ;;
      5) if press_confirm '确认使用专用 migration profile 执行迁移'; then menu_action with_lock run_migration; fi; pause ;;
      6) menu_action verify_audit; pause ;;
      7) [[ -s "$STATE_DIR/upgrade.log" ]] && tail -n 50 "$STATE_DIR/upgrade.log" || warn '暂无升级记录。'; pause ;;
      8) warn '迁移前失败会自动恢复旧镜像；迁移开始后必须从匹配的数据库和配置备份恢复。'; pause ;;
      9) [[ -s "$STATE_DIR/upgrade.log" ]] && tail -n 100 "$STATE_DIR/upgrade.log" || warn '暂无失败现场。'; compose logs --tail=100 migration cloudledger postgres 2>/dev/null || true; pause ;;
      10) menu_action harden_roles; pause ;;
      0) return ;;
    esac
  done
}

realtime_monitor() {
  local stop=0
  trap 'stop=1' INT
  while (( stop == 0 )); do
    clear 2>/dev/null || true
    header
    uptime
    command_exists free && free -h
    df -h "$STATE_DIR"
    service_state
    compose stats --no-stream 2>/dev/null || true
    sleep 5
  done
  trap 'cleanup_plaintext; exit 130' INT
}

monitor_menu() {
  local choice mem_used mem_total mem_pct api
  while :; do
    header
    printf '%s\n' '1. 查看综合状态面板' '2. 进入实时监控模式' '3. 查看 CPU 和内存使用率' \
      '4. 查看磁盘和 Docker 卷占用' '5. 查看容器资源压力' '6. 查看 PostgreSQL 连接和数据库大小' \
      '7. 检查 API /health' '8. 检查 API /ready' '9. 检查 Caddy 和 HTTPS' \
      '10. 检查 Origin CA 证书有效期' '11. 执行完整健康检查' '12. 查看最近健康告警' '0. 返回上一级'
    read_choice choice '请选择: ' '^(0|[1-9]|10|11|12)$' || return
    api=$(api_base_url)
    case "$choice" in
      1) menu_action verify_deploy; menu_action service_state; menu_action firewall_status; pause ;;
      2) realtime_monitor ;;
      3)
        command_exists free && free -h
        mem_used=$(free | awk '/Mem:/ {print $3}'); mem_total=$(free | awk '/Mem:/ {print $2}')
        if [[ "$mem_total" =~ ^[0-9]+$ && "$mem_total" -gt 0 ]]; then
          mem_pct=$((mem_used * 100 / mem_total)); printf '内存使用率: %s%%\n' "$mem_pct"
          (( mem_pct >= ${CLOUDLEDGER_MEMORY_WARN:-85} )) && warn "内存达到警告阈值 ${CLOUDLEDGER_MEMORY_WARN:-85}%"
        fi
        uptime; pause
        ;;
      4) menu_action df -h; menu_action docker system df; pause ;;
      5) menu_action compose stats --no-stream; pause ;;
      6) menu_action postgres_psql cloudledger -c "SELECT count(*) AS connections FROM pg_stat_activity; SELECT pg_size_pretty(pg_database_size('cloudledger')) AS database_size;"; menu_action docker volume ls; pause ;;
      7) menu_action health_url 'API /health' "$api/health"; pause ;;
      8) menu_action health_url 'API /ready' "$api/ready"; pause ;;
      9) menu_action compose run --rm --no-deps caddy caddy validate --config /etc/caddy/Caddyfile; menu_action health_url 'HTTPS' "$api/health"; pause ;;
      10) menu_action certificate_status; pause ;;
      11) menu_action internal_health; menu_action certificate_status; menu_action firewall_status; pause ;;
      12) [[ -s "$STATE_DIR/health.log" ]] && tail -n 100 "$STATE_DIR/health.log" || warn '暂无健康告警。'; pause ;;
      0) return ;;
    esac
  done
}

show_domain_config() {
  load_env
  printf 'API 域名: %s\nHTTP 发布: %s\nHTTPS 发布: %s\n管理端: 127.0.0.1:8788（仅 SSH 隧道）\n' \
    "${CLOUDLEDGER_API_DOMAIN:-未配置}" "${CLOUDLEDGER_HTTP_PUBLISH:-未配置}" "${CLOUDLEDGER_HTTPS_PUBLISH:-未配置}"
}

show_certificate_info() {
  local cert
  cert=$(certificate_file)
  [[ -f "$cert" ]] || { warn '未找到证书。'; return 1; }
  openssl x509 -in "$cert" -noout -subject -issuer -dates -ext subjectAltName
}

update_domain_configuration() {
  set_domains
  render_server_config
}

check_configured_certificate_domain() {
  local cert
  load_env || return 1
  require_env_value CLOUDLEDGER_API_DOMAIN || return 1
  cert=$(certificate_file)
  [[ -f "$cert" ]] || { fail '未找到 Origin CA 证书。'; return 1; }
  openssl x509 -checkhost "$CLOUDLEDGER_API_DOMAIN" -noout -in "$cert"
}

cloudflare_menu() {
  local choice
  while :; do
    header
    printf '%s\n' '1. 查看当前域名配置' '2. 修改 API 域名' '3. 查看管理端 SSH 隧道配置' \
      '4. 导入 Origin CA 证书和私钥' '5. 查看证书信息' '6. 检查证书覆盖的域名' \
      '7. 检查证书有效期' '8. 验证 Caddyfile' '9. 重载 Caddy' '10. 检查 Cloudflare 代理访问' \
      '11. 刷新并应用 Cloudflare-only 防火墙' '0. 返回上一级'
    read_choice choice '请选择: ' '^(0|[1-9]|10|11)$' || return
    case "$choice" in
      1) show_domain_config; pause ;;
      2) menu_action update_domain_configuration; pause ;;
      3) log '使用 ssh -N -L 8788:127.0.0.1:8788 <SSH主机>，再访问本机 127.0.0.1:8788。'; pause ;;
      4) menu_action install_cert; pause ;;
      5) menu_action show_certificate_info; pause ;;
      6) menu_action check_configured_certificate_domain; pause ;;
      7) menu_action certificate_status; pause ;;
      8) menu_action compose run --rm --no-deps caddy caddy validate --config /etc/caddy/Caddyfile; pause ;;
      9) menu_action compose exec -T caddy caddy reload --config /etc/caddy/Caddyfile; pause ;;
      10) menu_action health_url 'Cloudflare API 域名' "$(api_base_url)/health"; pause ;;
      11) menu_action with_lock internal_firewall_refresh; pause ;;
      0) return ;;
    esac
  done
}

rclone_menu() {
  local choice remote
  while :; do
    header
    printf '%s\n' '1. 检查 rclone 是否安装' '2. 创建 OneDrive remote' '3. 创建 rclone crypt remote' \
      '4. 查看当前远程配置' '5. 测试远程连接' '6. 测试上传和下载' '7. 查看远程空间使用情况' \
      '8. 查看 CloudLedger 备份目录' '9. 修改远程目录' '10. 重新配置 rclone' '11. 显示灾难恢复所需信息' '0. 返回上一级'
    read_choice choice '请选择: ' '^(0|[1-9]|10|11)$' || return
    load_env
    remote=${CLOUDLEDGER_RCLONE_REMOTE:-}
    case "$choice" in
      1) menu_action rclone version; pause ;;
      2|3|10) require_root; warn 'crypt 密码必须在服务器外保存。'; menu_action rclone config --config "$RCLONE_CONFIG"; chmod 600 "$RCLONE_CONFIG" 2>/dev/null || true; pause ;;
      4) if [[ -s "$RCLONE_CONFIG" ]]; then sed -E 's/^([[:space:]]*(pass(word)?2?|token|client_secret|access_token|refresh_token)[[:space:]]*=).*/\1 <已隐藏>/I' "$RCLONE_CONFIG"; else warn '没有 rclone 配置。'; fi; pause ;;
      5) [[ -n "$remote" ]] && validate_rclone_crypt_remote "$remote" && rclone lsd "$(rclone_remote_name "$remote"):" || fail '远程连接未配置。'; pause ;;
      6) menu_action rclone_test_transfer; pause ;;
      7) [[ -n "$remote" ]] && rclone about "$(rclone_remote_name "$remote"):" || fail '远程连接未配置。'; pause ;;
      8) [[ -n "$remote" ]] && rclone lsf "$remote" || fail '远程目录未配置。'; pause ;;
      9) read -r -p 'crypt 远程备份目录: ' remote; validate_rclone_crypt_remote "$remote" && rclone mkdir "$remote" && set_env_value CLOUDLEDGER_RCLONE_REMOTE "$remote"; pause ;;
      11) log '灾难恢复需要：OneDrive remote、服务器外保存的 crypt 密码、完整备份包、GHCR tag；crypt 密码不会进入备份。'; pause ;;
      0) return ;;
    esac
  done
}

rotate_database_passwords_transaction() {
  local migration=$1 runtime=$2 snapshot
  snapshot=$(mktemp "${TMPDIR:-/tmp}/cloudledger-password-env.XXXXXX") || return 1
  register_sensitive_path "$snapshot"
  chmod 600 "$snapshot" || { remove_sensitive_path "$snapshot"; return 1; }
  cp -- "$OPS_ENV" "$snapshot" || { remove_sensitive_path "$snapshot"; return 1; }
  if ! set_env_value CLOUDLEDGER_MIGRATION_DB_PASSWORD "$migration" \
    || ! set_env_value CLOUDLEDGER_RUNTIME_DB_PASSWORD "$runtime" \
    || ! set_env_value CLOUDLEDGER_MIGRATION_DATABASE_URL "postgres://cloudledger_migration:${migration}@127.0.0.1:5432/cloudledger"; then
    mv -f -- "$snapshot" "$OPS_ENV" || true
    unregister_sensitive_path "$snapshot"
    fail '数据库密码配置写入失败，ops.env 已恢复。'
    return 1
  fi
  if ! compose exec -T postgres psql --username cloudledger_bootstrap --dbname cloudledger \
    --single-transaction --set ON_ERROR_STOP=1 --set database_name=cloudledger \
    --set "migration_password=$migration" --set "runtime_password=$runtime" <"$DEPLOY_DIR/postgres_roles.sql"; then
    mv -f -- "$snapshot" "$OPS_ENV"
    unregister_sensitive_path "$snapshot"
    fail '数据库角色密码更新失败，ops.env 已恢复。'
    return 1
  fi
  if ! render_server_config || ! compose restart cloudledger || ! verify_database_roles; then
    remove_sensitive_path "$snapshot"
    fail '数据库密码已更新，但后端验证失败；请使用当前 ops.env 诊断，不能恢复旧密码文件。'
    return 1
  fi
  remove_sensitive_path "$snapshot" || return 1
}

rotate_database_passwords() {
  local migration runtime
  log '影响范围: migration/runtime 数据库登录与 server.toml 将同步更新，后端会重启。'
  press_confirm '确认轮换数据库密码' || return 0
  hidden_read migration '新的 migration 密码（隐藏）: '
  hidden_read runtime '新的 runtime 密码（隐藏）: '
  [[ "$migration" =~ ^[A-Za-z0-9_-]{16,128}$ && "$runtime" =~ ^[A-Za-z0-9_-]{16,128}$ ]] \
    || { unset migration runtime; fail '密码必须为 16-128 位字母、数字、下划线或连字符。'; return 1; }
  with_lock rotate_database_passwords_transaction "$migration" "$runtime"
  unset migration runtime
}

redact_config() {
  sed -E \
    -e 's#^([^#]*(PASSWORD|TOKEN|SECRET|KEY|PAT|HMAC|ADMIN_PATH|WEBHOOK_URL)[^=]*)=.*#\1=<已隐藏>#I' \
    -e 's#(postgres(ql)?://[^:]+:)[^@]+@#\1<已隐藏>@#g'
}

check_permissions() {
  local failed=0 path mode owner
  for path in "$OPS_ENV" "$SERVER_CONFIG" "$RCLONE_CONFIG" "$(certificate_key_file)"; do
    [[ -e "$path" ]] || { warn "缺少文件: $path"; failed=1; continue; }
    mode=$(stat -c '%a' "$path")
    [[ "$mode" == 600 ]] || { warn "权限应为 600: $mode $path"; failed=1; }
  done
  if [[ -e "$SERVER_CONFIG" ]]; then
    owner=$(stat -c '%u:%g' "$SERVER_CONFIG")
    [[ "$owner" == '10001:10001' || ${CLOUDLEDGER_ALLOW_NONROOT:-0} == 1 ]] \
      || { warn "server.toml 所有者应为 10001:10001: $owner"; failed=1; }
  fi
  [[ "$failed" -eq 0 ]] && ok '敏感文件权限检查通过。'
  return "$failed"
}

fix_permissions() {
  require_root
  local failed=0 path cert key
  ensure_dirs || return 1
  mkdir -p "$(dirname "$OPS_ENV")" "$CERT_DIR" || return 1
  cert=$(certificate_file) || return 1
  key=$(certificate_key_file) || return 1
  for path in "$STATE_DIR" "$BACKUP_DIR" "$(dirname "$OPS_ENV")" "$CERT_DIR"; do
    chmod 700 "$path" || { fail "无法修复目录权限: $path"; failed=1; }
  done
  for path in "$OPS_ENV" "$SERVER_CONFIG" "$RCLONE_CONFIG" "$key"; do
    [[ -f "$path" && ! -L "$path" ]] \
      || { fail "无法修复缺失或非普通敏感文件: $path"; failed=1; continue; }
    chmod 600 "$path" || { fail "无法修复敏感文件权限: $path"; failed=1; }
  done
  for path in "$cert" "$COMPOSE_FILE" "$DEPLOY_DIR/Caddyfile"; do
    [[ -f "$path" && ! -L "$path" ]] \
      || { fail "无法修复缺失或非普通部署文件: $path"; failed=1; continue; }
    chmod 644 "$path" || { fail "无法修复部署文件权限: $path"; failed=1; }
  done
  if [[ ${EUID:-$(id -u)} -eq 0 ]]; then
    chown 10001:10001 "$SERVER_CONFIG" || { fail '无法修复 server.toml 所有者。'; failed=1; }
  fi
  (( failed == 0 )) || return 1
  check_permissions || return 1
  ok '文件权限已修复并复核通过。'
}

diagnostic_export() {
  require_root
  ensure_dirs || return 1
  local target=${1:-$STATE_DIR/diagnostic-$(date +%Y%m%d-%H%M%S).txt}
  {
    printf 'CloudLedger diagnostic %s\n' "$(date -u +%FT%TZ)"
    uname -a
    service_state
    df -h "$STATE_DIR"
    [[ -r "$OPS_ENV" ]] && redact_config <"$OPS_ENV"
    [[ -r "$SERVER_CONFIG" ]] && redact_config <"$SERVER_CONFIG"
  } >"$target"
  chmod 600 "$target"
  if grep -Eiq '(password|token|secret|hmac_key|admin_path|webhook_url)[[:space:]]*=[[:space:]]*[^<[:space:]]' "$target"; then
    rm -f -- "$target"
    fail '诊断报告脱敏自检失败，已删除报告。'
    return 1
  fi
  ok "已导出脱敏诊断报告: $target"
}

update_turnstile_configuration() {
  configure_turnstile
  render_server_config
  compose restart cloudledger
  verify_turnstile
}

regenerate_deployment_configuration() {
  render_server_config
  normalize_ops_env
  validate_deployment_config
}

config_menu() {
  local choice value secret
  while :; do
    header
    printf '%s\n' '1. 查看脱敏后的全部配置' '2. 修改 GHCR 镜像仓库' '3. 修改数据库密码' \
      '4. 修改 Turnstile 配置' '5. 修改 webhook 告警地址' '6. 修改资源告警阈值' \
      '7. 检查文件权限' '8. 修复文件权限' '9. 验证 PostgreSQL 角色权限' '10. 加固迁移账号权限' \
      '11. 重新生成部署配置' '12. 导出脱敏诊断配置' '0. 返回上一级'
    read_choice choice '请选择: ' '^(0|[1-9]|10|11|12)$' || return
    case "$choice" in
      1) [[ -r "$OPS_ENV" ]] && redact_config <"$OPS_ENV"; [[ -r "$SERVER_CONFIG" ]] && redact_config <"$SERVER_CONFIG"; pause ;;
      2) menu_action configure_images; pause ;;
      3) menu_action rotate_database_passwords; pause ;;
      4) menu_action update_turnstile_configuration; pause ;;
      5) hidden_read secret 'Webhook URL（隐藏，留空禁用）: '; set_env_value CLOUDLEDGER_WEBHOOK_URL "$secret"; unset secret; pause ;;
      6)
        read -r -p '磁盘警告阈值 [80]: ' value; [[ "${value:-80}" =~ ^[1-9][0-9]?$ ]] && set_env_value CLOUDLEDGER_DISK_WARN "${value:-80}" || fail '阈值无效。'
        read -r -p '磁盘严重阈值 [90]: ' value; [[ "${value:-90}" =~ ^[1-9][0-9]?$ ]] && set_env_value CLOUDLEDGER_DISK_CRITICAL "${value:-90}" || fail '阈值无效。'
        read -r -p '内存警告阈值 [85]: ' value; [[ "${value:-85}" =~ ^[1-9][0-9]?$ ]] && set_env_value CLOUDLEDGER_MEMORY_WARN "${value:-85}" || fail '阈值无效。'
        pause
        ;;
      7) menu_action check_permissions; pause ;;
      8) menu_action fix_permissions; pause ;;
      9) menu_action verify_database_roles; pause ;;
      10) menu_action harden_roles; pause ;;
      11) menu_action regenerate_deployment_configuration; pause ;;
      12) menu_action diagnostic_export; pause ;;
      0) return ;;
    esac
  done
}

logs_menu() {
  local choice service
  while :; do
    header
    printf '%s\n' '1. 查看后端实时日志' '2. 查看 PostgreSQL 实时日志' '3. 查看 Caddy 实时日志' \
      '4. 查看全部服务日志' '5. 查看最近错误日志' '6. 查看备份任务日志' '7. 查看健康检查日志' \
      '8. 执行完整环境诊断' '9. 检查端口占用' '10. 检查 Docker 网络' '11. 检查 DNS 和 HTTPS' \
      '12. 导出脱敏诊断报告' '0. 返回上一级'
    read_choice choice '请选择: ' '^(0|[1-9]|10|11|12)$' || return
    case "$choice" in
      1|2|3) [[ "$choice" == 1 ]] && service=cloudledger; [[ "$choice" == 2 ]] && service=postgres; [[ "$choice" == 3 ]] && service=caddy; follow_logs "$service"; pause ;;
      4) follow_logs; pause ;;
      5) compose logs --tail=300 | grep -iE 'error|fatal|panic|failed' || true; pause ;;
      6) [[ -s "$STATE_DIR/backup.log" ]] && tail -n 100 "$STATE_DIR/backup.log" || warn '暂无备份任务日志。'; pause ;;
      7) [[ -s "$STATE_DIR/health.log" ]] && tail -n 100 "$STATE_DIR/health.log" || warn '暂无健康检查日志。'; pause ;;
      8) check_requirements || true; validate_deployment_config || true; verify_deploy || true; firewall_status || true; pause ;;
      9) command_exists ss && ss -lntp || netstat -lntp; pause ;;
      10) docker network ls; docker inspect "$(compose ps -q network-anchor)" --format '{{json .NetworkSettings.Networks}}'; pause ;;
      11) load_env; command_exists dig && dig "$CLOUDLEDGER_API_DOMAIN" || true; curl -I --max-time 10 "https://$CLOUDLEDGER_API_DOMAIN" || true; pause ;;
      12) menu_action diagnostic_export; pause ;;
      0) return ;;
    esac
  done
}

follow_logs() {
  local interrupted=0 rc=0
  trap 'interrupted=1' INT
  compose logs --tail=100 --follow "$@" || rc=$?
  install_cleanup_traps
  if (( interrupted == 1 || rc == 130 )); then return 0; fi
  return "$rc"
}

run_scheduled_task_menu() {
  local choice
  printf '%s\n' '1. 每日备份' '2. 健康检查' '3. 每周恢复演练' '4. Cloudflare 防火墙刷新' '0. 返回'
  read_choice choice '请选择: ' '^[0-4]$' || return
  case "$choice" in
    1) menu_action internal_backup ;;
    2) menu_action internal_health ;;
    3) menu_action with_lock internal_restore_test ;;
    4) menu_action with_lock internal_firewall_refresh ;;
    0) return ;;
  esac
}

schedule_menu() {
  local choice interval
  while :; do
    header
    printf '%s\n' '1. 查看全部定时任务' '2. 启用每日备份' '3. 禁用每日备份' '4. 修改每日备份时间' \
      '5. 启用健康检查' '6. 禁用健康检查' '7. 修改健康检查频率' '8. 启用每周恢复演练' \
      '9. 禁用每周恢复演练' '10. 立即执行一次定时任务' '11. 查看下次运行时间' '0. 返回上一级'
    read_choice choice '请选择: ' '^(0|[1-9]|10|11)$' || return
    case "$choice" in
      1|11) menu_action systemctl list-timers 'cloudledger-ops-*' --all; pause ;;
      2) require_root; menu_action systemctl enable --now cloudledger-ops-backup.timer; pause ;;
      3) require_root; menu_action systemctl disable --now cloudledger-ops-backup.timer; pause ;;
      4) read -r -p '每日备份时间（OnCalendar）: ' interval; systemd-analyze calendar "$interval" >/dev/null && sed -i "s#^OnCalendar=.*#OnCalendar=$interval#" "$SYSTEMD_INSTALL_DIR/cloudledger-ops-backup.timer" && systemctl daemon-reload; pause ;;
      5) require_root; menu_action systemctl enable --now cloudledger-ops-health.timer; pause ;;
      6) require_root; menu_action systemctl disable --now cloudledger-ops-health.timer; pause ;;
      7) read -r -p '健康检查间隔（例如 5m）: ' interval; [[ "$interval" =~ ^[1-9][0-9]*(s|m|h|d)$ ]] && sed -i "s#^OnUnitActiveSec=.*#OnUnitActiveSec=$interval#" "$SYSTEMD_INSTALL_DIR/cloudledger-ops-health.timer" && systemctl daemon-reload || fail '间隔格式无效。'; pause ;;
      8) require_root; menu_action systemctl enable --now cloudledger-ops-restore-test.timer; pause ;;
      9) require_root; menu_action systemctl disable --now cloudledger-ops-restore-test.timer; pause ;;
      10) run_scheduled_task_menu; pause ;;
      0) return ;;
    esac
  done
}

internal_backup() {
  ensure_dirs || return 1
  local rc=0
  with_lock make_backup >>"$STATE_DIR/backup.log" 2>&1 || rc=$?
  return "$rc"
}

internal_health() {
  ensure_dirs || return 1
  local api failed=0
  api=$(api_base_url)
  {
    date -u +%FT%TZ
    health_url 'API /health' "$api/health" || failed=1
    health_url 'API /ready' "$api/ready" || failed=1
    certificate_status || failed=1
    firewall_status || failed=1
  } >>"$STATE_DIR/health.log" 2>&1
  return "$failed"
}

internal_dispatch() {
  case ${1:-} in
    backup) internal_backup ;;
    health) internal_health ;;
    restore-test) with_lock internal_restore_test ;;
    firewall-refresh) with_lock internal_firewall_refresh ;;
    *) fail '未知内部任务。'; return 2 ;;
  esac
}

about_menu() {
  local choice
  while :; do
    header
    printf '%s\n' '1. 关于 CloudLedger' '2. 查看环境信息' '0. 返回上一级'
    read_choice choice '请选择: ' '^[0-2]$' || return
    case "$choice" in
      1) log "CloudLedger 运维工具箱 $(current_version)"; log '公开入口仅数字菜单；生产镜像固定 GHCR tag；备份必须是可校验 pg_dump -Fc。'; pause ;;
      2) uname -a; docker --version 2>/dev/null || true; docker compose version 2>/dev/null || true; rclone version 2>/dev/null | head -n1 || true; pause ;;
      0) return ;;
    esac
  done
}

first_install_menu() {
  local choice
  while :; do
    header
    printf '%s\n' '1. 检查服务器是否满足部署要求' '2. 自动安装 Docker、Compose 和辅助工具' \
      '3. 配置 GitHub Container Registry' '4. 选择要部署的 CloudLedger 版本' '5. 生成数据库账号和密码' \
      '6. 生成后端 server.toml' '7. 导入 Cloudflare Origin CA 证书' '8. 配置 API 域名和管理端本地监听' \
      '9. 执行完整首次部署' '10. 验证首次部署结果' '11. 执行全部安装向导' '0. 返回上一级'
    read_choice choice '请选择: ' '^(0|[1-9]|10|11)$' || return
    case "$choice" in
      1) check_requirements || true; pause ;;
      2) menu_action install_dependencies; pause ;;
      3) menu_action configure_registry_access; pause ;;
      4) menu_action configure_images; pause ;;
      5) menu_action generate_passwords; pause ;;
      6) menu_action render_server_config; pause ;;
      7) menu_action install_cert; pause ;;
      8) menu_action set_domains; menu_action configure_turnstile; pause ;;
      9) menu_action first_deploy; pause ;;
      10) menu_action verify_deploy; pause ;;
      11) menu_action install_wizard; pause ;;
      0) return ;;
    esac
  done
}

main_menu() {
  local choice
  while :; do
    if [[ -t 1 && -z "${NO_COLOR:-}" ]]; then clear 2>/dev/null || true; fi
    header
    printf '%s\n' '1. 首次安装与部署' '2. 服务管理' '3. 版本升级与数据库迁移' \
      '4. 数据备份与恢复' '5. 服务监控与压力查看' '6. Cloudflare 与 HTTPS 证书' \
      '7. OneDrive / rclone 管理' '8. 系统配置与安全管理' '9. 日志与故障诊断' \
      '10. 定时任务管理' '11. 关于与环境信息' '0. 退出'
    read_choice choice '请选择: ' '^(0|[1-9]|10|11)$' || return
    case "$choice" in
      1) first_install_menu ;;
      2) service_menu ;;
      3) migration_menu ;;
      4) backup_menu ;;
      5) monitor_menu ;;
      6) cloudflare_menu ;;
      7) rclone_menu ;;
      8) config_menu ;;
      9) logs_menu ;;
      10) schedule_menu ;;
      11) about_menu ;;
      0) return ;;
    esac
  done
}

if [[ ${1:-} == --internal ]]; then
  [[ $# -eq 2 ]] || { fail '内部任务参数无效。'; exit 2; }
  require_root
  internal_dispatch "$2"
  exit $?
fi
[[ $# -eq 0 ]] || die '公开入口只支持数字交互菜单；未知参数已拒绝。'
ensure_dirs || exit 1
set +e
main_menu
exit 0
