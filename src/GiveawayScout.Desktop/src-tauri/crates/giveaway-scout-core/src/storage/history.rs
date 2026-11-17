//! Bounded history reads use the original display order and an opaque keyset
//! cursor. The cursor carries values, not a row reference, so deleting or
//! merging the previous page's last card cannot invalidate the next page.
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use super::{Database, StorageError, campaigns, notification_from_row};
use crate::models::{NotificationHistoryPage, NotificationRecord};

const PAGE_SIZE: usize = 50;

pub(super) fn ensure_indexes(connection: &Connection) -> Result<(), StorageError> {
    // These additive indexes are created after rank-column migrations, including
    // when an old profile is opened. No history or settings values change.
    connection.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_notification_history_page_order
         ON notification_history(
            (discovery_batch IS NULL), -COALESCE(discovery_batch,0),
            CASE WHEN discovery_batch IS NOT NULL THEN
                CASE source WHEN 'freeSteamKeys' THEN 0 WHEN 'giveeClub' THEN 1
                    WHEN 'gamesForFarm' THEN 2 WHEN 'zeros' THEN 3 ELSE 4 END ELSE 0 END,
            (discovery_batch IS NOT NULL AND source_position IS NOT NULL),
            CASE WHEN discovery_batch IS NOT NULL THEN COALESCE(source_position,0) ELSE 0 END,
            received_at_utc DESC,id DESC);
         CREATE INDEX IF NOT EXISTS idx_campaigns_history_anchor ON campaigns(anchor_history_id);",
    )?;
    Ok(())
}

const SELECT: &str = "SELECT h.id,h.tracking_key,h.source,h.kind,h.title,h.url,h.received_at_utc,
        h.read_at_utc,h.delivery_state,h.delivery_attempts,h.last_delivery_error,h.store,
        (h.discovery_batch IS NULL) AS legacy,
        COALESCE(h.discovery_batch,0) AS batch,
        CASE WHEN h.discovery_batch IS NOT NULL THEN
            CASE h.source WHEN 'freeSteamKeys' THEN 0 WHEN 'giveeClub' THEN 1
                WHEN 'gamesForFarm' THEN 2 WHEN 'zeros' THEN 3 ELSE 4 END ELSE 0 END AS priority,
        (h.discovery_batch IS NOT NULL AND h.source_position IS NOT NULL) AS has_position,
        CASE WHEN h.discovery_batch IS NOT NULL THEN COALESCE(h.source_position,0) ELSE 0 END AS position
    FROM notification_history h INDEXED BY idx_notification_history_page_order
    CROSS JOIN campaigns c INDEXED BY idx_campaigns_history_anchor ON c.anchor_history_id=h.id";

const ORDER_AND_LIMIT: &str = "ORDER BY (h.discovery_batch IS NULL),-COALESCE(h.discovery_batch,0),
    CASE WHEN h.discovery_batch IS NOT NULL THEN
        CASE h.source WHEN 'freeSteamKeys' THEN 0 WHEN 'giveeClub' THEN 1
            WHEN 'gamesForFarm' THEN 2 WHEN 'zeros' THEN 3 ELSE 4 END ELSE 0 END,
    (h.discovery_batch IS NOT NULL AND h.source_position IS NOT NULL),
    CASE WHEN h.discovery_batch IS NOT NULL THEN COALESCE(h.source_position,0) ELSE 0 END,
    h.received_at_utc DESC,h.id DESC LIMIT 51";

const AFTER_CURSOR: &str = "WHERE
    (legacy,-batch,priority,has_position,position) >= (?1,?2,?3,?4,?5)
    AND ((legacy,-batch,priority,has_position,position) > (?1,?2,?3,?4,?5)
        OR (h.received_at_utc,h.id) < (?6,?7))";

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    version: u8,
    legacy: u8,
    batch: i64,
    priority: u8,
    has_position: u8,
    position: i64,
    received_at_utc: String,
    id: String,
}

impl Cursor {
    fn parse(value: &str) -> Result<Self, StorageError> {
        if value.len() > 2_048 {
            return Err(StorageError::InvalidHistoryCursor);
        }
        let cursor: Self =
            serde_json::from_str(value).map_err(|_| StorageError::InvalidHistoryCursor)?;
        if cursor.version != 1
            || cursor.legacy > 1
            || cursor.batch < 0
            || cursor.priority > 4
            || cursor.has_position > 1
            || cursor.position < 0
            || cursor.received_at_utc.is_empty()
            || cursor.received_at_utc.len() > 128
            || cursor.id.is_empty()
            || cursor.id.len() > 512
        {
            return Err(StorageError::InvalidHistoryCursor);
        }
        Ok(cursor)
    }

    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(NotificationRecord, Self)> {
        let record = notification_from_row(row)?;
        let cursor = Self {
            version: 1,
            legacy: row.get(12)?,
            batch: row.get(13)?,
            priority: row.get(14)?,
            has_position: row.get(15)?,
            position: row.get(16)?,
            received_at_utc: record.received_at_utc.clone(),
            id: record.id.clone(),
        };
        Ok((record, cursor))
    }
}

impl Database {
    /// Returns at most 50 canonical cards. The extra SQL row only determines
    /// whether another page exists; campaign metadata is enriched for this page.
    pub fn notification_history_page(
        &self,
        cursor: Option<&str>,
    ) -> Result<NotificationHistoryPage, StorageError> {
        let cursor = cursor.map(Cursor::parse).transpose()?;
        let query = format!(
            "{SELECT} {} {ORDER_AND_LIMIT}",
            if cursor.is_some() { AFTER_CURSOR } else { "" }
        );
        let mut statement = self.connection.prepare(&query)?;
        let mut records = if let Some(cursor) = cursor {
            statement
                .query_map(
                    params![
                        cursor.legacy,
                        -cursor.batch,
                        cursor.priority,
                        cursor.has_position,
                        cursor.position,
                        cursor.received_at_utc,
                        cursor.id,
                    ],
                    Cursor::from_row,
                )?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            statement
                .query_map([], Cursor::from_row)?
                .collect::<Result<Vec<_>, _>>()?
        };
        let has_more = records.len() > PAGE_SIZE;
        records.truncate(PAGE_SIZE);
        let next_cursor = if has_more {
            records
                .last()
                .map(|(_, cursor)| serde_json::to_string(cursor))
                .transpose()?
        } else {
            None
        };
        let items = records
            .into_iter()
            .map(|(record, _)| campaigns::enrich(&self.connection, record))
            .collect::<Result<_, _>>()?;
        Ok(NotificationHistoryPage { items, next_cursor })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query_plan(database: &Database, paged: bool) -> Vec<String> {
        let values: Vec<rusqlite::types::Value> = if paged {
            vec![
                0.into(),
                (-1).into(),
                0.into(),
                1.into(),
                49.into(),
                "time".to_owned().into(),
                "id".to_owned().into(),
            ]
        } else {
            Vec::new()
        };
        database
            .connection
            .prepare(&format!(
                "EXPLAIN QUERY PLAN {SELECT} {} {ORDER_AND_LIMIT}",
                if paged { AFTER_CURSOR } else { "" }
            ))
            .unwrap()
            .query_map(rusqlite::params_from_iter(values), |row| {
                row.get::<_, String>(3)
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    #[test]
    fn page_query_walks_ordered_indexes_without_sorting_the_whole_history() {
        let database = Database::open_in_memory().unwrap();
        for paged in [false, true] {
            let plan = query_plan(&database, paged);
            assert!(
                plan.iter()
                    .any(|step| step.contains("idx_notification_history_page_order")),
                "{plan:?}"
            );
            assert!(
                plan.iter()
                    .any(|step| step.contains("idx_campaigns_history_anchor")),
                "{plan:?}"
            );
            assert!(
                !plan.iter().any(|step| step.contains("TEMP B-TREE")),
                "{plan:?}"
            );
        }
    }

    #[test]
    #[ignore = "manual timing observation on an isolated 10,000-card profile"]
    fn observe_large_history_paging() {
        let database = Database::open_in_memory().unwrap();
        database.connection.execute_batch(
            "WITH RECURSIVE sequence(n) AS (VALUES(0) UNION ALL SELECT n+1 FROM sequence WHERE n<9999)
             INSERT INTO notification_history(id,tracking_key,source,kind,title,url,received_at_utc,
                store,discovery_batch,source_position)
             SELECT printf('card-%05d',n),printf('GiveeClub:%d',n),'giveeClub','giveaway',
                printf('Game %d',n),'https://example.com/game','2026-11-15T00:00:00Z','steam',1,n
             FROM sequence;
             INSERT INTO campaigns(id,anchor_history_id,title,url,kind,store,origin_source)
                SELECT 'campaign-'||id,id,title,url,kind,store,source FROM notification_history;
             INSERT INTO notification_campaigns(notification_id,campaign_id)
                SELECT id,'campaign-'||id FROM notification_history;
             INSERT INTO campaign_members(tracking_key,campaign_id,source,source_id,url,title,kind,store)
                SELECT tracking_key,'campaign-'||id,source,id,url,title,kind,store FROM notification_history;"
        ).unwrap();
        let mut observations = Vec::new();
        for legacy in [false, true] {
            if legacy {
                database
                    .connection
                    .execute(
                        "UPDATE notification_history SET discovery_batch=NULL,source_position=NULL",
                        [],
                    )
                    .unwrap();
            }
            let mut cursor = None;
            let mut timings = Vec::new();
            let mut count = 0;
            loop {
                let started = std::time::Instant::now();
                let page = database
                    .notification_history_page(cursor.as_deref())
                    .unwrap();
                timings.push(started.elapsed().as_secs_f64() * 1000.0);
                assert_eq!(page.items.len(), 50);
                count += page.items.len();
                cursor = page.next_cursor;
                if cursor.is_none() {
                    break;
                }
            }
            assert_eq!(count, 10_000);
            observations.push(serde_json::json!({
                "legacyRows": legacy, "cards": count, "pages": timings.len(), "pageSize": 50,
                "firstPageMs": timings[0], "middlePageMs": timings[timings.len()/2],
                "lastPageMs": timings[timings.len()-1],
                "firstPagePlan": query_plan(&database, false),
                "continuationPlan": query_plan(&database, true),
            }));
        }
        let report = serde_json::to_string_pretty(&serde_json::json!({
            "kind": "isolated-in-memory-debug-observation", "observations": observations
        }))
        .unwrap();
        println!("{report}");
        if let Some(path) = std::env::var_os("GIVEAWAY_SCOUT_HISTORY_PERF_REPORT") {
            std::fs::write(path, format!("{report}\n")).unwrap();
        }
    }
}
