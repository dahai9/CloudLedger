#!/bin/sh
set -eu

: "${CLOUDLEDGER_RUNTIME_DB_PASSWORD:?CLOUDLEDGER_RUNTIME_DB_PASSWORD is required}"

psql --username "$POSTGRES_USER" --dbname "$POSTGRES_DB" \
  --set "database_name=$POSTGRES_DB" \
  --set "migration_password=$POSTGRES_PASSWORD" \
  --set "runtime_password=$CLOUDLEDGER_RUNTIME_DB_PASSWORD" \
  --file /docker-entrypoint-initdb.d/10-postgres_roles.sql
