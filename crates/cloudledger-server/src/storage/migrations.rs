use sqlx::{migrate::Migrator, PgPool};

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");
pub const LATEST_MIGRATION: i64 = 4;

pub async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
    MIGRATOR.run(pool).await?;
    Ok(())
}

pub async fn ensure_current(pool: &PgPool) -> anyhow::Result<()> {
    let version: Option<i64> = sqlx::query_scalar(
        "SELECT version FROM _sqlx_migrations WHERE success ORDER BY version DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    if version != Some(LATEST_MIGRATION) {
        anyhow::bail!(
            "database schema is not current (expected migration {}, found {:?}); run cloudledger-server migrate",
            LATEST_MIGRATION,
            version
        );
    }
    Ok(())
}
