use rusqlite::params;

use super::{Database, StorageError};

impl Database {
    pub fn update_was_announced(&self, version: &str) -> Result<bool, StorageError> {
        Ok(self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM app_metadata WHERE key = ?1)",
            [format!("announced_update:{version}")],
            |row| row.get(0),
        )?)
    }

    /// Record only a successful system notification; failed delivery can retry.
    pub fn mark_update_announced(&self, version: &str) -> Result<(), StorageError> {
        self.connection.execute(
            "INSERT OR IGNORE INTO app_metadata(key, value) VALUES (?1, ?2)",
            params![format!("announced_update:{version}"), super::now_utc()?],
        )?;
        Ok(())
    }
}
