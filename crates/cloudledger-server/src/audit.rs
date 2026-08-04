use std::collections::{BTreeMap, BTreeSet};

use cloudledger_core::AuditLog;
use hmac::{Hmac, Mac};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::Sha256;
use sqlx::{PgPool, Postgres, Row, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::config::AuditSecurityConfig;

#[derive(Debug, Clone)]
pub struct AuditSigner {
    key_id: String,
    key: [u8; 32],
}

#[derive(Debug, Clone)]
pub struct AuditAppend {
    pub scope_key: String,
    pub actor_type: String,
    pub actor_id: Option<Uuid>,
    pub action: String,
    pub resource_type: String,
    pub resource_id: Option<Uuid>,
    pub metadata: Value,
    pub occurred_at: OffsetDateTime,
    pub id: Uuid,
}

#[derive(Debug, Clone)]
pub struct SecurityAuditEvent {
    pub scope_key: String,
    pub actor_type: String,
    pub actor_id: Option<Uuid>,
    pub action: String,
    pub resource_type: String,
    pub resource_id: Option<Uuid>,
    pub metadata: Value,
}

impl SecurityAuditEvent {
    pub fn into_append(self) -> AuditAppend {
        AuditAppend {
            scope_key: self.scope_key,
            actor_type: self.actor_type,
            actor_id: self.actor_id,
            action: self.action,
            resource_type: self.resource_type,
            resource_id: self.resource_id,
            metadata: self.metadata,
            occurred_at: OffsetDateTime::now_utc(),
            id: Uuid::new_v4(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditVerificationReport {
    pub chains: usize,
    pub events: usize,
}

#[derive(Serialize)]
struct CanonicalEvent<'a> {
    scope_key: &'a str,
    sequence: i64,
    previous_hash: String,
    key_id: &'a str,
    actor_type: &'a str,
    actor_id: Option<String>,
    action: &'a str,
    resource_type: &'a str,
    resource_id: Option<String>,
    metadata: &'a Value,
    occurred_at_unix_micros: String,
    id: String,
}

impl AuditSigner {
    pub fn from_config(config: &AuditSecurityConfig) -> anyhow::Result<Self> {
        Ok(Self {
            key_id: config.key_id.clone(),
            key: config.hmac_key_bytes()?,
        })
    }

    pub fn development_default() -> Self {
        Self {
            key_id: "development-test-key".to_string(),
            key: [0x5a; 32],
        }
    }

    pub async fn initialize_legacy(&self, pool: &PgPool) -> anyhow::Result<()> {
        let existing: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_events")
            .fetch_one(pool)
            .await?;
        if existing > 0 {
            return Ok(());
        }

        let legacy = sqlx::query(
            "SELECT id, organization_id, ledger_id, actor_user_id, action, resource_type, resource_id, summary, created_at FROM audit_logs_legacy ORDER BY created_at, id",
        )
        .fetch_all(pool)
        .await?;
        let mut transaction = pool.begin().await?;
        let mut scopes = BTreeSet::new();
        for row in legacy {
            let organization_id: Option<Uuid> = row.try_get("organization_id")?;
            let scope_key = scope_key(organization_id);
            scopes.insert(scope_key.clone());
            self.append(
                &mut transaction,
                AuditAppend {
                    scope_key,
                    actor_type: "business_user".to_string(),
                    actor_id: Some(row.try_get("actor_user_id")?),
                    action: row.try_get("action")?,
                    resource_type: row.try_get("resource_type")?,
                    resource_id: Some(row.try_get("resource_id")?),
                    metadata: json!({
                        "ledger_id": row.try_get::<Uuid, _>("ledger_id")?.to_string(),
                        "summary": row.try_get::<String, _>("summary")?,
                        "source": "legacy_audit_logs"
                    }),
                    occurred_at: row.try_get("created_at")?,
                    id: row.try_get("id")?,
                },
            )
            .await?;
        }
        if scopes.is_empty() {
            scopes.insert("platform".to_string());
        }
        for scope_key in scopes {
            self.append(
                &mut transaction,
                AuditAppend {
                    scope_key,
                    actor_type: "system".to_string(),
                    actor_id: None,
                    action: "legacy_cutover".to_string(),
                    resource_type: "audit_chain".to_string(),
                    resource_id: None,
                    metadata: json!({"schema_version": 4}),
                    occurred_at: OffsetDateTime::now_utc(),
                    id: Uuid::new_v4(),
                },
            )
            .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn append_domain_logs(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        audits: &[AuditLog],
    ) -> anyhow::Result<()> {
        for audit in audits {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM audit_events WHERE id = $1)")
                    .bind(audit.id)
                    .fetch_one(&mut **transaction)
                    .await?;
            if exists {
                continue;
            }
            self.append(
                transaction,
                AuditAppend {
                    scope_key: scope_key(audit.organization_id),
                    actor_type: "business_user".to_string(),
                    actor_id: Some(audit.actor_user_id),
                    action: audit.action.clone(),
                    resource_type: audit.resource_type.clone(),
                    resource_id: Some(audit.resource_id),
                    metadata: json!({
                        "ledger_id": audit.ledger_id.to_string(),
                        "summary": audit.summary,
                    }),
                    occurred_at: audit.created_at,
                    id: audit.id,
                },
            )
            .await?;
        }
        Ok(())
    }

    pub async fn append(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        event: AuditAppend,
    ) -> anyhow::Result<(i64, Vec<u8>)> {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(&event.scope_key)
            .execute(&mut **transaction)
            .await?;
        let head = sqlx::query(
            "SELECT sequence, event_hash FROM audit_events WHERE scope_key = $1 ORDER BY sequence DESC LIMIT 1",
        )
        .bind(&event.scope_key)
        .fetch_optional(&mut **transaction)
        .await?;
        let (sequence, previous_hash) = match head {
            Some(row) => (
                row.try_get::<i64, _>("sequence")? + 1,
                row.try_get::<Vec<u8>, _>("event_hash")?,
            ),
            None => (1, Vec::new()),
        };
        let event_hash = self.sign(sequence, &previous_hash, &event)?;
        sqlx::query(
            "INSERT INTO audit_events (id, scope_key, sequence, previous_hash, event_hash, key_id, actor_type, actor_id, action, resource_type, resource_id, metadata, occurred_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
        )
        .bind(event.id)
        .bind(&event.scope_key)
        .bind(sequence)
        .bind(&previous_hash)
        .bind(&event_hash)
        .bind(&self.key_id)
        .bind(&event.actor_type)
        .bind(event.actor_id)
        .bind(&event.action)
        .bind(&event.resource_type)
        .bind(event.resource_id)
        .bind(&event.metadata)
        .bind(event.occurred_at)
        .execute(&mut **transaction)
        .await?;
        println!(
            "{}",
            json!({
                "event": "audit_chain_head",
                "scope": event.scope_key,
                "sequence": sequence,
                "hash": hex::encode(&event_hash),
                "key_id": self.key_id,
            })
        );
        Ok((sequence, event_hash))
    }

    pub async fn verify(&self, pool: &PgPool) -> anyhow::Result<AuditVerificationReport> {
        let rows = sqlx::query(
            "SELECT id, scope_key, sequence, previous_hash, event_hash, key_id, actor_type, actor_id, action, resource_type, resource_id, metadata, occurred_at FROM audit_events ORDER BY scope_key, sequence",
        )
        .fetch_all(pool)
        .await?;
        let mut heads: BTreeMap<String, (i64, Vec<u8>)> = BTreeMap::new();
        for row in &rows {
            let scope_key: String = row.try_get("scope_key")?;
            let sequence: i64 = row.try_get("sequence")?;
            let previous_hash: Vec<u8> = row.try_get("previous_hash")?;
            let expected_head = heads
                .get(&scope_key)
                .cloned()
                .unwrap_or_else(|| (0, Vec::new()));
            if sequence != expected_head.0 + 1 || previous_hash != expected_head.1 {
                anyhow::bail!("audit chain linkage failed at {scope_key} sequence {sequence}");
            }
            let event = AuditAppend {
                scope_key: scope_key.clone(),
                actor_type: row.try_get("actor_type")?,
                actor_id: row.try_get("actor_id")?,
                action: row.try_get("action")?,
                resource_type: row.try_get("resource_type")?,
                resource_id: row.try_get("resource_id")?,
                metadata: row.try_get("metadata")?,
                occurred_at: row.try_get("occurred_at")?,
                id: row.try_get("id")?,
            };
            let key_id: String = row.try_get("key_id")?;
            if key_id != self.key_id {
                anyhow::bail!("unknown audit key id {key_id} at {scope_key} sequence {sequence}");
            }
            let expected_hash = self.sign(sequence, &previous_hash, &event)?;
            let event_hash: Vec<u8> = row.try_get("event_hash")?;
            if expected_hash != event_hash {
                anyhow::bail!("audit event hash failed at {scope_key} sequence {sequence}");
            }
            heads.insert(scope_key, (sequence, event_hash));
        }
        Ok(AuditVerificationReport {
            chains: heads.len(),
            events: rows.len(),
        })
    }

    fn sign(
        &self,
        sequence: i64,
        previous_hash: &[u8],
        event: &AuditAppend,
    ) -> anyhow::Result<Vec<u8>> {
        let canonical = serde_json::to_vec(&CanonicalEvent {
            scope_key: &event.scope_key,
            sequence,
            previous_hash: hex::encode(previous_hash),
            key_id: &self.key_id,
            actor_type: &event.actor_type,
            actor_id: event.actor_id.map(|id| id.to_string()),
            action: &event.action,
            resource_type: &event.resource_type,
            resource_id: event.resource_id.map(|id| id.to_string()),
            metadata: &event.metadata,
            occurred_at_unix_micros: (event.occurred_at.unix_timestamp_nanos() / 1_000).to_string(),
            id: event.id.to_string(),
        })?;
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.key).expect("HMAC accepts a 32-byte key");
        mac.update(&canonical);
        Ok(mac.finalize().into_bytes().to_vec())
    }
}

pub fn scope_key(organization_id: Option<Uuid>) -> String {
    organization_id
        .map(|id| format!("organization:{id}"))
        .unwrap_or_else(|| "platform".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_hash_changes_for_sequence_or_metadata() {
        let signer = AuditSigner::development_default();
        let event = AuditAppend {
            scope_key: "platform".to_string(),
            actor_type: "system".to_string(),
            actor_id: None,
            action: "test".to_string(),
            resource_type: "test".to_string(),
            resource_id: None,
            metadata: json!({"a": 1, "b": 2}),
            occurred_at: OffsetDateTime::UNIX_EPOCH,
            id: Uuid::nil(),
        };
        let first = signer.sign(1, &[], &event).unwrap();
        let second = signer.sign(2, &first, &event).unwrap();
        let mut changed = event.clone();
        changed.metadata = json!({"a": 1, "b": 3});
        assert_ne!(first, second);
        assert_ne!(first, signer.sign(1, &[], &changed).unwrap());
    }
}
