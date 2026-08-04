use std::{future::Future, path::PathBuf, pin::Pin};

use cloudledger_service::{AppLedgerService, AppLedgerSnapshot};

use crate::{
    audit::SecurityAuditEvent,
    auth::{AuthService, AuthSnapshot},
};

use super::PostgresStore;

#[derive(Debug, Clone)]
pub enum BackendStore {
    Postgres(PostgresStore),
    Json(JsonStore),
}

#[derive(Debug, Clone)]
pub struct JsonStore {
    ledger_state_path: PathBuf,
    auth_state_path: PathBuf,
}

impl BackendStore {
    pub(crate) fn append_security_event(
        &self,
        event: SecurityAuditEvent,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
        Box::pin(async move {
            match self {
                Self::Postgres(store) => store.append_security_event(event).await,
                Self::Json(_) => Ok(()),
            }
        })
    }

    pub(crate) fn json(ledger_state_path: PathBuf, auth_state_path: PathBuf) -> Self {
        Self::Json(JsonStore {
            ledger_state_path,
            auth_state_path,
        })
    }

    pub(crate) fn save_ledger(
        &self,
        snapshot: AppLedgerSnapshot,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
        Box::pin(async move {
            match self {
                Self::Postgres(store) => store.save_ledger(snapshot).await,
                Self::Json(store) => Ok(AppLedgerService::from_snapshot(snapshot)
                    .save_to_path(&store.ledger_state_path)?),
            }
        })
    }

    pub(crate) fn save_auth(
        &self,
        snapshot: AuthSnapshot,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
        Box::pin(async move {
            match self {
                Self::Postgres(store) => store.save_auth(snapshot).await,
                Self::Json(store) => Ok(
                    AuthService::from_snapshot(snapshot).save_to_path(&store.auth_state_path)?
                ),
            }
        })
    }

    pub(crate) fn save_all(
        &self,
        ledger: AppLedgerSnapshot,
        auth: AuthSnapshot,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
        Box::pin(async move {
            match self {
                Self::Postgres(store) => store.save_all(ledger, auth).await,
                Self::Json(store) => {
                    AppLedgerService::from_snapshot(ledger)
                        .save_to_path(&store.ledger_state_path)?;
                    AuthService::from_snapshot(auth).save_to_path(&store.auth_state_path)?;
                    Ok(())
                }
            }
        })
    }
}
