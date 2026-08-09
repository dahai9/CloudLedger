#!/usr/bin/env bash
set -Eeuo pipefail

name=${0##*/}
trace=${CLOUDLEDGER_TEST_TRACE:?CLOUDLEDGER_TEST_TRACE is required}
remote_dir=${CLOUDLEDGER_TEST_REMOTE_DIR:-}

record() {
  printf '%s\n' "$1" >>"$trace"
}

has_arg() {
  local expected=$1 arg
  shift
  for arg in "$@"; do
    [[ "$arg" == "$expected" ]] && return 0
  done
  return 1
}

emit_dump() {
  case ${CLOUDLEDGER_TEST_PG_DUMP_MODE:-ok} in
    ok)
      printf 'PGDMPcloudledger-test-dump\n'
      ;;
    empty)
      return 0
      ;;
    fail)
      return 1
      ;;
    partial-fail)
      printf 'PGDMPpartial-cloudledger-test-dump\n'
      return 1
      ;;
    *)
      printf 'unknown CLOUDLEDGER_TEST_PG_DUMP_MODE\n' >&2
      return 2
      ;;
  esac
}

remote_path() {
  local spec=$1 relative=${1#*:}
  [[ "$spec" == *:* && -n "$remote_dir" ]] || return 1
  printf '%s/%s' "$remote_dir" "${relative#/}"
}

emit_nft_table() {
  local mode=${CLOUDLEDGER_TEST_NFT_TABLE_MODE:-valid}
  printf '%s\n' 'table inet cloudledger_origin {'
  if [[ "$mode" != missing-sets ]]; then
    printf '%s\n' \
      '  set cloudflare_ipv4 { type ipv4_addr; flags interval; elements = { 173.245.48.0/20 }; }' \
      '  set cloudflare_ipv6 { type ipv6_addr; flags interval; elements = { 2400:cb00::/32 }; }'
  fi
  if [[ "$mode" != missing-input ]]; then
    printf '%s\n' '  chain input {'
    if [[ "$mode" == non-hook ]]; then
      printf '%s\n' '    type filter hook output priority -10; policy accept;'
    else
      printf '%s\n' '    type filter hook input priority -10; policy accept;'
    fi
    [[ "$mode" == extra-accept ]] && printf '%s\n' '    tcp dport 443 accept'
    printf '%s\n' \
      '    iifname "lo" tcp dport 443 accept' \
      '    ip saddr @cloudflare_ipv4 tcp dport 443 accept' \
      '    ip6 saddr @cloudflare_ipv6 tcp dport 443 accept'
    [[ "$mode" != missing-reject ]] && printf '%s\n' '    tcp dport 443 reject with tcp reset'
    printf '%s\n' '  }'
  fi
  if [[ "$mode" != missing-forward ]]; then
    printf '%s\n' '  chain forward {' \
      '    type filter hook forward priority -10; policy accept;'
    if [[ "$mode" == missing-bridge ]]; then
      printf '%s\n' \
        '    oifname "wrong0" ip saddr @cloudflare_ipv4 tcp dport 443 accept' \
        '    oifname "wrong0" ip6 saddr @cloudflare_ipv6 tcp dport 443 accept'
    else
      printf '%s\n' \
        '    oifname "cld-origin0" ip saddr @cloudflare_ipv4 tcp dport 443 accept' \
        '    oifname "cld-origin0" ip6 saddr @cloudflare_ipv6 tcp dport 443 accept'
    fi
    [[ "$mode" != missing-reject ]] && printf '%s\n' '    oifname "cld-origin0" tcp dport 443 reject with tcp reset'
    printf '%s\n' '  }'
  fi
  if [[ "$mode" == unauthorized-chain ]]; then
    printf '%s\n' '  chain bypass {' '    type filter hook input priority -20; policy accept;' '    tcp dport 443 accept' '  }'
  fi
  printf '%s\n' '}'
}

case $name in
  docker)
    if [[ ${1:-} == login ]]; then
      record 'docker:login'
      dd of=/dev/null bs=4096 status=none || true
      exit 0
    fi

    if [[ ${1:-} == compose ]]; then
      joined=" $* "
      if [[ "$joined" == *' config '* && ${CLOUDLEDGER_TEST_ASSERT_CLEAN_COMPOSE_ENV:-0} == 1 ]]; then
        [[ -z ${CLOUDLEDGER_RUNTIME_DB_PASSWORD+x} && -z ${CLOUDLEDGER_SERVER_IMAGE+x} ]] || exit 88
        record 'compose:clean-env'
      fi
      if [[ "$joined" == *' ps -q network-anchor '* ]]; then
        [[ -n ${CLOUDLEDGER_TEST_ANCHOR_ID:-} ]] && printf '%s\n' "$CLOUDLEDGER_TEST_ANCHOR_ID"
        exit 0
      fi
      if [[ "$joined" == *' exec '*postgres*' psql '* ]]; then
        if [[ "$joined" == *'FROM pg_roles'* ]]; then
          printf '%s\n' \
            'cloudledger_bootstrap:true:true:true:false:false' \
            'cloudledger_migration:false:false:false:false:false' \
            'cloudledger_runtime:false:false:false:false:false'
          record 'postgres:roles'
        elif [[ "$joined" == *"has_table_privilege('cloudledger_runtime', '_sqlx_migrations'"* ]]; then
          printf 'true:false:false:false:false\n'
          record 'postgres:runtime-migration-permissions'
        elif [[ "$joined" == *'string_agg(version'* ]]; then
          printf '%s\n' "${CLOUDLEDGER_TEST_MIGRATION_SUMMARY:-1,2,3,4,5|5|true}"
          record 'postgres:migrations-exact'
        elif [[ "$joined" == *"count(*) FROM _sqlx_migrations"* ]]; then
          printf '5\n'
          record 'restore:migrations'
        elif [[ "$joined" == *to_regclass* ]]; then
          table=$(printf '%s' "$joined" | sed -n "s/.*public\.\([A-Za-z0-9_]*\).*/\1/p")
          printf 'public.%s\n' "$table"
          record "restore:table:$table"
        else
          record 'postgres:query'
        fi
        exit 0
      fi
      if [[ "$joined" == *' exec '*postgres*createdb* ]]; then
        [[ "$joined" == *'--maintenance-db postgres'* ]] || exit 1
        record 'restore:createdb'
        exit 0
      fi
      if [[ "$joined" == *' exec '*postgres*dropdb* ]]; then
        [[ "$joined" == *'--if-exists --force --maintenance-db postgres'* ]] || exit 1
        record 'restore:dropdb'
        drop_count_file=${CLOUDLEDGER_TEST_DROPDB_COUNT_FILE:-}
        drop_count=0
        [[ -n "$drop_count_file" && -f "$drop_count_file" ]] && drop_count=$(<"$drop_count_file")
        drop_count=$((drop_count + 1))
        [[ -n "$drop_count_file" ]] && printf '%s\n' "$drop_count" >"$drop_count_file"
        if [[ ${CLOUDLEDGER_TEST_DROPDB_MODE:-ok} == fail-cleanup && "$drop_count" -ge 2 ]]; then exit 1; fi
        exit 0
      fi
      if [[ "$joined" == *' exec '*postgres*pg_restore* ]]; then
        if [[ "$joined" == *'--list'* ]]; then
          count_file=${CLOUDLEDGER_TEST_PG_RESTORE_COUNT_FILE:-}
          count=0
          [[ -n "$count_file" && -f "$count_file" ]] && count=$(<"$count_file")
          count=$((count + 1))
          [[ -n "$count_file" ]] && printf '%s\n' "$count" >"$count_file"
          record 'backup:pg-restore-list'
          if [[ ${CLOUDLEDGER_TEST_PG_RESTORE_MODE:-ok} == fail-verify && "$count" -ge 2 ]]; then
            exit 1
          fi
        else
          [[ "$joined" != *'--clean'* && "$joined" == *'--single-transaction --exit-on-error'* ]] || exit 1
          record 'restore:pg-restore'
          apply_count_file=${CLOUDLEDGER_TEST_PG_RESTORE_APPLY_COUNT_FILE:-}
          apply_count=0
          [[ -n "$apply_count_file" && -f "$apply_count_file" ]] && apply_count=$(<"$apply_count_file")
          apply_count=$((apply_count + 1))
          [[ -n "$apply_count_file" ]] && printf '%s\n' "$apply_count" >"$apply_count_file"
          if [[ ${CLOUDLEDGER_TEST_SIGNAL_ON_PG_RESTORE:-0} == 1 && "$apply_count" -eq 1 ]]; then
            kill -TERM "$PPID"
          fi
        fi
        [[ ${CLOUDLEDGER_TEST_PG_RESTORE_MODE:-ok} == fail ]] && exit 1
        exit 0
      fi
      if [[ "$joined" == *' ps '*'--status running'*'--services '* ]]; then
        printf '%s\n' network-anchor postgres cloudledger admin-relay caddy
      elif [[ "$joined" == *' ps '* ]]; then
        printf 'cloudledger-test running healthy\n'
      fi

      if [[ "$joined" == *' pull '* ]]; then
        record 'compose:pull'
        [[ ${CLOUDLEDGER_TEST_SIGNAL_ON_COMPOSE:-} == pull ]] && kill -TERM "$PPID"
        [[ ${CLOUDLEDGER_TEST_COMPOSE_FAIL_AT:-} == pull ]] && exit 1
      fi
      if [[ "$joined" == *' run '*migration* ]]; then
        if [[ "$joined" == *'audit verify'* ]]; then
          record 'compose:audit'
          if [[ "$joined" == *restore-test.toml* ]]; then
            test_config=''
            for arg in "$@"; do
              case "$arg" in *:/etc/cloudledger/restore-test.toml:ro) test_config=${arg%%:*} ;; esac
            done
            [[ -n "$test_config" && -f "$test_config" ]] || exit 1
            grep -Eq '^url = "postgres://cloudledger_runtime:[A-Za-z0-9_-]+@127\.0\.0\.1:5432/cloudledger_restore_test_[0-9]+_[0-9]+"$' "$test_config" \
              || exit 1
            record 'restore:audit'
            record 'restore:audit-temp-database'
          fi
        else
          record 'compose:migration'
          [[ ${CLOUDLEDGER_TEST_SIGNAL_ON_COMPOSE:-} == migration ]] && kill -TERM "$PPID"
          [[ ${CLOUDLEDGER_TEST_COMPOSE_FAIL_AT:-} == migration ]] && exit 1
        fi
      fi
      if [[ "$joined" == *' exec '*postgres*pg_dump* ]]; then
        record 'backup:docker-pg-dump'
        emit_dump
        exit $?
      fi
      if [[ "$joined" == *' exec '*network-anchor*wget*'/health'* ]]; then
        record 'http:local-health'
        exit 0
      fi
      if [[ "$joined" == *' exec '*network-anchor*wget*'/ready'* ]]; then
        record 'http:local-ready'
        exit 0
      fi
      if [[ "$joined" == *' up '* ]]; then
        if [[ "$joined" == *' up -d network-anchor postgres '* ]]; then
          record 'compose:up:database'
        elif [[ "$joined" == *' up -d --no-deps cloudledger '* ]]; then
          record 'compose:up:backend'
        elif [[ "$joined" == *' up -d --no-deps caddy '* ]]; then
          record 'compose:up:caddy'
        else
          record 'compose:up:other'
        fi
      fi
      exit 0
    fi

    if [[ ${1:-} == image && ${2:-} == inspect ]]; then
      record 'docker:image-inspect'
      exit 0
    fi
    if [[ ${1:-} == manifest && ${2:-} == inspect ]]; then
      record 'docker:manifest-inspect'
      exit 0
    fi
    if [[ ${1:-} == network && ${2:-} == inspect ]]; then
      record 'docker:network-inspect'
      printf '%s\n' cld-origin0
      exit 0
    fi
    if [[ ${1:-} == inspect ]]; then
      record 'docker:inspect'
      printf '%s\n' healthy
      exit 0
    fi
    if [[ ${1:-} == info ]]; then
      exit 0
    fi
    if [[ ${1:-} == ps ]]; then
      if [[ " $* " == *' publish=443 '* && ${CLOUDLEDGER_TEST_HTTPS_DOCKER_OWNER:-} == other ]]; then
        printf 'other443\tunrelated-https\n'
      fi
      exit 0
    fi
    exit 0
    ;;

  curl)
    url=''
    output_file=''
    previous=''
    for arg in "$@"; do
      if [[ "$previous" == -o || "$previous" == --output ]]; then
        output_file=$arg
      elif [[ "$arg" == http://* || "$arg" == https://* ]]; then
        url=$arg
      fi
      previous=$arg
    done
    response=''
    case $url in
      *cloudflare.com/ips-v4*)
        record 'cloudflare:ipv4'
        if [[ ${CLOUDLEDGER_TEST_CLOUDFLARE_IP_MODE:-valid} == invalid ]]; then
          response='not-an-ip'
        else
          response=$'173.245.48.0/20\n103.21.244.0/22'
        fi
        ;;
      *cloudflare.com/ips-v6*)
        record 'cloudflare:ipv6'
        if [[ ${CLOUDLEDGER_TEST_CLOUDFLARE_IP_MODE:-valid} == invalid ]]; then
          response='also-not-an-ip'
        else
          response=$'2400:cb00::/32\n2606:4700::/32'
        fi
        ;;
      *siteverify*)
        record 'turnstile:probe'
        if [[ ${CLOUDLEDGER_TEST_TURNSTILE_PROBE_MODE:-valid-secret} == invalid-secret ]]; then
          response='{"success":false,"error-codes":["invalid-input-secret"]}'
        else
          response='{"success":false,"error-codes":["invalid-input-response"]}'
        fi
        ;;
      *api.github.com*/releases*)
        record 'github:releases'
        response='[{"tag_name":"v0.1.4"}]'
        ;;
      */auth/security)
        record 'turnstile:status'
        response='{"turnstileEnabled":true,"turnstileSiteKey":"test-turnstile-site-key"}'
        ;;
      */ready)
        record 'http:ready'
        [[ "$url" == https://* ]] && record 'http:https'
        ;;
      */health)
        record 'http:health'
        [[ "$url" == https://* ]] && record 'http:https'
        ;;
      *)
        record 'http:request'
        ;;
    esac
    if [[ -n "$output_file" ]]; then
      printf '%s\n' "$response" >"$output_file"
    elif [[ -n "$response" ]]; then
      printf '%s\n' "$response"
    fi
    exit 0
    ;;

  rclone)
    [[ ${CLOUDLEDGER_TEST_RCLONE_MODE:-ok} == fail ]] && {
      record 'rclone:failure'
      exit 1
    }
    case ${1:-} in
      copyto)
        src=${2:?source required}
        dst=${3:?destination required}
        if [[ "$dst" == *:* ]]; then
          target=$(remote_path "$dst")
          mkdir -p "$(dirname "$target")"
          cp -- "$src" "$target"
          record 'rclone:upload'
          if [[ ${CLOUDLEDGER_TEST_SIGNAL_ON_RCLONE:-} == upload ]]; then kill -TERM "$PPID"; fi
        else
          source_file=$(remote_path "$src")
          mkdir -p "$(dirname "$dst")"
          cp -- "$source_file" "$dst"
          record 'rclone:download'
        fi
        ;;
      moveto)
        source_file=$(remote_path "${2:?source required}")
        target=$(remote_path "${3:?destination required}")
        mkdir -p "$(dirname "$target")"
        mv -f -- "$source_file" "$target"
        record 'rclone:publish'
        ;;
      copy)
        src=${2:?source required}
        dst=${3:?destination required}
        if [[ "$dst" == *:* ]]; then
          target=$(remote_path "$dst")
          mkdir -p "$target"
          cp -- "$src" "$target/"
          record 'rclone:upload'
        else
          source_file=$(remote_path "$src")
          mkdir -p "$dst"
          cp -- "$source_file" "$dst/"
          record 'rclone:download'
        fi
        ;;
      config)
        record 'rclone:config'
        if [[ " $* " == *' show '* ]]; then
          printf '%s\n' '[cloudledger-crypt]' 'type = crypt' 'remote = onedrive:CloudLedger'
        fi
        ;;
      listremotes)
        printf '%s\n' onedrive: cloudledger-crypt:
        ;;
      lsf|lsd)
        record 'rclone:list'
        [[ -d "$remote_dir" ]] && find "$remote_dir" -type f -printf '%f\n'
        ;;
      about)
        printf '{"total":1000000,"used":1000,"free":999000}\n'
        ;;
      deletefile)
        record 'rclone:delete'
        target=$(remote_path "${2:?remote path required}")
        rm -f -- "$target"
        ;;
      delete|purge)
        record 'rclone:delete'
        ;;
      version)
        printf 'rclone v1.test\n'
        ;;
      *)
        record 'rclone:other'
        ;;
    esac
    ;;

  nft)
    nft_state=${CLOUDLEDGER_TEST_NFT_STATE:-}
    if [[ ${1:-} == list && ${2:-} == table ]]; then
      record 'firewall:inspect'
      if [[ -n "$nft_state" && -f "$nft_state" ]]; then
        emit_nft_table
        exit 0
      fi
      exit 1
    fi
    if [[ ${1:-} == list && ${2:-} == set ]]; then
      record 'firewall:inspect-set'
      if [[ -n "$nft_state" && -f "$nft_state" && ${CLOUDLEDGER_TEST_NFT_TABLE_MODE:-valid} != missing-sets ]]; then
        case ${5:-} in
          cloudflare_ipv4) printf '%s\n' 'set cloudflare_ipv4 { elements = { 173.245.48.0/20 } }' ;;
          cloudflare_ipv6) printf '%s\n' 'set cloudflare_ipv6 { elements = { 2400:cb00::/32 } }' ;;
          *) exit 1 ;;
        esac
        exit 0
      fi
      exit 1
    fi
    if has_arg --check "$@" || has_arg -c "$@"; then
      record 'firewall:check'
      for arg in "$@"; do
        if [[ -f "$arg" ]] && grep -Eq 'not-an-ip|also-not-an-ip' "$arg"; then
          exit 1
        fi
      done
      exit 0
    fi
    if has_arg -f "$@" || has_arg --file "$@"; then
      record 'firewall:apply'
      [[ -n "$nft_state" ]] && : >"$nft_state"
      exit 0
    fi
    record 'firewall:inspect'
    exit 0
    ;;

  systemctl)
    case ${1:-} in
      is-active)
        printf 'active\n'
        ;;
      is-enabled)
        printf 'enabled\n'
        ;;
      enable)
        record 'systemd:enable'
        for arg in "$@"; do
          case $arg in
            cloudledger-ops-*.timer) record "systemd:timer:$arg" ;;
          esac
        done
        ;;
      daemon-reload)
        record 'systemd:daemon-reload'
        ;;
      *)
        record 'systemd:other'
        ;;
    esac
    ;;

  pg_dump)
    record 'backup:host-pg-dump'
    target=''
    for arg in "$@"; do
      case $arg in --file=*) target=${arg#--file=} ;; esac
    done
    if [[ -n "$target" ]]; then
      emit_dump >"$target"
    else
      emit_dump
    fi
    ;;

  pg_restore)
    record 'restore:pg-restore'
    [[ ${CLOUDLEDGER_TEST_PG_RESTORE_MODE:-ok} == fail ]] && exit 1
    ;;

  createdb)
    record 'restore:createdb'
    ;;

  dropdb)
    record 'restore:dropdb'
    ;;

  rm)
    for arg in "$@"; do
      if [[ -n ${CLOUDLEDGER_TEST_RM_FAIL_PATTERN:-} && "$arg" == *"$CLOUDLEDGER_TEST_RM_FAIL_PATTERN"* ]]; then
        record 'rm:injected-failure'
        printf '%s\n' "$arg" >>"${CLOUDLEDGER_TEST_RM_FAILED_PATH_FILE:?rm failure path file is required}"
        exit 1
      fi
    done
    exec "${CLOUDLEDGER_TEST_REAL_RM:?real rm path is required}" "$@"
    ;;

  psql)
    joined=" $* "
    if [[ "$joined" == *to_regclass* ]]; then
      table=$(printf '%s' "$joined" | sed -n "s/.*public\.\([A-Za-z0-9_]*\).*/\1/p")
      printf 'public.%s\n' "$table"
      record "restore:table:$table"
    elif [[ "$joined" == *'_sqlx_migrations'* ]]; then
      printf '5\n'
      record 'restore:migrations'
    elif [[ "$joined" == *audit* ]]; then
      record 'restore:audit'
    else
      record 'postgres:query'
    fi
    ;;

  ps)
    printf 'systemd\n'
    ;;

  ss)
    [[ -n ${CLOUDLEDGER_TEST_HTTPS_LISTENER:-} ]] && printf '%s\n' "$CLOUDLEDGER_TEST_HTTPS_LISTENER"
    record 'ss:ok'
    ;;

  caddy|dig|systemd-analyze)
    record "$name:ok"
    ;;

  apt-get|dnf|yum)
    record 'packages:install'
    ;;

  *)
    printf 'unsupported mock command: %s\n' "$name" >&2
    exit 127
    ;;
esac
