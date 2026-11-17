//! Campaign identity and aliases keep old activation UUIDs valid while history
//! and delivery expose one card for each confirmed giveaway.
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use super::{StorageError, now_utc};
use crate::{
    campaign_identity::identify_campaign,
    models::{GameStore, GiveawayItem, GiveawayKind, GiveawaySource, NotificationRecord},
};

pub(super) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS campaigns (
    id TEXT PRIMARY KEY,
    identity TEXT UNIQUE,
    anchor_history_id TEXT,
    title TEXT NOT NULL,
    url TEXT NOT NULL,
    kind TEXT NOT NULL,
    store TEXT NOT NULL,
    origin_source TEXT,
    origin_name TEXT,
    direct_metadata INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS campaign_members (
    tracking_key TEXT PRIMARY KEY,
    campaign_id TEXT NOT NULL REFERENCES campaigns(id),
    source TEXT NOT NULL,
    source_id TEXT NOT NULL,
    url TEXT NOT NULL,
    title TEXT NOT NULL,
    kind TEXT NOT NULL,
    store TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_campaign_members_campaign ON campaign_members(campaign_id);
CREATE TABLE IF NOT EXISTS notification_campaigns (
    notification_id TEXT PRIMARY KEY REFERENCES notification_history(id) ON DELETE CASCADE,
    campaign_id TEXT NOT NULL REFERENCES campaigns(id)
);
CREATE INDEX IF NOT EXISTS idx_notification_campaigns_campaign ON notification_campaigns(campaign_id);
CREATE TABLE IF NOT EXISTS redirect_cache (
    original_url TEXT PRIMARY KEY,
    resolved_url TEXT,
    retry_after_utc TEXT,
    updated_at_utc TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS source_status (
    source TEXT PRIMARY KEY,
    status_json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS source_initial_policy (
    source TEXT PRIMARY KEY,
    mode TEXT NOT NULL CHECK(mode IN ('silent','notifyCurrent'))
);
"#;

pub(super) fn parse_source(source: &str) -> Option<GiveawaySource> {
    GiveawaySource::ALL
        .into_iter()
        .find(|candidate| candidate.storage_name() == source)
}

fn source_id(tracking_key: Option<&str>, source: GiveawaySource, fallback: &str) -> String {
    tracking_key
        .and_then(|key| key.strip_prefix(&format!("{}:", source.tracking_prefix())))
        .unwrap_or(fallback)
        .to_owned()
}

/// All rows are retained. Aliases refer to their old physical UUID, but the
/// canonical anchor keeps the position of the earliest existing history card.
pub(super) fn migrate(transaction: &Transaction<'_>) -> Result<(), StorageError> {
    transaction.execute_batch(SCHEMA)?;
    let records = {
        let mut statement = transaction.prepare(
            "SELECT id, tracking_key, source, kind, title, url, store FROM notification_history ORDER BY rowid"
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for (id, tracking, source, kind, title, url, store) in records {
        let source_enum = parse_source(&source);
        // Imported/user-created legacy records retain independent identities.
        let Some(source_enum) = source_enum else {
            let campaign_id = format!("legacy:{id}");
            transaction.execute("INSERT INTO campaigns(id,identity,anchor_history_id,title,url,kind,store) VALUES(?1,?1,?2,?3,?4,?5,?6)", params![campaign_id,id,title,url,kind,store])?;
            transaction.execute(
                "INSERT INTO notification_campaigns(notification_id,campaign_id) VALUES(?1,?2)",
                params![id, campaign_id],
            )?;
            continue;
        };
        let item = GiveawayItem {
            source: source_enum,
            source_id: source_id(tracking.as_deref(), source_enum, &id),
            title,
            url,
            kind: if kind == "raffle" {
                GiveawayKind::Raffle
            } else {
                GiveawayKind::Giveaway
            },
            store: GameStore::from_storage_name(&store),
            details: None,
        };
        let (campaign_id, _) = upsert_member(transaction, &item)?;
        transaction.execute(
            "INSERT INTO notification_campaigns(notification_id,campaign_id) VALUES(?1,?2)",
            params![id, campaign_id],
        )?;
        transaction.execute(
            "UPDATE campaigns SET anchor_history_id=COALESCE(anchor_history_id,?1) WHERE id=?2",
            params![id, campaign_id],
        )?;
        synchronize(transaction, &campaign_id)?;
    }
    // Source IDs already prove some silent baseline identities even though
    // v12 retained no full item. Preserve these memberships without creating
    // a history card, so a later aggregator copy cannot notify again.
    let tracked = {
        let mut statement=transaction.prepare("SELECT t.source,t.source_id,t.title FROM tracked_giveaways t LEFT JOIN campaign_members m ON m.tracking_key=t.tracking_key WHERE m.tracking_key IS NULL")?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    for (source, source_id, title) in tracked {
        let Some(source) = parse_source(&source) else {
            continue;
        };
        let url = match source {
            GiveawaySource::GiveeClub => {
                format!("https://givee.club/{}", source_id.trim_start_matches('/'))
            }
            GiveawaySource::GamesForFarm
                if source_id.starts_with("http://") || source_id.starts_with("https://") =>
            {
                source_id.clone()
            }
            GiveawaySource::GamesForFarm => format!(
                "https://freeanywhere.net/{}",
                source_id.trim_start_matches('/')
            ),
            GiveawaySource::Zeros => "http://zeros.group/free/".to_owned(),
            GiveawaySource::FreeSteamKeys => String::new(),
        };
        let inferred = GiveawayItem {
            source,
            source_id,
            title,
            url,
            kind: GiveawayKind::Giveaway,
            store: GameStore::Unknown,
            details: None,
        };
        let matching = if let Some(identity) =
            identify_campaign(inferred.source, &inferred.source_id, &inferred.url)
        {
            transaction
                .query_row(
                    "SELECT id FROM campaigns WHERE identity=?1",
                    [identity.key],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
        } else {
            None
        };
        if let Some(campaign_id) = matching {
            // An inferred platform/type must not overwrite metadata actually
            // captured from an existing history row.
            transaction.execute("INSERT INTO campaign_members(tracking_key,campaign_id,source,source_id,url,title,kind,store) VALUES(?1,?2,?3,?4,?5,?6,'giveaway','unknown')",params![inferred.tracking_key(),campaign_id,inferred.source.storage_name(),inferred.source_id,inferred.url,inferred.title])?;
        } else {
            upsert_member(transaction, &inferred)?;
        }
    }
    Ok(())
}

pub(super) fn upsert_member(
    transaction: &Transaction<'_>,
    item: &GiveawayItem,
) -> Result<(String, bool), StorageError> {
    let tracking = item.tracking_key();
    let mut existing: Option<String> = transaction
        .query_row(
            "SELECT campaign_id FROM campaign_members WHERE tracking_key=?1",
            [&tracking],
            |row| row.get(0),
        )
        .optional()?;
    let resolved: Option<String> = transaction
        .query_row(
            "SELECT resolved_url FROM redirect_cache WHERE original_url=?1",
            [&item.url],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    let effective_url = resolved.as_deref().unwrap_or(&item.url);
    let identity = identify_campaign(item.source, &item.source_id, effective_url);
    let previous_identity: Option<String> = if let Some(existing) = &existing {
        transaction
            .query_row(
                "SELECT identity FROM campaigns WHERE id=?1",
                [existing],
                |row| row.get(0),
            )
            .optional()?
            .flatten()
    } else {
        None
    };
    if let (Some(campaign_id), Some(previous), Some(next)) =
        (&existing, &previous_identity, &identity)
        && !previous.starts_with("local:")
        && previous != &next.key
    {
        // A publisher may edit an old post to point at another campaign. This
        // is evidence of reassignment, never evidence that two confirmed IDs
        // are equal. Keep historical attribution/activation UUIDs on the old
        // campaign and move only the current source membership.
        let archived_source_id = format!("archived:{}:{}", item.source_id, campaign_id);
        let archived_tracking = format!("{}:{}", item.source.tracking_prefix(), archived_source_id);
        transaction.execute(
            "INSERT OR IGNORE INTO campaign_members(tracking_key,campaign_id,source,source_id,url,title,kind,store)
             SELECT ?1,m.campaign_id,m.source,?2,c.url,m.title,m.kind,m.store
             FROM campaign_members m JOIN campaigns c ON c.id=m.campaign_id WHERE m.tracking_key=?3",
            params![archived_tracking,archived_source_id,tracking],
        )?;
        transaction.execute(
            "DELETE FROM campaign_members WHERE tracking_key=?1",
            [&tracking],
        )?;
        existing = None;
    }
    let key = identity
        .as_ref()
        .map(|value| value.key.clone())
        .or(previous_identity)
        .unwrap_or_else(|| format!("local:{tracking}"));
    let matched: Option<String> = transaction
        .query_row(
            "SELECT id FROM campaigns WHERE identity=?1",
            [&key],
            |row| row.get(0),
        )
        .optional()?;
    let newly_created = existing.is_none() && matched.is_none();
    let campaign_id = match (existing, matched) {
        (Some(existing), Some(matched)) if existing != matched => {
            merge(transaction, &existing, &matched)?
        }
        (Some(existing), _) => existing,
        (_, Some(matched)) => matched,
        _ => {
            let id = uuid::Uuid::new_v4().to_string();
            transaction.execute(
                "INSERT INTO campaigns(id,identity,title,url,kind,store) VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    id,
                    key,
                    item.title,
                    item.url,
                    item.kind.storage_name(),
                    item.store.storage_name()
                ],
            )?;
            id
        }
    };
    transaction.execute(
        "UPDATE campaigns SET identity=?1 WHERE id=?2",
        params![key, campaign_id],
    )?;
    transaction.execute(
        "INSERT INTO campaign_members(tracking_key,campaign_id,source,source_id,url,title,kind,store) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
         ON CONFLICT(tracking_key) DO UPDATE SET campaign_id=excluded.campaign_id,url=excluded.url,title=excluded.title,kind=excluded.kind,store=excluded.store",
        params![tracking,campaign_id,item.source.storage_name(),item.source_id,item.url,item.title,item.kind.storage_name(),item.store.storage_name()],
    )?;
    let origin_source = identity.as_ref().and_then(|value| value.origin_source);
    let origin_name = identity
        .as_ref()
        .and_then(|value| value.origin_name.clone());
    let url = identity
        .as_ref()
        .map(|value| value.url.as_str())
        .unwrap_or(effective_url);
    let direct = origin_source == Some(item.source)
        || (origin_source.is_none() && item.source != GiveawaySource::FreeSteamKeys);
    transaction.execute(
        "UPDATE campaigns SET
             url=CASE WHEN ?9 OR origin_name IS NULL THEN ?1 ELSE url END,
             origin_source=CASE WHEN ?9 THEN ?2 ELSE origin_source END,
             origin_name=CASE WHEN ?9 THEN ?3 ELSE origin_name END,
             title=CASE WHEN ?4 OR direct_metadata=0 THEN ?5 ELSE title END,
             kind=CASE WHEN ?4 OR direct_metadata=0 THEN ?6 ELSE kind END,
             store=CASE WHEN ?4 OR direct_metadata=0 THEN ?7 ELSE store END,
             direct_metadata=MAX(direct_metadata,?4)
         WHERE id=?8",
        params![
            url,
            origin_source.map(GiveawaySource::storage_name),
            origin_name,
            direct,
            item.title,
            item.kind.storage_name(),
            item.store.storage_name(),
            campaign_id,
            identity.is_some()
        ],
    )?;
    synchronize(transaction, &campaign_id)?;
    Ok((campaign_id, newly_created))
}

fn anchor_rank(transaction: &Transaction<'_>, id: &str) -> Result<Option<i64>, StorageError> {
    Ok(transaction.query_row("SELECT h.rowid FROM campaigns c JOIN notification_history h ON h.id=c.anchor_history_id WHERE c.id=?1",[id],|row|row.get(0)).optional()?)
}

fn merge(transaction: &Transaction<'_>, first: &str, second: &str) -> Result<String, StorageError> {
    let first_rank = anchor_rank(transaction, first)?;
    let second_rank = anchor_rank(transaction, second)?;
    let (keep, remove) =
        if second_rank.is_some() && (first_rank.is_none() || second_rank < first_rank) {
            (second, first)
        } else {
            (first, second)
        };
    // Prefer already collected metadata from the actual origin, independently
    // from the source settings and from the UUID chosen as the stable anchor.
    transaction.execute("UPDATE campaigns SET
        title=CASE WHEN direct_metadata=0 AND (SELECT direct_metadata FROM campaigns WHERE id=?2)=1 THEN (SELECT title FROM campaigns WHERE id=?2) ELSE title END,
        kind=CASE WHEN direct_metadata=0 AND (SELECT direct_metadata FROM campaigns WHERE id=?2)=1 THEN (SELECT kind FROM campaigns WHERE id=?2) ELSE kind END,
        store=CASE WHEN direct_metadata=0 AND (SELECT direct_metadata FROM campaigns WHERE id=?2)=1 THEN (SELECT store FROM campaigns WHERE id=?2) ELSE store END,
        direct_metadata=MAX(direct_metadata,(SELECT direct_metadata FROM campaigns WHERE id=?2)),
        anchor_history_id=COALESCE(anchor_history_id,(SELECT anchor_history_id FROM campaigns WHERE id=?2)) WHERE id=?1",params![keep,remove])?;
    transaction.execute(
        "UPDATE campaign_members SET campaign_id=?1 WHERE campaign_id=?2",
        params![keep, remove],
    )?;
    transaction.execute(
        "UPDATE notification_campaigns SET campaign_id=?1 WHERE campaign_id=?2",
        params![keep, remove],
    )?;
    transaction.execute("DELETE FROM campaigns WHERE id=?1", [remove])?;
    synchronize(transaction, keep)?;
    Ok(keep.to_owned())
}

/// Keep aliases' state synchronized, so old OS activations/read IDs and pending
/// delivery rechecks observe the same earliest read and successful delivery.
pub(super) fn synchronize(transaction: &Transaction<'_>, id: &str) -> Result<(), StorageError> {
    transaction.execute(
        "UPDATE notification_history SET
          read_at_utc=(SELECT MIN(h.read_at_utc) FROM notification_history h JOIN notification_campaigns n ON n.notification_id=h.id WHERE n.campaign_id=?1),
          delivery_state=CASE WHEN EXISTS(SELECT 1 FROM notification_history h JOIN notification_campaigns n ON n.notification_id=h.id WHERE n.campaign_id=?1 AND h.delivery_state='delivered') THEN 'delivered' ELSE delivery_state END,
          title=(SELECT title FROM campaigns WHERE id=?1),url=(SELECT url FROM campaigns WHERE id=?1),
          kind=(SELECT kind FROM campaigns WHERE id=?1),store=(SELECT store FROM campaigns WHERE id=?1)
         WHERE id IN(SELECT notification_id FROM notification_campaigns WHERE campaign_id=?1)", [id])?;
    Ok(())
}

pub(super) fn enrich(
    connection: &Connection,
    mut record: NotificationRecord,
) -> Result<NotificationRecord, StorageError> {
    let campaign: Option<(String,Option<String>,Option<String>)>=connection.query_row("SELECT c.id,c.origin_source,c.origin_name FROM campaigns c JOIN notification_campaigns n ON n.campaign_id=c.id WHERE n.notification_id=?1",[&record.id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
    if let Some((id, source, name)) = campaign {
        record.campaign_id = Some(id.clone());
        record.origin_source = source.as_deref().and_then(parse_source);
        record.origin_name = name;
        let mut statement = connection
            .prepare("SELECT DISTINCT source FROM campaign_members WHERE campaign_id=?1")?;
        let sources = statement
            .query_map([&id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        record.detected_sources = GiveawaySource::ALL
            .into_iter()
            .filter(|source| sources.iter().any(|name| name == source.storage_name()))
            .collect();
    }
    Ok(record)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedirectCacheEntry {
    pub url: Option<String>,
    pub retry_after_utc: Option<String>,
}

impl super::Database {
    pub fn cached_redirect(
        &self,
        original_url: &str,
    ) -> Result<Option<RedirectCacheEntry>, StorageError> {
        Ok(self
            .connection
            .query_row(
                "SELECT resolved_url,retry_after_utc FROM redirect_cache WHERE original_url=?1",
                [original_url],
                |row| {
                    Ok(RedirectCacheEntry {
                        url: row.get(0)?,
                        retry_after_utc: row.get(1)?,
                    })
                },
            )
            .optional()?)
    }
    pub fn cache_redirect(
        &self,
        original_url: &str,
        resolved_url: Option<&str>,
        retry_after_utc: Option<&str>,
    ) -> Result<(), StorageError> {
        // Independent feed lookups may finish out of order. Keep the first
        // confirmed destination until the source publishes a different input
        // URL, which uses a separate cache key.
        self.connection.execute(
            "INSERT INTO redirect_cache(original_url,resolved_url,retry_after_utc,updated_at_utc)
             VALUES(?1,?2,CASE WHEN ?2 IS NULL THEN ?3 ELSE NULL END,?4)
             ON CONFLICT(original_url) DO UPDATE SET
               resolved_url=COALESCE(redirect_cache.resolved_url,excluded.resolved_url),
               retry_after_utc=CASE WHEN COALESCE(excluded.resolved_url,redirect_cache.resolved_url) IS NULL
                   THEN excluded.retry_after_utc ELSE NULL END,
               updated_at_utc=excluded.updated_at_utc",
            params![original_url,resolved_url,retry_after_utc,now_utc()?],
        )?;
        Ok(())
    }
    pub fn pending_redirects(&self, limit: usize) -> Result<Vec<String>, StorageError> {
        let mut statement=self.connection.prepare("SELECT DISTINCT m.url FROM campaign_members m LEFT JOIN redirect_cache r ON r.original_url=m.url WHERE m.source='freeSteamKeys' AND m.url LIKE '%freesteamkeys.com/open/%' AND r.resolved_url IS NULL AND (r.retry_after_utc IS NULL OR r.retry_after_utc<=?1) ORDER BY m.rowid DESC LIMIT ?2")?;
        Ok(statement
            .query_map(
                params![now_utc()?, i64::try_from(limit).unwrap_or(i64::MAX)],
                |row| row.get(0),
            )?
            .collect::<Result<Vec<_>, _>>()?)
    }
    pub fn apply_resolved_url(
        &mut self,
        original_url: &str,
        resolved_url: &str,
    ) -> Result<(), StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        transaction.execute("INSERT INTO redirect_cache(original_url,resolved_url,retry_after_utc,updated_at_utc) VALUES(?1,?2,NULL,?3) ON CONFLICT(original_url) DO UPDATE SET resolved_url=COALESCE(redirect_cache.resolved_url,excluded.resolved_url),retry_after_utc=NULL,updated_at_utc=excluded.updated_at_utc",params![original_url,resolved_url,now_utc()?])?;
        let members = {
            let mut statement = transaction.prepare(
                "SELECT source,source_id,title,kind,store FROM campaign_members WHERE url=?1",
            )?;
            statement
                .query_map([original_url], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        for (source, source_id, title, kind, store) in members {
            if let Some(source) = parse_source(&source) {
                upsert_member(
                    &transaction,
                    &GiveawayItem {
                        source,
                        source_id,
                        title,
                        url: original_url.to_owned(),
                        kind: if kind == "raffle" {
                            GiveawayKind::Raffle
                        } else {
                            GiveawayKind::Giveaway
                        },
                        store: GameStore::from_storage_name(&store),
                        details: None,
                    },
                )?;
            }
        }
        transaction.commit()?;
        Ok(())
    }
}
