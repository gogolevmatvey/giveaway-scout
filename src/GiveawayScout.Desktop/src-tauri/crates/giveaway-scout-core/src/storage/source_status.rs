use rusqlite::{OptionalExtension, params};

use super::{Database, StorageError};
use crate::models::{GiveawaySource, SourcePhase, SourceStatus};

impl Database {
    pub fn source_statuses(&self) -> Result<Vec<SourceStatus>, StorageError> {
        let settings = self.settings()?;
        GiveawaySource::ALL
            .into_iter()
            .map(|source| {
                let stored: Option<String> = self
                    .connection
                    .query_row(
                        "SELECT status_json FROM source_status WHERE source=?1",
                        [source.storage_name()],
                        |row| row.get(0),
                    )
                    .optional()?;
                let mut status = stored
                    .map(|json| serde_json::from_str::<SourceStatus>(&json))
                    .transpose()?
                    .unwrap_or_else(|| SourceStatus::new(source));
                if !settings.allows_source(source) {
                    status.phase = SourcePhase::Disabled;
                    status.next_check_at_utc = None;
                }
                Ok(status)
            })
            .collect()
    }

    pub fn save_source_status(&self, status: &SourceStatus) -> Result<(), StorageError> {
        self.connection.execute("INSERT INTO source_status(source,status_json) VALUES(?1,?2) ON CONFLICT(source) DO UPDATE SET status_json=excluded.status_json",params![status.source.storage_name(),serde_json::to_string(status)?])?;
        Ok(())
    }
}
