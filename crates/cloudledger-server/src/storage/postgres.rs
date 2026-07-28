use std::{future::Future, path::Path, pin::Pin, time::Duration};

use cloudledger_core::{AuditLog, FinancialAccount, Ledger, Membership, Money, Organization, User};
use cloudledger_service::{AppLedgerService, AppLedgerSnapshot};
use serde::{de::DeserializeOwned, Serialize};
use sqlx::{postgres::PgPoolOptions, PgPool, Postgres, Row, Transaction as PgTransaction};
use uuid::Uuid;

use crate::{
    auth::{AuthService, AuthSnapshot, StoredSession, StoredUser},
    config::DatabaseConfig,
};

use super::migrations;

#[derive(Debug, Clone)]
pub struct PostgresStore {
    pool: PgPool,
}

impl PostgresStore {
    pub async fn connect(config: &DatabaseConfig) -> anyhow::Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .acquire_timeout(Duration::from_secs(config.connect_timeout_seconds))
            .connect(&config.url)
            .await
            .map_err(|error| anyhow::anyhow!("connect to PostgreSQL: {error}"))?;
        migrations::migrate(&pool)
            .await
            .map_err(|error| anyhow::anyhow!("migrate PostgreSQL schema: {error}"))?;
        Ok(Self { pool })
    }

    pub async fn load_or_import(
        &self,
        ledger_state_path: &Path,
        auth_state_path: &Path,
    ) -> anyhow::Result<(AppLedgerService, AuthService, bool)> {
        if self.has_state().await? {
            return Ok((self.load_ledger().await?, self.load_auth().await?, false));
        }

        let imported_legacy = ledger_state_path.exists() || auth_state_path.exists();
        let ledger = if ledger_state_path.exists() {
            AppLedgerService::load_from_path(ledger_state_path)?
        } else {
            AppLedgerService::uninitialized()
        };
        let auth = AuthService::load_or_default(auth_state_path)?;
        self.save_all(ledger.snapshot(), auth.snapshot()).await?;
        Ok((ledger, auth, imported_legacy))
    }

    pub(crate) async fn save_ledger(&self, snapshot: AppLedgerSnapshot) -> anyhow::Result<()> {
        let mut transaction = self.pool.begin().await?;
        replace_ledger(&mut transaction, &snapshot).await?;
        transaction.commit().await?;
        Ok(())
    }

    pub(crate) async fn save_auth(&self, snapshot: AuthSnapshot) -> anyhow::Result<()> {
        let mut transaction = self.pool.begin().await?;
        replace_auth(&mut transaction, &snapshot).await?;
        transaction.commit().await?;
        Ok(())
    }

    pub(crate) async fn save_all(
        &self,
        ledger: AppLedgerSnapshot,
        auth: AuthSnapshot,
    ) -> anyhow::Result<()> {
        let mut transaction = self.pool.begin().await?;
        replace_ledger(&mut transaction, &ledger).await?;
        replace_auth(&mut transaction, &auth).await?;
        transaction.commit().await?;
        Ok(())
    }

    async fn has_state(&self) -> anyhow::Result<bool> {
        Ok(sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM app_metadata WHERE singleton_id = 1)",
        )
        .fetch_one(&self.pool)
        .await?)
    }

    async fn load_ledger(&self) -> anyhow::Result<AppLedgerService> {
        let metadata = sqlx::query(
            "SELECT schema_version, current_user_id FROM app_metadata WHERE singleton_id = 1",
        )
        .fetch_one(&self.pool)
        .await?;
        let schema_version = metadata.try_get::<i32, _>("schema_version")? as u32;
        let current_user_id = metadata
            .try_get::<Option<Uuid>, _>("current_user_id")?
            .unwrap_or_else(Uuid::nil);

        let users = sqlx::query(
            "SELECT id, display_name, email, phone, created_at FROM domain_users ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| {
            Ok(User {
                id: row.try_get("id")?,
                display_name: row.try_get("display_name")?,
                email: row.try_get("email")?,
                phone: row.try_get("phone")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
        let organizations =
            sqlx::query("SELECT id, name, created_by, created_at FROM organizations ORDER BY id")
                .fetch_all(&self.pool)
                .await?
                .into_iter()
                .map(|row| {
                    Ok(Organization {
                        id: row.try_get("id")?,
                        name: row.try_get("name")?,
                        created_by: row.try_get("created_by")?,
                        created_at: row.try_get("created_at")?,
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
        let memberships = sqlx::query(
            "SELECT id, organization_id, user_id, role, created_at FROM organization_memberships ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| {
            Ok(Membership {
                id: row.try_get("id")?,
                organization_id: row.try_get("organization_id")?,
                user_id: row.try_get("user_id")?,
                role: enum_from_db(&row.try_get::<String, _>("role")?)?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
        let ledgers = sqlx::query(
            "SELECT id, name, kind, owner_user_id, organization_id, created_at, deleted_at FROM ledgers ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| {
            Ok(Ledger {
                id: row.try_get("id")?,
                name: row.try_get("name")?,
                kind: enum_from_db(&row.try_get::<String, _>("kind")?)?,
                owner_user_id: row.try_get("owner_user_id")?,
                organization_id: row.try_get("organization_id")?,
                created_at: row.try_get("created_at")?,
                deleted_at: row.try_get("deleted_at")?,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
        let accounts = sqlx::query(
            "SELECT id, ledger_id, name, kind, opening_balance_minor, currency, created_at, deleted_at FROM financial_accounts ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| {
            Ok(FinancialAccount {
                id: row.try_get("id")?,
                ledger_id: row.try_get("ledger_id")?,
                name: row.try_get("name")?,
                kind: enum_from_db(&row.try_get::<String, _>("kind")?)?,
                opening_balance: Money::new(
                    row.try_get("opening_balance_minor")?,
                    row.try_get::<String, _>("currency")?,
                )?,
                created_at: row.try_get("created_at")?,
                deleted_at: row.try_get("deleted_at")?,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
        let transactions = sqlx::query(
            "SELECT id, ledger_id, account_id, category_id, kind, amount_minor, currency, occurred_at, description, approval_state, payment_state, created_by, submitted_by, approved_by, approved_at, paid_by, paid_at, received_by, received_at, version, created_at, updated_at, deleted_at FROM transactions ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| {
            Ok(cloudledger_core::Transaction {
                id: row.try_get("id")?,
                ledger_id: row.try_get("ledger_id")?,
                account_id: row.try_get("account_id")?,
                category_id: row.try_get("category_id")?,
                kind: enum_from_db(&row.try_get::<String, _>("kind")?)?,
                amount: Money::new(
                    row.try_get("amount_minor")?,
                    row.try_get::<String, _>("currency")?,
                )?,
                occurred_at: row.try_get("occurred_at")?,
                description: row.try_get("description")?,
                approval_state: enum_from_db(&row.try_get::<String, _>("approval_state")?)?,
                payment_state: enum_from_db(&row.try_get::<String, _>("payment_state")?)?,
                created_by: row.try_get("created_by")?,
                submitted_by: row.try_get("submitted_by")?,
                approved_by: row.try_get("approved_by")?,
                approved_at: row.try_get("approved_at")?,
                paid_by: row.try_get("paid_by")?,
                paid_at: row.try_get("paid_at")?,
                received_by: row.try_get("received_by")?,
                received_at: row.try_get("received_at")?,
                version: row.try_get("version")?,
                created_at: row.try_get("created_at")?,
                updated_at: row.try_get("updated_at")?,
                deleted_at: row.try_get("deleted_at")?,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
        let audit_logs = sqlx::query(
            "SELECT id, organization_id, ledger_id, actor_user_id, action, resource_type, resource_id, summary, created_at FROM audit_logs ORDER BY created_at, id",
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| {
            Ok(AuditLog {
                id: row.try_get("id")?,
                organization_id: row.try_get("organization_id")?,
                ledger_id: row.try_get("ledger_id")?,
                actor_user_id: row.try_get("actor_user_id")?,
                action: row.try_get("action")?,
                resource_type: row.try_get("resource_type")?,
                resource_id: row.try_get("resource_id")?,
                summary: row.try_get("summary")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

        Ok(AppLedgerService::from_snapshot(AppLedgerSnapshot {
            schema_version,
            current_user_id,
            users,
            organizations,
            memberships,
            ledgers,
            accounts,
            transactions,
            audit_logs,
        }))
    }

    async fn load_auth(&self) -> anyhow::Result<AuthService> {
        let users = sqlx::query(
            "SELECT id, display_name, email, phone, password_hash, account_kind, organization_id, created_at FROM auth_users ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| {
            Ok(StoredUser {
                id: row.try_get("id")?,
                display_name: row.try_get("display_name")?,
                email: row.try_get("email")?,
                phone: row.try_get("phone")?,
                password_hash: row.try_get("password_hash")?,
                account_kind: enum_from_db(&row.try_get::<String, _>("account_kind")?)?,
                organization_id: row.try_get("organization_id")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
        let installations = sqlx::query(
            "SELECT installation_id, user_id FROM auth_installations ORDER BY installation_id",
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| Ok((row.try_get("installation_id")?, row.try_get("user_id")?)))
        .collect::<anyhow::Result<Vec<_>>>()?;
        let sessions = sqlx::query(
            "SELECT access_token, user_id, installation_id, refresh_token, kind, created_at, refreshed_at FROM auth_sessions ORDER BY access_token",
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| {
            Ok(StoredSession {
                user_id: row.try_get("user_id")?,
                installation_id: row
                    .try_get::<Option<String>, _>("installation_id")?
                    .unwrap_or_default(),
                access_token: row.try_get("access_token")?,
                refresh_token: row
                    .try_get::<Option<String>, _>("refresh_token")?
                    .unwrap_or_default(),
                kind: enum_from_db(&row.try_get::<String, _>("kind")?)?,
                created_at: row.try_get("created_at")?,
                refreshed_at: row.try_get("refreshed_at")?,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(AuthService::from_snapshot(AuthSnapshot {
            users,
            installations,
            sessions,
        }))
    }
}

fn replace_ledger<'a, 'connection>(
    transaction: &'a mut PgTransaction<'connection, Postgres>,
    snapshot: &'a AppLedgerSnapshot,
) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>
where
    'connection: 'a,
{
    Box::pin(async move {
        sqlx::Executor::execute(
            &mut **transaction,
            sqlx::raw_sql(
                "DELETE FROM audit_logs; DELETE FROM transactions; DELETE FROM financial_accounts; DELETE FROM ledgers; DELETE FROM organization_memberships; DELETE FROM organizations; DELETE FROM domain_users; DELETE FROM app_metadata;",
            ),
        )
        .await?;

        for user in &snapshot.users {
            sqlx::Executor::execute(
                &mut **transaction,
                sqlx::query("INSERT INTO domain_users (id, display_name, email, phone, created_at) VALUES ($1, $2, $3, $4, $5)")
                    .bind(user.id)
                    .bind(&user.display_name)
                    .bind(&user.email)
                    .bind(&user.phone)
                    .bind(user.created_at),
            )
            .await?;
        }
        for organization in &snapshot.organizations {
            sqlx::Executor::execute(
                &mut **transaction,
                sqlx::query(
                    "INSERT INTO organizations (id, name, created_by, created_at) VALUES ($1, $2, $3, $4)",
                )
                .bind(organization.id)
                .bind(&organization.name)
                .bind(organization.created_by)
                .bind(organization.created_at),
            )
            .await?;
        }
        for membership in &snapshot.memberships {
            sqlx::Executor::execute(
                &mut **transaction,
                sqlx::query("INSERT INTO organization_memberships (id, organization_id, user_id, role, created_at) VALUES ($1, $2, $3, $4, $5)")
                    .bind(membership.id)
                    .bind(membership.organization_id)
                    .bind(membership.user_id)
                    .bind(enum_to_db(membership.role)?)
                    .bind(membership.created_at),
            )
            .await?;
        }
        for ledger in &snapshot.ledgers {
            sqlx::Executor::execute(
                &mut **transaction,
                sqlx::query("INSERT INTO ledgers (id, name, kind, owner_user_id, organization_id, created_at, deleted_at) VALUES ($1, $2, $3, $4, $5, $6, $7)")
                    .bind(ledger.id)
                    .bind(&ledger.name)
                    .bind(enum_to_db(ledger.kind)?)
                    .bind(ledger.owner_user_id)
                    .bind(ledger.organization_id)
                    .bind(ledger.created_at)
                    .bind(ledger.deleted_at),
            )
            .await?;
        }
        for account in &snapshot.accounts {
            sqlx::Executor::execute(
                &mut **transaction,
                sqlx::query("INSERT INTO financial_accounts (id, ledger_id, name, kind, opening_balance_minor, currency, created_at, deleted_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)")
                    .bind(account.id)
                    .bind(account.ledger_id)
                    .bind(&account.name)
                    .bind(enum_to_db(account.kind)?)
                    .bind(account.opening_balance.amount_minor)
                    .bind(&account.opening_balance.currency)
                    .bind(account.created_at)
                    .bind(account.deleted_at),
            )
            .await?;
        }
        for entry in &snapshot.transactions {
            sqlx::Executor::execute(
                &mut **transaction,
                sqlx::query("INSERT INTO transactions (id, ledger_id, account_id, category_id, kind, amount_minor, currency, occurred_at, description, approval_state, payment_state, created_by, submitted_by, approved_by, approved_at, paid_by, paid_at, received_by, received_at, version, created_at, updated_at, deleted_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23)")
                    .bind(entry.id)
                    .bind(entry.ledger_id)
                    .bind(entry.account_id)
                    .bind(entry.category_id)
                    .bind(enum_to_db(entry.kind)?)
                    .bind(entry.amount.amount_minor)
                    .bind(&entry.amount.currency)
                    .bind(entry.occurred_at)
                    .bind(&entry.description)
                    .bind(enum_to_db(entry.approval_state)?)
                    .bind(enum_to_db(entry.payment_state)?)
                    .bind(entry.created_by)
                    .bind(entry.submitted_by)
                    .bind(entry.approved_by)
                    .bind(entry.approved_at)
                    .bind(entry.paid_by)
                    .bind(entry.paid_at)
                    .bind(entry.received_by)
                    .bind(entry.received_at)
                    .bind(entry.version)
                    .bind(entry.created_at)
                    .bind(entry.updated_at)
                    .bind(entry.deleted_at),
            )
            .await?;
        }
        for audit in &snapshot.audit_logs {
            sqlx::Executor::execute(
                &mut **transaction,
                sqlx::query("INSERT INTO audit_logs (id, organization_id, ledger_id, actor_user_id, action, resource_type, resource_id, summary, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)")
                    .bind(audit.id)
                    .bind(audit.organization_id)
                    .bind(audit.ledger_id)
                    .bind(audit.actor_user_id)
                    .bind(&audit.action)
                    .bind(&audit.resource_type)
                    .bind(audit.resource_id)
                    .bind(&audit.summary)
                    .bind(audit.created_at),
            )
            .await?;
        }
        sqlx::Executor::execute(
            &mut **transaction,
            sqlx::query("INSERT INTO app_metadata (singleton_id, schema_version, current_user_id) VALUES (1, $1, $2)")
                .bind(snapshot.schema_version as i32)
                .bind((!snapshot.current_user_id.is_nil()).then_some(snapshot.current_user_id)),
        )
        .await?;
        Ok(())
    })
}

fn replace_auth<'a, 'connection>(
    transaction: &'a mut PgTransaction<'connection, Postgres>,
    snapshot: &'a AuthSnapshot,
) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>
where
    'connection: 'a,
{
    Box::pin(async move {
        sqlx::Executor::execute(
            &mut **transaction,
            sqlx::raw_sql(
                "DELETE FROM auth_sessions; DELETE FROM auth_installations; DELETE FROM auth_users;",
            ),
        )
        .await?;
        for user in &snapshot.users {
            sqlx::Executor::execute(
                &mut **transaction,
                sqlx::query("INSERT INTO auth_users (id, display_name, email, phone, password_hash, account_kind, organization_id, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)")
                    .bind(user.id)
                    .bind(&user.display_name)
                    .bind(&user.email)
                    .bind(&user.phone)
                    .bind(&user.password_hash)
                    .bind(enum_to_db(user.account_kind)?)
                    .bind(user.organization_id)
                    .bind(user.created_at),
            )
            .await?;
        }
        for (installation_id, user_id) in &snapshot.installations {
            sqlx::Executor::execute(
                &mut **transaction,
                sqlx::query(
                    "INSERT INTO auth_installations (installation_id, user_id) VALUES ($1, $2)",
                )
                .bind(installation_id)
                .bind(user_id),
            )
            .await?;
        }
        for session in &snapshot.sessions {
            let installation_id =
                (!session.installation_id.is_empty()).then_some(session.installation_id.as_str());
            let refresh_token =
                (!session.refresh_token.is_empty()).then_some(session.refresh_token.as_str());
            sqlx::Executor::execute(
                &mut **transaction,
                sqlx::query("INSERT INTO auth_sessions (access_token, user_id, installation_id, refresh_token, kind, created_at, refreshed_at) VALUES ($1, $2, $3, $4, $5, $6, $7)")
                    .bind(&session.access_token)
                    .bind(session.user_id)
                    .bind(installation_id)
                    .bind(refresh_token)
                    .bind(enum_to_db(session.kind)?)
                    .bind(session.created_at)
                    .bind(session.refreshed_at),
            )
            .await?;
        }
        Ok(())
    })
}

fn enum_to_db(value: impl Serialize) -> anyhow::Result<String> {
    serde_json::to_value(value)?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("enum did not serialize as a string"))
}

fn enum_from_db<T: DeserializeOwned>(value: &str) -> anyhow::Result<T> {
    Ok(serde_json::from_value(serde_json::Value::String(
        value.to_string(),
    ))?)
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use cloudledger_service::{
        AppConfirmTransactionReceiptInput, AppCreateOrganizationInput, AppDecideApprovalInput,
        AppMarkTransactionPaidInput, ApprovalDecision,
    };

    use super::*;
    use crate::auth::{AccountKind, AdminCreateUserInput};

    const TEST_DATABASE_URL_ENV: &str = "CLOUDLEDGER_TEST_DATABASE_URL";

    #[tokio::test]
    #[ignore = "requires an isolated PostgreSQL database in CLOUDLEDGER_TEST_DATABASE_URL"]
    async fn postgres_storage_end_to_end() {
        let database_url = std::env::var(TEST_DATABASE_URL_ENV)
            .expect("CLOUDLEDGER_TEST_DATABASE_URL must point to an isolated test database");
        reset_public_schema(&database_url).await;

        let config = DatabaseConfig {
            url: database_url.clone(),
            max_connections: 4,
            connect_timeout_seconds: 10,
        };
        let data_dir = std::env::temp_dir().join(format!(
            "cloudledger-postgres-integration-{}",
            Uuid::new_v4()
        ));
        fs::create_dir_all(&data_dir).expect("create integration data directory");
        let ledger_path = data_dir.join("ledger-state.json");
        let auth_path = data_dir.join("auth-state.json");

        let store = PostgresStore::connect(&config)
            .await
            .expect("apply schema migrations");
        const EXPECTED_TABLES: &[&str] = &[
            "app_metadata",
            "domain_users",
            "organizations",
            "organization_memberships",
            "ledgers",
            "financial_accounts",
            "transactions",
            "audit_logs",
            "auth_users",
            "auth_installations",
            "auth_sessions",
        ];
        for table in EXPECTED_TABLES {
            let relation: Option<String> = sqlx::query_scalar("SELECT to_regclass($1)::text")
                .bind(format!("public.{table}"))
                .fetch_one(&store.pool)
                .await
                .expect("inspect migrated table");
            assert_eq!(relation.as_deref(), Some(*table));
        }
        let migration_version: i64 = sqlx::query_scalar(
            "SELECT version FROM _sqlx_migrations WHERE success ORDER BY version DESC LIMIT 1",
        )
        .fetch_one(&store.pool)
        .await
        .expect("read migration version");
        assert_eq!(migration_version, 2);
        let migration_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations WHERE success")
                .fetch_one(&store.pool)
                .await
                .expect("count applied migrations");
        assert_eq!(migration_count, 2);
        let admin_organization_fk_is_deferred: bool = sqlx::query_scalar(
            "SELECT condeferrable FROM pg_constraint WHERE conname = 'auth_users_organization_id_fkey'",
        )
        .fetch_one(&store.pool)
        .await
        .expect("inspect admin organization foreign key");
        assert!(admin_organization_fk_is_deferred);

        let (empty_ledger, empty_auth, imported) = store
            .load_or_import(&ledger_path, &auth_path)
            .await
            .expect("initialize empty database");
        assert!(!imported);
        assert!(!empty_ledger.setup_status().initialized);
        assert!(empty_auth.snapshot().users.is_empty());
        drop(store);

        let reopened_store = PostgresStore::connect(&config)
            .await
            .expect("reuse already migrated schema");
        let migration_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations WHERE success")
                .fetch_one(&reopened_store.pool)
                .await
                .expect("count migrations after reconnect");
        assert_eq!(migration_count, 2);
        reopened_store.pool.close().await;

        reset_public_schema(&database_url).await;
        let mut legacy_ledger = AppLedgerService::uninitialized();
        let mut legacy_auth = AuthService::default();
        let (legacy_user_id, legacy_organization_id) = add_organization_and_admin(
            &mut legacy_ledger,
            &mut legacy_auth,
            "Legacy Organization",
            "legacy-admin@example.com",
        );
        legacy_ledger
            .save_to_path(&ledger_path)
            .expect("write legacy ledger source");
        legacy_auth
            .save_to_path(&auth_path)
            .expect("write legacy auth source");
        make_directory_read_only(&data_dir);

        let store = PostgresStore::connect(&config)
            .await
            .expect("recreate schema for JSON import");
        let (imported_ledger, imported_auth, imported) = store
            .load_or_import(&ledger_path, &auth_path)
            .await
            .expect("import read-only legacy JSON");
        make_directory_writable(&data_dir);
        assert!(imported);
        assert_eq!(imported_ledger.organizations().len(), 1);
        assert_eq!(
            imported_auth.account_kind(legacy_user_id),
            Some(AccountKind::OrganizationAdmin)
        );
        assert_eq!(
            imported_auth
                .snapshot()
                .users
                .iter()
                .find(|user| user.id == legacy_user_id)
                .and_then(|user| user.organization_id),
            Some(legacy_organization_id)
        );

        let mut invalid_ledger = imported_ledger.clone();
        let mut invalid_auth = imported_auth.clone();
        let invalid_user_id = Uuid::new_v4();
        invalid_ledger
            .create_organization(AppCreateOrganizationInput {
                organization_name: "Must Roll Back".to_string(),
                admin_user_id: invalid_user_id,
                admin_display_name: "Invalid Admin".to_string(),
                admin_email: Some("invalid-admin@example.com".to_string()),
                admin_phone: None,
            })
            .expect("stage organization for rollback test");
        invalid_auth
            .create_or_update_admin_user(AdminCreateUserInput {
                user_id: invalid_user_id,
                display_name: "Invalid Admin".to_string(),
                email: Some("invalid-admin@example.com".to_string()),
                phone: None,
                password: Some("invalid-password".to_string()),
                account_kind: AccountKind::OrganizationAdmin,
                organization_id: Some(Uuid::new_v4()),
            })
            .expect("stage invalid admin reference");
        assert!(store
            .save_all(invalid_ledger.snapshot(), invalid_auth.snapshot())
            .await
            .is_err());

        let (after_rollback_ledger, after_rollback_auth, imported_again) = store
            .load_or_import(&ledger_path, &auth_path)
            .await
            .expect("reload after failed atomic write");
        assert!(!imported_again);
        assert_eq!(after_rollback_ledger.organizations().len(), 1);
        assert_eq!(after_rollback_auth.account_kind(invalid_user_id), None);

        let mut committed_ledger = after_rollback_ledger;
        let mut committed_auth = after_rollback_auth;
        let (committed_user_id, committed_organization_id) = add_organization_and_admin(
            &mut committed_ledger,
            &mut committed_auth,
            "Committed Organization",
            "committed-admin@example.com",
        );
        store
            .save_all(committed_ledger.snapshot(), committed_auth.snapshot())
            .await
            .expect("commit organization and auth atomically");

        let (reloaded_ledger, reloaded_auth, imported_again) = store
            .load_or_import(&ledger_path, &auth_path)
            .await
            .expect("reload PostgreSQL state");
        assert!(!imported_again);
        assert_eq!(reloaded_ledger.organizations().len(), 2);
        assert_eq!(
            reloaded_auth.account_kind(committed_user_id),
            Some(AccountKind::OrganizationAdmin)
        );
        assert_eq!(
            reloaded_auth
                .snapshot()
                .users
                .iter()
                .find(|user| user.id == committed_user_id)
                .and_then(|user| user.organization_id),
            Some(committed_organization_id)
        );

        store.pool.close().await;

        let mut workflow_ledger = AppLedgerService::seeded();
        let business_owner_id = workflow_ledger.current_user_id();
        let employee_id = workflow_ledger
            .users()
            .into_iter()
            .find(|user| user.display_name == "Bob")
            .and_then(|user| Uuid::parse_str(&user.id).ok())
            .expect("seeded employee");
        let pending_id = workflow_ledger
            .snapshot()
            .transactions
            .iter()
            .find(|transaction| {
                transaction.approval_state == cloudledger_core::ApprovalState::Submitted
            })
            .map(|transaction| transaction.id)
            .expect("seeded pending reimbursement");
        workflow_ledger
            .decide_approval(AppDecideApprovalInput {
                actor_user_id: business_owner_id,
                transaction_id: pending_id,
                decision: ApprovalDecision::Approve,
                decision_note: None,
            })
            .expect("approve reimbursement");
        workflow_ledger
            .mark_transaction_paid(AppMarkTransactionPaidInput {
                actor_user_id: business_owner_id,
                transaction_id: pending_id,
            })
            .expect("mark reimbursement paid");
        workflow_ledger
            .confirm_transaction_receipt(AppConfirmTransactionReceiptInput {
                actor_user_id: employee_id,
                transaction_id: pending_id,
            })
            .expect("confirm reimbursement receipt");

        let workflow_store = PostgresStore::connect(&config)
            .await
            .expect("connect for workflow persistence");
        workflow_store
            .save_all(
                workflow_ledger.snapshot(),
                AuthService::default().snapshot(),
            )
            .await
            .expect("persist completed reimbursement workflow");
        workflow_store.pool.close().await;

        let restarted_store = PostgresStore::connect(&config)
            .await
            .expect("restart after workflow persistence");
        let (restarted_ledger, _, imported_again) = restarted_store
            .load_or_import(&ledger_path, &auth_path)
            .await
            .expect("reload completed reimbursement workflow");
        assert!(!imported_again);
        let persisted = restarted_ledger
            .snapshot()
            .transactions
            .into_iter()
            .find(|transaction| transaction.id == pending_id)
            .expect("persisted reimbursement");
        assert_eq!(
            persisted.approval_state,
            cloudledger_core::ApprovalState::Approved
        );
        assert_eq!(
            persisted.payment_state,
            cloudledger_core::PaymentState::Received
        );
        assert_eq!(persisted.approved_by, Some(business_owner_id));
        assert!(persisted.approved_at.is_some());
        assert_eq!(persisted.paid_by, Some(business_owner_id));
        assert!(persisted.paid_at.is_some());
        assert_eq!(persisted.received_by, Some(employee_id));
        assert!(persisted.received_at.is_some());

        restarted_store.pool.close().await;
        fs::remove_dir_all(data_dir).expect("remove integration data directory");
    }

    fn add_organization_and_admin(
        ledger: &mut AppLedgerService,
        auth: &mut AuthService,
        organization_name: &str,
        email: &str,
    ) -> (Uuid, Uuid) {
        let user_id = Uuid::new_v4();
        let member = ledger
            .create_organization(AppCreateOrganizationInput {
                organization_name: organization_name.to_string(),
                admin_user_id: user_id,
                admin_display_name: format!("{organization_name} Admin"),
                admin_email: Some(email.to_string()),
                admin_phone: None,
            })
            .expect("create organization");
        let organization_id = Uuid::parse_str(&member.organization_id)
            .expect("organization membership contains a UUID");
        auth.create_or_update_admin_user(AdminCreateUserInput {
            user_id,
            display_name: format!("{organization_name} Admin"),
            email: Some(email.to_string()),
            phone: None,
            password: Some("integration-password".to_string()),
            account_kind: AccountKind::OrganizationAdmin,
            organization_id: Some(organization_id),
        })
        .expect("create organization admin");
        (user_id, organization_id)
    }

    async fn reset_public_schema(database_url: &str) {
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(database_url)
            .await
            .expect("connect to isolated PostgreSQL test database");
        sqlx::Executor::execute(
            &pool,
            sqlx::raw_sql("DROP SCHEMA public CASCADE; CREATE SCHEMA public;"),
        )
        .await
        .expect("reset isolated PostgreSQL test schema");
        pool.close().await;
    }

    #[cfg(unix)]
    fn make_directory_read_only(path: &Path) {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o500))
            .expect("make legacy source directory read-only");
    }

    #[cfg(not(unix))]
    fn make_directory_read_only(_path: &Path) {}

    #[cfg(unix)]
    fn make_directory_writable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .expect("restore legacy source directory permissions");
    }

    #[cfg(not(unix))]
    fn make_directory_writable(_path: &Path) {}
}
