use rusqlite::{OptionalExtension, params};
use time::OffsetDateTime;
use uuid::Uuid;

use super::{Database, StorageError};

pub struct UsageStatisticsRequest {
    pub installation_id: String,
    pub day: String,
}

impl Database {
    /// Called only by the production reporter. Attempts are limited across restarts.
    pub fn usage_statistics_request(
        &self,
        now: OffsetDateTime,
    ) -> Result<Option<UsageStatisticsRequest>, StorageError> {
        let day = now.date().to_string();
        let state = self.connection.query_row(
            "SELECT installation_id, last_attempt_at, last_success_day FROM usage_statistics_state WHERE id = 1",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?, row.get::<_, Option<String>>(2)?)),
        ).optional()?;
        if let Some((_, last_attempt, last_success)) = state.as_ref()
            && (last_success.as_deref() == Some(day.as_str())
                || last_attempt
                    .is_some_and(|attempt| (0..3600).contains(&(now.unix_timestamp() - attempt))))
        {
            return Ok(None);
        }
        let identifier = state.map_or_else(|| Uuid::new_v4().to_string(), |state| state.0);
        self.connection.execute(
            "INSERT INTO usage_statistics_state(id, installation_id, last_attempt_at) VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET last_attempt_at = excluded.last_attempt_at",
            params![identifier, now.unix_timestamp()],
        )?;
        Ok(Some(UsageStatisticsRequest {
            installation_id: identifier,
            day,
        }))
    }

    pub fn usage_statistics_succeeded(
        &self,
        request: &UsageStatisticsRequest,
    ) -> Result<(), StorageError> {
        self.connection.execute(
            "UPDATE usage_statistics_state SET last_success_day = ?1
             WHERE installation_id = ?2",
            params![request.day, request.installation_id],
        )?;
        Ok(())
    }
}
