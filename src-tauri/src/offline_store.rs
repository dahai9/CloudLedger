use std::{path::Path, sync::Mutex};

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

const MAX_DOCUMENT_BYTES: usize = 5 * 1024 * 1024;
const CACHE_SCHEMA_VERSION: &str = "2";

pub struct OfflineStore {
    connection: Mutex<Connection>,
}

impl OfflineStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let mut connection = Connection::open(path).map_err(|error| error.to_string())?;
        connection
            .execute_batch(
                "
                PRAGMA journal_mode = WAL;
                PRAGMA foreign_keys = ON;
                CREATE TABLE IF NOT EXISTS offline_cache (
                    user_id TEXT PRIMARY KEY NOT NULL,
                    document TEXT NOT NULL,
                    updated_at INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS offline_metadata (
                    key TEXT PRIMARY KEY NOT NULL,
                    value TEXT NOT NULL
                );
                ",
            )
            .map_err(|error| error.to_string())?;
        let cache_schema_version = connection
            .query_row(
                "SELECT value FROM offline_metadata WHERE key = 'cache_schema_version'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if cache_schema_version.as_deref() != Some(CACHE_SCHEMA_VERSION) {
            let transaction = connection
                .transaction()
                .map_err(|error| error.to_string())?;
            transaction
                .execute("DELETE FROM offline_cache", [])
                .map_err(|error| error.to_string())?;
            transaction
                .execute("DELETE FROM offline_metadata", [])
                .map_err(|error| error.to_string())?;
            transaction
                .execute(
                    "INSERT INTO offline_metadata (key, value) VALUES ('cache_schema_version', ?1)",
                    [CACHE_SCHEMA_VERSION],
                )
                .map_err(|error| error.to_string())?;
            transaction.commit().map_err(|error| error.to_string())?;
        }
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn load_last(&self) -> Result<Option<Value>, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "offline store lock poisoned")?;
        let document = connection
            .query_row(
                "SELECT cache.document
                 FROM offline_metadata metadata
                 JOIN offline_cache cache ON cache.user_id = metadata.value
                 WHERE metadata.key = 'active_user_id'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        document
            .map(|document| serde_json::from_str(&document).map_err(|error| error.to_string()))
            .transpose()
    }

    pub fn save(&self, user_id: &str, document: &Value) -> Result<(), String> {
        let encoded = serde_json::to_vec(document).map_err(|error| error.to_string())?;
        if encoded.len() > MAX_DOCUMENT_BYTES {
            return Err("offline cache exceeds 5 MiB".to_string());
        }
        let document = String::from_utf8(encoded).map_err(|error| error.to_string())?;
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| "offline store lock poisoned")?;
        let transaction = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "INSERT INTO offline_cache (user_id, document, updated_at)
                 VALUES (?1, ?2, unixepoch())
                 ON CONFLICT(user_id) DO UPDATE SET
                   document = excluded.document,
                   updated_at = excluded.updated_at",
                params![user_id, document],
            )
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "INSERT INTO offline_metadata (key, value) VALUES ('active_user_id', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![user_id],
            )
            .map_err(|error| error.to_string())?;
        transaction.commit().map_err(|error| error.to_string())
    }

    pub fn clear(&self, user_id: &str) -> Result<(), String> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| "offline store lock poisoned")?;
        let transaction = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "DELETE FROM offline_cache WHERE user_id = ?1",
                params![user_id],
            )
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "DELETE FROM offline_metadata
                 WHERE key = 'active_user_id' AND value = ?1",
                params![user_id],
            )
            .map_err(|error| error.to_string())?;
        transaction.commit().map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_version_two_store_clears_legacy_cache() {
        let path = std::env::temp_dir().join(format!(
            "cloudledger-offline-cache-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        {
            let connection = Connection::open(&path).expect("open legacy store");
            connection
                .execute_batch(
                    "
                    CREATE TABLE offline_cache (
                        user_id TEXT PRIMARY KEY NOT NULL,
                        document TEXT NOT NULL,
                        updated_at INTEGER NOT NULL
                    );
                    CREATE TABLE offline_metadata (
                        key TEXT PRIMARY KEY NOT NULL,
                        value TEXT NOT NULL
                    );
                    INSERT INTO offline_cache (user_id, document, updated_at)
                    VALUES ('legacy-user', '{\"version\":1}', unixepoch());
                    INSERT INTO offline_metadata (key, value)
                    VALUES ('active_user_id', 'legacy-user');
                    ",
                )
                .expect("seed legacy cache");
        }

        let store = OfflineStore::open(&path).expect("upgrade cache");
        assert!(store.load_last().expect("load cache").is_none());
        store
            .save("current-user", &serde_json::json!({"version": 2}))
            .expect("save current cache");
        assert_eq!(
            store.load_last().expect("load current cache"),
            Some(serde_json::json!({"version": 2}))
        );
        drop(store);
        std::fs::remove_file(path).expect("remove cache");
    }
}
