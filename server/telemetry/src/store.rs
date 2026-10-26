use hmac::{Hmac, Mac};
use rusqlite::{Connection, Transaction, params};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::{path::Path, sync::Mutex, time::Duration};
use time::{Date, Duration as Days};

pub const RETENTION_DAYS: i64 = 90;

pub(crate) fn installation_key(salt: &[u8], identifier: &str) -> String {
    let mut hmac =
        Hmac::<Sha256>::new_from_slice(salt).expect("HMAC-SHA256 accepts keys of any size");
    hmac.update(identifier.as_bytes());
    hmac.finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub struct Store {
    connection: Mutex<Connection>,
    salt: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Metrics {
    pub date_utc: String,
    pub retention_days: i64,
    pub observed_installations: i64,
    pub active_today: i64,
    pub active7_days: i64,
    pub active30_days: i64,
    pub first_seen7_days: i64,
    pub versions: Vec<VersionCount>,
    pub daily: Vec<DailyCount>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct VersionCount {
    pub version: String,
    pub installations: i64,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DailyCount {
    pub date: String,
    pub installations: i64,
}

impl Store {
    pub fn open(database: &Path, salt: Vec<u8>) -> Result<Self, crate::Error> {
        if salt.len() < 32 {
            return Err("At least 32 bytes of server salt required".into());
        }
        let connection = Connection::open(database)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        // This is the original Python schema. Existing rows and HMAC keys are preserved.
        connection.execute_batch(
            "PRAGMA foreign_keys=ON;
             PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS installations (
                 id TEXT PRIMARY KEY, first_day TEXT NOT NULL,
                 last_day TEXT NOT NULL, version TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS activity (
                 id TEXT NOT NULL REFERENCES installations(id) ON DELETE CASCADE,
                 day TEXT NOT NULL, PRIMARY KEY(id, day)
             );
             CREATE INDEX IF NOT EXISTS activity_day ON activity(day);",
        )?;
        Ok(Self {
            connection: Mutex::new(connection),
            salt,
        })
    }

    fn prune(transaction: &Transaction<'_>, today: Date) -> rusqlite::Result<()> {
        let cutoff = (today - Days::days(RETENTION_DAYS - 1)).to_string();
        transaction.execute("DELETE FROM activity WHERE day < ?", [&cutoff])?;
        transaction.execute("DELETE FROM installations WHERE last_day < ?", [&cutoff])?;
        Ok(())
    }

    pub fn record(&self, identifier: &str, version: &str, today: Date) -> rusqlite::Result<()> {
        let key = installation_key(&self.salt, identifier);
        let day = today.to_string();
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let transaction = connection.transaction()?;
        Self::prune(&transaction, today)?;
        transaction.execute(
            "INSERT INTO installations VALUES (?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET last_day=excluded.last_day,
             version=excluded.version",
            params![key, day, day, version],
        )?;
        transaction.execute(
            "INSERT OR IGNORE INTO activity VALUES (?, ?)",
            params![key, day],
        )?;
        transaction.commit()
    }

    pub fn metrics(&self, today: Date) -> rusqlite::Result<Metrics> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let transaction = connection.transaction()?;
        Self::prune(&transaction, today)?;
        let day = today.to_string();
        let start7 = (today - Days::days(6)).to_string();
        let start30 = (today - Days::days(29)).to_string();
        let data = Metrics {
            date_utc: day.clone(),
            retention_days: RETENTION_DAYS,
            observed_installations: transaction.query_row("SELECT COUNT(*) FROM installations", [], |r| r.get(0))?,
            active_today: transaction.query_row("SELECT COUNT(DISTINCT id) FROM activity WHERE day=?", [&day], |r| r.get(0))?,
            active7_days: transaction.query_row("SELECT COUNT(DISTINCT id) FROM activity WHERE day>=?", [&start7], |r| r.get(0))?,
            active30_days: transaction.query_row("SELECT COUNT(DISTINCT id) FROM activity WHERE day>=?", [&start30], |r| r.get(0))?,
            first_seen7_days: transaction.query_row("SELECT COUNT(*) FROM installations WHERE first_day>=?", [&start7], |r| r.get(0))?,
            versions: transaction.prepare("SELECT version, COUNT(*) FROM installations WHERE last_day>=? GROUP BY version ORDER BY COUNT(*) DESC LIMIT 30")?
                .query_map([&start30], |row| Ok(VersionCount { version: row.get(0)?, installations: row.get(1)? }))?
                .collect::<rusqlite::Result<_>>()?,
            daily: transaction.prepare("SELECT day, COUNT(*) FROM activity WHERE day>=? GROUP BY day ORDER BY day DESC")?
                .query_map([&start30], |row| Ok(DailyCount { date: row.get(0)?, installations: row.get(1)? }))?
                .collect::<rusqlite::Result<_>>()?,
        };
        transaction.commit()?;
        Ok(data)
    }

    pub fn maintenance(&self, today: Date) -> rusqlite::Result<()> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let transaction = connection.transaction()?;
        Self::prune(&transaction, today)?;
        transaction.commit()
    }
}
