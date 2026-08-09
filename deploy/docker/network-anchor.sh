#!/bin/sh
set -eu

relay_pid=''

shutdown() {
  if [ -n "$relay_pid" ]; then
    kill "$relay_pid" 2>/dev/null || true
    wait "$relay_pid" 2>/dev/null || true
  fi
  exit 0
}

trap shutdown TERM INT

socat TCP-LISTEN:18788,bind=0.0.0.0,reuseaddr,fork TCP:127.0.0.1:8788 &
relay_pid=$!
wait "$relay_pid"
