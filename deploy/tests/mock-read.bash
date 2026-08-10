# Sourced through BASH_ENV by deploy/tests/cloudledger-ops-test.sh.
# It intentionally affects only the operations script, not its mock children.
[[ ${0##*/} == cloudledger-ops.sh ]] || return 0

exec 8<"${CLOUDLEDGER_TEST_MENU_ANSWERS:?CLOUDLEDGER_TEST_MENU_ANSWERS is required}"

read() {
  local prompt='' variable='' answer='' arg
  local -a original=("$@")
  while (($#)); do
    arg=$1
    shift
    case $arg in
      -p)
        prompt=${1:-}
        shift || true
        ;;
      -r|-s)
        ;;
      -*)
        ;;
      *)
        variable=$arg
        ;;
    esac
  done
  [[ -n "$variable" ]] || variable=REPLY
  if [[ -z "$prompt" ]]; then
    builtin read "${original[@]}"
    return $?
  fi
  if [[ -n ${CLOUDLEDGER_TEST_READ_TRACE:-} ]]; then
    printf '%s\n' "$prompt" >>"$CLOUDLEDGER_TEST_READ_TRACE"
  fi

  case $prompt in
    *请选择*|*选择菜单*|*菜单编号*)
      if ! IFS= builtin read -r answer <&8; then
        return 1
      fi
      ;;
    *registry*|*Registry*) answer=${CLOUDLEDGER_TEST_REGISTRY:-ghcr.io} ;;
    *GitHub*用户名*|*GHCR*用户*|*镜像所有者*|*仓库所有者*) answer=${CLOUDLEDGER_TEST_GHCR_OWNER:-cloudledger} ;;
    *PAT*|*GitHub*令牌*|*GHCR*密码*) answer=${CLOUDLEDGER_TEST_GHCR_PAT:-test-ghcr-pat} ;;
    *tag*|*Tag*|*版本*) answer=${CLOUDLEDGER_TEST_TAG:-v0.1.5} ;;
    *API*域名*) answer=${CLOUDLEDGER_TEST_API_DOMAIN:-cloudledger-test.513921.xyz} ;;
    *管理端*域名*|*Admin*域名*) answer=${CLOUDLEDGER_TEST_ADMIN_DOMAIN:-127.0.0.1} ;;
    *site*key*|*Site*Key*|*站点密钥*) answer=${CLOUDLEDGER_TEST_TURNSTILE_SITE_KEY:-test-turnstile-site-key} ;;
    *Turnstile*secret*|*Turnstile*Secret*|*Turnstile*私钥*) answer=${CLOUDLEDGER_TEST_TURNSTILE_SECRET:-test-turnstile-secret} ;;
    *Origin*证书*|*证书路径*) answer=${CLOUDLEDGER_TEST_CERT_FILE:?certificate fixture is required} ;;
    *Origin*私钥*|*私钥路径*) answer=${CLOUDLEDGER_TEST_KEY_FILE:?private-key fixture is required} ;;
    *OneDrive*remote*|*OneDrive*名称*) answer=${CLOUDLEDGER_TEST_ONEDRIVE_REMOTE:-onedrive} ;;
    *crypt*remote*|*crypt*名称*) answer=${CLOUDLEDGER_TEST_CRYPT_REMOTE:-cloudledger-crypt} ;;
    *crypt*远程*目录*) answer=${CLOUDLEDGER_TEST_CRYPT_PATH:-cloudledger-crypt:CloudLedger/backups} ;;
    *远程*目录*|*备份目录*) answer=${CLOUDLEDGER_TEST_REMOTE_PATH:-CloudLedger/backups} ;;
    *crypt*密码*|*rclone*密码*) answer=${CLOUDLEDGER_TEST_RCLONE_PASSWORD:-test-rclone-password} ;;
    *YES*|*确认*|*继续执行*) answer=YES ;;
    *) answer='' ;;
  esac

  printf -v "$variable" '%s' "$answer"
  return 0
}
