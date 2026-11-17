mod campaigns;
mod history;
mod source_status;
mod update_announcements;
mod usage_statistics;

pub use campaigns::RedirectCacheEntry;
pub use usage_statistics::UsageStatisticsRequest;

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use rusqlite::{
    Connection, ErrorCode, OptionalExtension, Transaction, TransactionBehavior, params,
};
use serde::Serialize;
use thiserror::Error;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use crate::models::{
    AppSettings, AutostartTarget, DataCounts, DiscoveryResult, GameStore, GiveawayItem,
    GiveawaySource, InitialSyncMode, NotificationRecord,
};

const TRACKING_INITIALIZED_KEY: &str = "tracking_initialized";
const FORCE_SILENT_BASELINE_KEY: &str = "force_silent_baseline";

// A single policy for both queue snapshots and the final per-record recheck.
// Read records retain their actual delivery state, but are no longer retried.
const ELIGIBLE_NOTIFICATION_SELECT: &str = r#"
SELECT history.id, history.tracking_key, history.source, history.kind,
       history.title, history.url, history.received_at_utc, history.read_at_utc,
       history.delivery_state, history.delivery_attempts, history.last_delivery_error, history.store
FROM notification_history AS history
JOIN notification_campaigns aliases ON aliases.notification_id=history.id
JOIN campaigns campaign ON campaign.id=aliases.campaign_id AND campaign.anchor_history_id=history.id
CROSS JOIN settings AS prefs
WHERE prefs.id = 1 AND history.delivery_state != 'delivered'
    AND history.read_at_utc IS NULL
    AND EXISTS (SELECT 1 FROM campaign_members member WHERE member.campaign_id=campaign.id
        AND ((member.source = 'giveeClub' AND prefs.givee_club_enabled)
        OR (member.source = 'gamesForFarm' AND prefs.games_for_farm_enabled)
        OR (member.source = 'zeros' AND prefs.zeros_enabled)
        OR (member.source = 'freeSteamKeys' AND prefs.free_steam_keys_enabled)))
    AND (history.store != 'steam' OR prefs.steam_notifications_enabled)
    AND (history.store != 'epicGames' OR prefs.epic_games_notifications_enabled)
    AND (history.store != 'gog' OR prefs.gog_notifications_enabled)
"#;

const SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at_utc TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS app_metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    check_interval_minutes INTEGER NOT NULL CHECK (check_interval_minutes >= 5),
    source_check_intervals_json TEXT NOT NULL DEFAULT '{"freeSteamKeys":60,"giveeClub":60,"gamesForFarm":60,"zeros":60}',
    http_timeout_seconds INTEGER NOT NULL DEFAULT 30 CHECK (http_timeout_seconds BETWEEN 5 AND 120),
    http_retry_count INTEGER NOT NULL DEFAULT 2 CHECK (http_retry_count BETWEEN 0 AND 5),
    givee_club_enabled INTEGER NOT NULL CHECK (givee_club_enabled IN (0, 1)),
    games_for_farm_enabled INTEGER NOT NULL CHECK (games_for_farm_enabled IN (0, 1)),
    zeros_enabled INTEGER NOT NULL DEFAULT 0 CHECK (zeros_enabled IN (0, 1)),
    free_steam_keys_enabled INTEGER NOT NULL DEFAULT 1 CHECK (free_steam_keys_enabled IN (0, 1)),
    steam_notifications_enabled INTEGER NOT NULL DEFAULT 1 CHECK (steam_notifications_enabled IN (0, 1)),
    epic_games_notifications_enabled INTEGER NOT NULL DEFAULT 1 CHECK (epic_games_notifications_enabled IN (0, 1)),
    gog_notifications_enabled INTEGER NOT NULL DEFAULT 1 CHECK (gog_notifications_enabled IN (0, 1)),
    notification_sound_enabled INTEGER NOT NULL CHECK (notification_sound_enabled IN (0, 1)),
    start_at_login INTEGER NOT NULL CHECK (start_at_login IN (0, 1)),
    autostart_target TEXT NOT NULL DEFAULT 'gui' CHECK (autostart_target IN ('gui', 'cli')),
    start_in_tray INTEGER NOT NULL DEFAULT 0 CHECK (start_in_tray IN (0, 1)),
    keep_running_in_tray INTEGER NOT NULL CHECK (keep_running_in_tray IN (0, 1)),
    language_code TEXT CHECK (language_code IS NULL OR language_code IN ('ru', 'en')),
    verbose_logging INTEGER NOT NULL CHECK (verbose_logging IN (0, 1)),
    log_max_file_size_mb INTEGER NOT NULL DEFAULT 2 CHECK (log_max_file_size_mb BETWEEN 1 AND 100),
    log_file_count INTEGER NOT NULL DEFAULT 1 CHECK (log_file_count BETWEEN 1 AND 10),
    log_directory TEXT,
    initial_sync_mode TEXT CHECK (initial_sync_mode IS NULL OR initial_sync_mode IN ('silent', 'notifyCurrent')),
    updated_at_utc TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS tracked_giveaways (
    tracking_key TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    source_id TEXT NOT NULL,
    title TEXT NOT NULL,
    first_seen_utc TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_tracked_giveaways_source
    ON tracked_giveaways(source);

CREATE TABLE IF NOT EXISTS notification_history (
    id TEXT PRIMARY KEY,
    tracking_key TEXT,
    source TEXT NOT NULL,
    kind TEXT NOT NULL,
    title TEXT NOT NULL,
    url TEXT NOT NULL,
    received_at_utc TEXT NOT NULL,
    read_at_utc TEXT,
    delivery_state TEXT NOT NULL DEFAULT 'delivered',
    delivery_attempts INTEGER NOT NULL DEFAULT 1 CHECK (delivery_attempts >= 0),
    last_delivery_error TEXT,
    store TEXT NOT NULL DEFAULT 'unknown',
    discovery_batch INTEGER CHECK (discovery_batch > 0),
    source_position INTEGER CHECK (source_position >= 0)
);

CREATE INDEX IF NOT EXISTS idx_notification_history_received
    ON notification_history(received_at_utc DESC);

CREATE INDEX IF NOT EXISTS idx_notification_history_unread
    ON notification_history(read_at_utc) WHERE read_at_utc IS NULL;
"#;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("could not create data directory {path}: {source}")]
    CreateDataDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not format timestamp: {0}")]
    FormatTimestamp(#[from] time::error::Format),
    #[error("invalid stored JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid settings: {0}")]
    InvalidSettings(String),
    #[error("invalid notification history cursor")]
    InvalidHistoryCursor,
    #[error("discovered items must belong to successfully checked sources")]
    InvalidDiscoveryBatch,
    #[error("database integrity check failed: {0}")]
    IntegrityCheckFailed(String),
    #[error("could not back up damaged database from {path} to {backup_path}: {source}")]
    BackupDamagedDatabase {
        path: PathBuf,
        backup_path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not remove damaged database file {path}: {source}")]
    RemoveDamagedDatabase {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl StorageError {
    fn is_database_corruption(&self) -> bool {
        match self {
            Self::IntegrityCheckFailed(_) => true,
            Self::Database(rusqlite::Error::SqliteFailure(error, _)) => matches!(
                error.code,
                ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase
            ),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseRecoveryReport {
    pub backup_path: String,
    pub original_error: String,
}

pub struct Database {
    connection: Connection,
}

impl Database {
    /// Flush committed WAL records before an explicitly confirmed application update.
    /// Never removes settings/history or changes the schema.
    pub fn checkpoint_before_update(&self) -> Result<bool, StorageError> {
        let busy: i32 =
            self.connection
                .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?;
        Ok(busy == 0)
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| StorageError::CreateDataDirectory {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let connection = Connection::open(path)?;
        Self::prepare(connection)
    }

    pub fn open_with_recovery(
        path: impl AsRef<Path>,
    ) -> Result<(Self, Option<DatabaseRecoveryReport>), StorageError> {
        let path = path.as_ref();
        match Self::open(path) {
            Ok(database) => Ok((database, None)),
            Err(error) if path.is_file() && error.is_database_corruption() => {
                let report = back_up_and_remove_damaged_database(path, &error)?;
                let database = Self::open(path)?;
                Ok((database, Some(report)))
            }
            Err(error) => Err(error),
        }
    }

    pub fn open_in_memory() -> Result<Self, StorageError> {
        Self::prepare(Connection::open_in_memory()?)
    }

    fn prepare(connection: Connection) -> Result<Self, StorageError> {
        connection.busy_timeout(Duration::from_secs(5))?;
        let integrity: String =
            connection.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
        if integrity != "ok" {
            return Err(StorageError::IntegrityCheckFailed(integrity));
        }
        connection.pragma_update(None, "foreign_keys", true)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.execute_batch(SCHEMA)?;

        let database = Self { connection };
        database.ensure_schema_version()?;
        history::ensure_indexes(&database.connection)?;
        database.ensure_default_settings()?;
        Ok(database)
    }

    fn ensure_schema_version(&self) -> Result<(), StorageError> {
        let applied_at_utc = now_utc()?;
        self.connection.execute(
            "INSERT OR IGNORE INTO schema_migrations(version, applied_at_utc) VALUES (1, ?1)",
            [applied_at_utc],
        )?;

        let add_timeout = !self.settings_column_exists("http_timeout_seconds")?;
        let add_retries = !self.settings_column_exists("http_retry_count")?;
        let add_initial_sync_mode = !self.settings_column_exists("initial_sync_mode")?;
        let add_steam = !self.settings_column_exists("steam_notifications_enabled")?;
        let add_epic = !self.settings_column_exists("epic_games_notifications_enabled")?;
        let add_gog = !self.settings_column_exists("gog_notifications_enabled")?;
        let add_zeros = !self.settings_column_exists("zeros_enabled")?;
        let add_free_steam_keys = !self.settings_column_exists("free_steam_keys_enabled")?;
        let add_start_in_tray = !self.settings_column_exists("start_in_tray")?;
        let add_autostart_target = !self.settings_column_exists("autostart_target")?;
        let add_log_max_file_size = !self.settings_column_exists("log_max_file_size_mb")?;
        let add_log_file_count = !self.settings_column_exists("log_file_count")?;
        let add_log_directory = !self.settings_column_exists("log_directory")?;
        let add_source_check_intervals =
            !self.settings_column_exists("source_check_intervals_json")?;
        let add_notification_store = !self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('notification_history') WHERE name = 'store')",
            [],
            |row| row.get::<_, bool>(0),
        )?;
        let add_discovery_batch = !self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('notification_history') WHERE name = 'discovery_batch')",
            [],
            |row| row.get::<_, bool>(0),
        )?;
        let add_source_position = !self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('notification_history') WHERE name = 'source_position')",
            [],
            |row| row.get::<_, bool>(0),
        )?;
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute_batch(
            "CREATE TABLE IF NOT EXISTS usage_statistics_state (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                installation_id TEXT NOT NULL,
                last_attempt_at INTEGER,
                last_success_day TEXT
            );",
        )?;
        if add_start_in_tray {
            transaction.execute(
                "ALTER TABLE settings ADD COLUMN start_in_tray INTEGER NOT NULL DEFAULT 0 CHECK (start_in_tray IN (0, 1))", [],
            )?;
            // Preserve the old coupled autostart behavior exactly once.
            transaction.execute(
                "UPDATE settings SET start_in_tray = keep_running_in_tray",
                [],
            )?;
        }
        if add_zeros {
            transaction.execute(
                "ALTER TABLE settings ADD COLUMN zeros_enabled INTEGER NOT NULL DEFAULT 0 CHECK (zeros_enabled IN (0, 1))", [],
            )?;
        }
        if add_free_steam_keys {
            transaction.execute(
                "ALTER TABLE settings ADD COLUMN free_steam_keys_enabled INTEGER NOT NULL DEFAULT 1 CHECK (free_steam_keys_enabled IN (0, 1))", [],
            )?;
        }
        if add_timeout {
            transaction.execute(
                "ALTER TABLE settings ADD COLUMN http_timeout_seconds INTEGER NOT NULL DEFAULT 30 CHECK (http_timeout_seconds BETWEEN 5 AND 120)",
                [],
            )?;
        }
        if add_retries {
            transaction.execute(
                "ALTER TABLE settings ADD COLUMN http_retry_count INTEGER NOT NULL DEFAULT 2 CHECK (http_retry_count BETWEEN 0 AND 5)",
                [],
            )?;
        }
        if add_initial_sync_mode {
            transaction.execute(
                "ALTER TABLE settings ADD COLUMN initial_sync_mode TEXT CHECK (initial_sync_mode IS NULL OR initial_sync_mode IN ('silent', 'notifyCurrent'))",
                [],
            )?;
        }
        if add_steam {
            transaction.execute(
                "ALTER TABLE settings ADD COLUMN steam_notifications_enabled INTEGER NOT NULL DEFAULT 1 CHECK (steam_notifications_enabled IN (0, 1))", [],
            )?;
        }
        if add_epic {
            transaction.execute(
                "ALTER TABLE settings ADD COLUMN epic_games_notifications_enabled INTEGER NOT NULL DEFAULT 1 CHECK (epic_games_notifications_enabled IN (0, 1))", [],
            )?;
        }
        if add_gog {
            transaction.execute(
                "ALTER TABLE settings ADD COLUMN gog_notifications_enabled INTEGER NOT NULL DEFAULT 1 CHECK (gog_notifications_enabled IN (0, 1))", [],
            )?;
        }
        if add_log_max_file_size {
            transaction.execute(
                "ALTER TABLE settings ADD COLUMN log_max_file_size_mb INTEGER NOT NULL DEFAULT 2 CHECK (log_max_file_size_mb BETWEEN 1 AND 100)", [],
            )?;
        }
        if add_log_file_count {
            transaction.execute(
                "ALTER TABLE settings ADD COLUMN log_file_count INTEGER NOT NULL DEFAULT 1 CHECK (log_file_count BETWEEN 1 AND 10)", [],
            )?;
        }
        if add_log_directory {
            transaction.execute("ALTER TABLE settings ADD COLUMN log_directory TEXT", [])?;
        }
        if add_notification_store {
            transaction.execute(
                "ALTER TABLE notification_history ADD COLUMN store TEXT NOT NULL DEFAULT 'unknown'",
                [],
            )?;
        }
        // Leave older records unranked: their original timestamp/UUID order
        // cannot be reconstructed from source page positions collected now.
        if add_discovery_batch {
            transaction.execute(
                "ALTER TABLE notification_history ADD COLUMN discovery_batch INTEGER CHECK (discovery_batch > 0)",
                [],
            )?;
        }
        if add_source_position {
            transaction.execute(
                "ALTER TABLE notification_history ADD COLUMN source_position INTEGER CHECK (source_position >= 0)",
                [],
            )?;
        }
        transaction.execute(
            "INSERT OR IGNORE INTO schema_migrations(version, applied_at_utc) VALUES (4, ?1)",
            [now_utc()?],
        )?;
        transaction.execute(
            "INSERT OR IGNORE INTO schema_migrations(version, applied_at_utc) VALUES (2, ?1)",
            [now_utc()?],
        )?;
        transaction.execute(
            "INSERT OR IGNORE INTO schema_migrations(version, applied_at_utc) VALUES (3, ?1)",
            [now_utc()?],
        )?;
        let source_baselines_migrated = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = 5)",
            [],
            |row| row.get::<_, bool>(0),
        )?;
        if !source_baselines_migrated {
            import_known_source_baselines(&transaction)?;
            transaction.execute(
                "INSERT INTO schema_migrations(version, applied_at_utc) VALUES (5, ?1)",
                [now_utc()?],
            )?;
        }
        transaction.execute(
            "INSERT OR IGNORE INTO schema_migrations(version, applied_at_utc) VALUES (6, ?1)",
            [now_utc()?],
        )?;
        transaction.execute(
            "INSERT OR IGNORE INTO schema_migrations(version, applied_at_utc) VALUES (7, ?1)",
            [now_utc()?],
        )?;
        transaction.execute(
            "INSERT OR IGNORE INTO schema_migrations(version, applied_at_utc) VALUES (8, ?1)",
            [now_utc()?],
        )?;
        transaction.execute(
            "INSERT OR IGNORE INTO schema_migrations(version, applied_at_utc) VALUES (9, ?1)",
            [now_utc()?],
        )?;
        transaction.execute(
            "INSERT OR IGNORE INTO schema_migrations(version, applied_at_utc) VALUES (10, ?1)",
            [now_utc()?],
        )?;
        transaction.execute(
            "INSERT OR IGNORE INTO schema_migrations(version, applied_at_utc) VALUES (11, ?1)",
            [now_utc()?],
        )?;
        transaction.execute(
            "INSERT OR IGNORE INTO schema_migrations(version, applied_at_utc) VALUES (12, ?1)",
            [now_utc()?],
        )?;
        let campaigns_migrated = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version=13)",
            [],
            |row| row.get::<_, bool>(0),
        )?;
        if !campaigns_migrated {
            campaigns::migrate(&transaction)?;
            transaction.execute(
                "INSERT INTO schema_migrations(version,applied_at_utc) VALUES(13,?1)",
                [now_utc()?],
            )?;
        }
        if add_source_check_intervals {
            transaction.execute(
                r#"ALTER TABLE settings ADD COLUMN source_check_intervals_json TEXT NOT NULL DEFAULT '{"freeSteamKeys":60,"giveeClub":60,"gamesForFarm":60,"zeros":60}'"#,
                [],
            )?;
            // Preserve an existing profile's shared interval exactly once. New
            // profiles have no settings row yet and retain the one-hour default.
            transaction.execute(
                "UPDATE settings SET source_check_intervals_json = json_object(
                    'freeSteamKeys', check_interval_minutes,
                    'giveeClub', check_interval_minutes,
                    'gamesForFarm', check_interval_minutes,
                    'zeros', check_interval_minutes
                )",
                [],
            )?;
        }
        transaction.execute(
            "INSERT OR IGNORE INTO schema_migrations(version, applied_at_utc) VALUES (14, ?1)",
            [now_utc()?],
        )?;
        if add_autostart_target {
            transaction.execute(
                "ALTER TABLE settings ADD COLUMN autostart_target TEXT NOT NULL DEFAULT 'gui' CHECK (autostart_target IN ('gui', 'cli'))",
                [],
            )?;
        }
        transaction.execute(
            "INSERT OR IGNORE INTO schema_migrations(version, applied_at_utc) VALUES (15, ?1)",
            [now_utc()?],
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn settings_column_exists(&self, name: &str) -> Result<bool, StorageError> {
        let mut statement = self.connection.prepare("PRAGMA table_info(settings)")?;
        let columns = statement.query_map([], |row| row.get::<_, String>(1))?;
        for column in columns {
            if column? == name {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn ensure_default_settings(&self) -> Result<(), StorageError> {
        let defaults = AppSettings::default();
        self.connection.execute(
            r#"INSERT OR IGNORE INTO settings(
                id, check_interval_minutes, http_timeout_seconds, http_retry_count,
                givee_club_enabled, games_for_farm_enabled,
                notification_sound_enabled, start_at_login, keep_running_in_tray,
                language_code, verbose_logging, initial_sync_mode, updated_at_utc, start_in_tray,
                log_max_file_size_mb, log_file_count, log_directory
            ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)"#,
            params![
                defaults.check_interval_minutes,
                defaults.http_timeout_seconds,
                defaults.http_retry_count,
                defaults.givee_club_enabled,
                defaults.games_for_farm_enabled,
                defaults.notification_sound_enabled,
                defaults.start_at_login,
                defaults.keep_running_in_tray,
                defaults.language_code,
                defaults.verbose_logging,
                defaults
                    .initial_sync_mode
                    .map(InitialSyncMode::storage_name),
                now_utc()?,
                defaults.start_in_tray,
                defaults.log_max_file_size_mb,
                defaults.log_file_count,
                defaults.log_directory,
            ],
        )?;
        Ok(())
    }

    pub fn settings(&self) -> Result<AppSettings, StorageError> {
        self.connection
            .query_row(
                r#"SELECT check_interval_minutes, givee_club_enabled, games_for_farm_enabled,
                    notification_sound_enabled, start_at_login, keep_running_in_tray, language_code,
                    verbose_logging, http_timeout_seconds, http_retry_count, initial_sync_mode,
                    steam_notifications_enabled, epic_games_notifications_enabled, zeros_enabled,
                    start_in_tray, gog_notifications_enabled,
                    log_max_file_size_mb, log_file_count, log_directory, free_steam_keys_enabled,
                    source_check_intervals_json, autostart_target
                FROM settings WHERE id = 1"#,
                [],
                |row| {
                    let initial_sync_mode = row
                        .get::<_, Option<String>>(10)?
                        .and_then(|value| InitialSyncMode::from_storage_name(&value));
                    Ok(AppSettings {
                        check_interval_minutes: row.get(0)?,
                        source_check_intervals: serde_json::from_str(&row.get::<_, String>(20)?)
                            .map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    20,
                                    rusqlite::types::Type::Text,
                                    Box::new(error),
                                )
                            })?,
                        http_timeout_seconds: row.get(8)?,
                        http_retry_count: row.get(9)?,
                        givee_club_enabled: row.get(1)?,
                        games_for_farm_enabled: row.get(2)?,
                        zeros_enabled: row.get(13)?,
                        free_steam_keys_enabled: row.get(19)?,
                        steam_notifications_enabled: row.get(11)?,
                        epic_games_notifications_enabled: row.get(12)?,
                        gog_notifications_enabled: row.get(15)?,
                        notification_sound_enabled: row.get(3)?,
                        start_at_login: row.get(4)?,
                        autostart_target: match row.get::<_, String>(21)?.as_str() {
                            "cli" => AutostartTarget::Cli,
                            _ => AutostartTarget::Gui,
                        },
                        start_in_tray: row.get(14)?,
                        keep_running_in_tray: row.get(5)?,
                        language_code: row.get(6)?,
                        verbose_logging: row.get(7)?,
                        log_max_file_size_mb: row.get(16)?,
                        log_file_count: row.get(17)?,
                        log_directory: row.get(18)?,
                        initial_sync_mode,
                    })
                },
            )
            .map_err(StorageError::from)
    }

    pub fn update_settings(&self, settings: &AppSettings) -> Result<(), StorageError> {
        Self::validate_settings(settings)?;
        self.save_settings(settings)
    }

    pub fn validate_settings(settings: &AppSettings) -> Result<(), StorageError> {
        if !(5..=1_440).contains(&settings.check_interval_minutes) {
            return Err(StorageError::InvalidSettings(
                "check interval must be between 5 and 1440 minutes".to_owned(),
            ));
        }
        for source in GiveawaySource::ALL {
            if !(5..=1_440).contains(&settings.check_interval_for(source)) {
                return Err(StorageError::InvalidSettings(format!(
                    "{} check interval must be between 5 and 1440 minutes",
                    source.storage_name()
                )));
            }
        }
        if !(5..=120).contains(&settings.http_timeout_seconds) {
            return Err(StorageError::InvalidSettings(
                "HTTP timeout must be between 5 and 120 seconds".to_owned(),
            ));
        }
        if settings.http_retry_count > 5 {
            return Err(StorageError::InvalidSettings(
                "HTTP retry count must be between 0 and 5".to_owned(),
            ));
        }
        if !(1..=100).contains(&settings.log_max_file_size_mb) {
            return Err(StorageError::InvalidSettings(
                "log file size must be between 1 and 100 MB".to_owned(),
            ));
        }
        if !(1..=10).contains(&settings.log_file_count) {
            return Err(StorageError::InvalidSettings(
                "log file count must be between 1 and 10, including the current file".to_owned(),
            ));
        }
        if settings.log_directory.as_deref().is_some_and(|directory| {
            directory.trim().is_empty()
                || directory.contains('\0')
                || !Path::new(directory).is_absolute()
        }) {
            return Err(StorageError::InvalidSettings(
                "log directory must be an absolute path or unset".to_owned(),
            ));
        }
        if settings
            .language_code
            .as_deref()
            .is_some_and(|value| !matches!(value, "ru" | "en"))
        {
            return Err(StorageError::InvalidSettings(
                "language must be ru, en, or unset".to_owned(),
            ));
        }

        Ok(())
    }

    fn save_settings(&self, settings: &AppSettings) -> Result<(), StorageError> {
        let previous = self.settings()?;
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute(
            r#"UPDATE settings SET
                check_interval_minutes = ?1,
                http_timeout_seconds = ?2,
                http_retry_count = ?3,
                givee_club_enabled = ?4,
                games_for_farm_enabled = ?5,
                notification_sound_enabled = ?6,
                start_at_login = ?7,
                keep_running_in_tray = ?8,
                language_code = ?9,
                verbose_logging = ?10,
                initial_sync_mode = ?11,
                updated_at_utc = ?12,
                steam_notifications_enabled = ?13,
                epic_games_notifications_enabled = ?14,
                zeros_enabled = ?15,
                start_in_tray = ?16,
                gog_notifications_enabled = ?17,
                log_max_file_size_mb = ?18,
                log_file_count = ?19,
                log_directory = ?20,
                free_steam_keys_enabled = ?21,
                source_check_intervals_json = ?22,
                autostart_target = ?23
            WHERE id = 1"#,
            params![
                settings.check_interval_minutes,
                settings.http_timeout_seconds,
                settings.http_retry_count,
                settings.givee_club_enabled,
                settings.games_for_farm_enabled,
                settings.notification_sound_enabled,
                settings.start_at_login,
                settings.keep_running_in_tray,
                settings.language_code,
                settings.verbose_logging,
                settings
                    .initial_sync_mode
                    .map(InitialSyncMode::storage_name),
                now_utc()?,
                settings.steam_notifications_enabled,
                settings.epic_games_notifications_enabled,
                settings.zeros_enabled,
                settings.start_in_tray,
                settings.gog_notifications_enabled,
                settings.log_max_file_size_mb,
                settings.log_file_count,
                settings.log_directory,
                settings.free_steam_keys_enabled,
                serde_json::to_string(&settings.source_check_intervals)?,
                settings.autostart_target.storage_name(),
            ],
        )?;
        let choosing_initial_mode = previous.initial_sync_mode.is_none()
            && settings.initial_sync_mode.is_some()
            && !metadata_value(&transaction, TRACKING_INITIALIZED_KEY)?
                .is_some_and(|value| value == "true")
            && !metadata_value(&transaction, FORCE_SILENT_BASELINE_KEY)?
                .is_some_and(|value| value == "true");
        for source in GiveawaySource::ALL {
            if choosing_initial_mode && settings.allows_source(source) {
                transaction.execute("INSERT INTO source_initial_policy(source,mode) VALUES(?1,?2) ON CONFLICT(source) DO UPDATE SET mode=excluded.mode",params![source.storage_name(),settings.initial_sync_mode.unwrap().storage_name()])?;
            } else if settings.allows_source(source) && !previous.allows_source(source) {
                transaction.execute(
                    "INSERT OR IGNORE INTO source_initial_policy(source,mode) VALUES(?1,'silent')",
                    [source.storage_name()],
                )?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn data_size_bytes(&self) -> Result<u64, StorageError> {
        let path = self.connection.query_row(
            "SELECT file FROM pragma_database_list WHERE name = 'main'",
            [],
            |row| row.get::<_, String>(0),
        )?;
        if path.is_empty() {
            return Ok(0);
        }

        let mut size = fs::metadata(&path).map_or(0, |metadata| metadata.len());
        for suffix in ["-wal", "-shm"] {
            let sidecar = format!("{path}{suffix}");
            size += fs::metadata(sidecar).map_or(0, |metadata| metadata.len());
        }
        Ok(size)
    }

    pub fn counts(&self) -> Result<DataCounts, StorageError> {
        let tracked_giveaways: i64 =
            self.connection
                .query_row("SELECT COUNT(*) FROM tracked_giveaways", [], |row| {
                    row.get(0)
                })?;
        let notification_history: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM campaigns WHERE anchor_history_id IS NOT NULL",
            [],
            |row| row.get(0),
        )?;
        let unread_notifications: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM campaigns c JOIN notification_history h ON h.id=c.anchor_history_id WHERE h.read_at_utc IS NULL",
            [],
            |row| row.get(0),
        )?;

        Ok(DataCounts {
            tracked_giveaways: tracked_giveaways as u64,
            notification_history: notification_history as u64,
            unread_notifications: unread_notifications as u64,
        })
    }

    pub fn tracking_initialized(&self) -> Result<bool, StorageError> {
        Ok(self
            .connection
            .query_row(
                "SELECT value FROM app_metadata WHERE key = ?1",
                [TRACKING_INITIALIZED_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .is_some_and(|value| value == "true"))
    }

    pub fn initial_sync_choice_required(&self) -> Result<bool, StorageError> {
        let force_silent_baseline = self
            .connection
            .query_row(
                "SELECT value FROM app_metadata WHERE key = ?1",
                [FORCE_SILENT_BASELINE_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .is_some_and(|value| value == "true");
        Ok(!self.tracking_initialized()?
            && !force_silent_baseline
            && self.settings()?.initial_sync_mode.is_none())
    }

    pub fn source_tracking_initialized(
        &self,
        source: GiveawaySource,
    ) -> Result<bool, StorageError> {
        Ok(self
            .connection
            .query_row(
                "SELECT value FROM app_metadata WHERE key = ?1",
                [source_baseline_key(source)],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .is_some_and(|value| value == "true"))
    }

    /// Each source must have completed a successful fetch and parse, even when
    /// it legitimately returned no public items. Failed/disabled sources must
    /// not be passed: their first successful baseline remains pending.
    pub fn process_discovered_items(
        &mut self,
        items: &[GiveawayItem],
        successful_sources: &[GiveawaySource],
    ) -> Result<DiscoveryResult, StorageError> {
        let successful_sources: HashSet<_> = successful_sources.iter().copied().collect();
        if items
            .iter()
            .any(|item| !successful_sources.contains(&item.source))
        {
            return Err(StorageError::InvalidDiscoveryBatch);
        }
        if successful_sources.is_empty() {
            return Ok(DiscoveryResult {
                is_initial_synchronization: false,
                new_items: Vec::new(),
            });
        }
        let settings = self.settings()?;
        let mut seen_keys = HashSet::new();
        let unique_items = items
            .iter()
            .enumerate()
            .filter(|(_, item)| seen_keys.insert(item.tracking_key()))
            .collect::<Vec<_>>();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let is_initialized = metadata_value(&transaction, TRACKING_INITIALIZED_KEY)?
            .is_some_and(|value| value == "true");
        let force_silent = metadata_value(&transaction, FORCE_SILENT_BASELINE_KEY)?
            .is_some_and(|value| value == "true");
        // Capture the choice for every currently enabled source before the first
        // independent completion changes the global initialization marker.
        if !is_initialized {
            for source in GiveawaySource::ALL {
                if force_silent || settings.allows_source(source) {
                    let mode = if force_silent {
                        InitialSyncMode::Silent
                    } else {
                        settings
                            .initial_sync_mode
                            .unwrap_or(InitialSyncMode::Silent)
                    };
                    transaction.execute(
                        "INSERT OR IGNORE INTO source_initial_policy(source,mode) VALUES(?1,?2)",
                        params![source.storage_name(), mode.storage_name()],
                    )?;
                }
            }
        }
        let mut initialized_sources = HashSet::new();
        for source in &successful_sources {
            if metadata_value(&transaction, &source_baseline_key(*source))?
                .is_some_and(|value| value == "true")
            {
                initialized_sources.insert(*source);
            }
        }
        let first_seen_utc = now_utc()?;
        let discovery_batch: i64 = transaction.query_row(
            "SELECT COALESCE(MAX(discovery_batch),0)+1 FROM notification_history",
            [],
            |row| row.get(0),
        )?;
        let mut new_items = Vec::new();
        for (position, item) in unique_items {
            let tracking_key = item.tracking_key();
            let tracked = tracking_key_exists(&transaction, &tracking_key)?;
            // Existing silent baseline entries are enriched too, because v12
            // only retained a title/tracking key and cannot identify duplicates.
            let (campaign_id, new_campaign) = campaigns::upsert_member(&transaction, item)?;
            transaction.execute("INSERT INTO tracked_giveaways(tracking_key,source,source_id,title,first_seen_utc) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(tracking_key) DO UPDATE SET title=excluded.title",params![tracking_key,item.source.storage_name(),item.source_id,item.title,first_seen_utc])?;
            let mode: Option<String> = transaction
                .query_row(
                    "SELECT mode FROM source_initial_policy WHERE source=?1",
                    [item.source.storage_name()],
                    |row| row.get(0),
                )
                .optional()?;
            let notify = initialized_sources.contains(&item.source)
                || mode.as_deref() == Some("notifyCurrent");
            if !tracked
                && new_campaign
                && notify
                && settings.allows_source(item.source)
                && settings.allows_store(item.store)
            {
                let id = Uuid::new_v4().to_string();
                transaction.execute(
                    "INSERT INTO notification_history(id,tracking_key,source,kind,title,url,received_at_utc,read_at_utc,delivery_state,delivery_attempts,last_delivery_error,store,discovery_batch,source_position)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,NULL,'pending',0,NULL,?8,?9,?10)",
                    params![id,tracking_key,item.source.storage_name(),item.kind.storage_name(),item.title,item.url,first_seen_utc,item.store.storage_name(),discovery_batch,i64::try_from(position).map_err(|_|StorageError::InvalidDiscoveryBatch)?],
                )?;
                transaction.execute(
                    "INSERT INTO notification_campaigns(notification_id,campaign_id) VALUES(?1,?2)",
                    params![id, campaign_id],
                )?;
                transaction.execute(
                    "UPDATE campaigns SET anchor_history_id=?1 WHERE id=?2",
                    params![id, campaign_id],
                )?;
                campaigns::synchronize(&transaction, &campaign_id)?;
                let mut canonical = item.clone();
                canonical.url = transaction.query_row(
                    "SELECT url FROM campaigns WHERE id=?1",
                    [&campaign_id],
                    |row| row.get(0),
                )?;
                new_items.push(canonical);
            }
        }
        set_metadata(&transaction, TRACKING_INITIALIZED_KEY, "true")?;
        transaction.execute(
            "DELETE FROM app_metadata WHERE key=?1",
            [FORCE_SILENT_BASELINE_KEY],
        )?;
        for source in &successful_sources {
            set_metadata(&transaction, &source_baseline_key(*source), "true")?;
            transaction.execute(
                "INSERT OR IGNORE INTO source_initial_policy(source,mode) VALUES(?1,'silent')",
                [source.storage_name()],
            )?;
        }
        transaction.commit()?;
        Ok(DiscoveryResult {
            is_initial_synchronization: !is_initialized,
            new_items,
        })
    }

    pub fn notification(&self, id: &str) -> Result<Option<NotificationRecord>, StorageError> {
        let record=self.connection.query_row(
            "SELECT id,tracking_key,source,kind,title,url,received_at_utc,read_at_utc,delivery_state,delivery_attempts,last_delivery_error,store FROM notification_history WHERE id=?1",
            [id],notification_from_row).optional()?;
        record
            .map(|record| campaigns::enrich(&self.connection, record))
            .transpose()
    }

    pub fn notifications(&self) -> Result<Vec<NotificationRecord>, StorageError> {
        let mut statement=self.connection.prepare(
            "SELECT h.id,h.tracking_key,h.source,h.kind,h.title,h.url,h.received_at_utc,h.read_at_utc,h.delivery_state,h.delivery_attempts,h.last_delivery_error,h.store
             FROM notification_history h JOIN campaigns c ON c.anchor_history_id=h.id
             ORDER BY h.discovery_batch IS NULL,h.discovery_batch DESC,
                CASE WHEN h.discovery_batch IS NOT NULL THEN
                    CASE h.source WHEN 'freeSteamKeys' THEN 0 WHEN 'giveeClub' THEN 1 WHEN 'gamesForFarm' THEN 2 WHEN 'zeros' THEN 3 ELSE 4 END END,
                CASE WHEN h.discovery_batch IS NOT NULL THEN h.source_position END,
                h.received_at_utc DESC,h.id DESC")?;
        let records = statement
            .query_map([], notification_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        records
            .into_iter()
            .map(|record| campaigns::enrich(&self.connection, record))
            .collect()
    }

    pub fn pending_notifications(&self) -> Result<Vec<NotificationRecord>, StorageError> {
        let query=format!("{ELIGIBLE_NOTIFICATION_SELECT}
             ORDER BY history.discovery_batch IS NOT NULL,history.discovery_batch,
                CASE WHEN history.discovery_batch IS NOT NULL THEN
                    CASE history.source WHEN 'freeSteamKeys' THEN 0 WHEN 'giveeClub' THEN 1 WHEN 'gamesForFarm' THEN 2 WHEN 'zeros' THEN 3 ELSE 4 END END,
                CASE WHEN history.discovery_batch IS NOT NULL THEN history.source_position END DESC,
                history.received_at_utc,history.id");
        let mut statement = self.connection.prepare(&query)?;
        let records = statement
            .query_map([], notification_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        records
            .into_iter()
            .map(|record| campaigns::enrich(&self.connection, record))
            .collect()
    }

    /// Rechecked immediately before delivery. Aliases are never independent
    /// queue entries; any enabled detecting source can make a campaign eligible.
    pub fn notification_for_delivery(
        &self,
        id: &str,
    ) -> Result<Option<NotificationRecord>, StorageError> {
        let query = format!("{ELIGIBLE_NOTIFICATION_SELECT} AND history.id=?1");
        let record = self
            .connection
            .query_row(&query, [id], notification_from_row)
            .optional()?;
        record
            .map(|record| campaigns::enrich(&self.connection, record))
            .transpose()
    }

    pub fn record_notification_delivered(&self, id: &str) -> Result<bool, StorageError> {
        Ok(self.connection.execute("UPDATE notification_history SET delivery_state='delivered',delivery_attempts=delivery_attempts+1,last_delivery_error=NULL
            WHERE id IN(SELECT notification_id FROM notification_campaigns WHERE campaign_id=(SELECT campaign_id FROM notification_campaigns WHERE notification_id=?1))",[id])?>0)
    }

    pub fn record_notification_delivery_failure(
        &self,
        id: &str,
        error: &str,
    ) -> Result<bool, StorageError> {
        // A late failed response must never undo another alias' successful OS
        // delivery recorded while a campaign was being merged.
        Ok(self.connection.execute("UPDATE notification_history SET delivery_state='pending',delivery_attempts=delivery_attempts+1,last_delivery_error=?1
            WHERE id=?2 AND delivery_state!='delivered'",params![error,id])?>0)
    }

    pub fn mark_notification_read(&self, id: &str) -> Result<bool, StorageError> {
        Ok(self.connection.execute("UPDATE notification_history SET read_at_utc=COALESCE(read_at_utc,?1)
            WHERE id IN(SELECT notification_id FROM notification_campaigns WHERE campaign_id=(SELECT campaign_id FROM notification_campaigns WHERE notification_id=?2))",params![now_utc()?,id])?>0)
    }

    pub fn mark_all_notifications_read(&self) -> Result<u64, StorageError> {
        let count = self.counts()?.unread_notifications;
        self.connection.execute(
            "UPDATE notification_history SET read_at_utc=?1 WHERE read_at_utc IS NULL",
            [now_utc()?],
        )?;
        Ok(count)
    }

    pub fn clear_giveaway_data(&mut self) -> Result<(), StorageError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute("DELETE FROM notification_campaigns", [])?;
        transaction.execute("DELETE FROM campaign_members", [])?;
        transaction.execute("DELETE FROM campaigns", [])?;
        transaction.execute("DELETE FROM redirect_cache", [])?;
        transaction.execute("DELETE FROM source_status", [])?;
        transaction.execute("DELETE FROM source_initial_policy", [])?;
        transaction.execute("DELETE FROM notification_history", [])?;
        transaction.execute("DELETE FROM tracked_giveaways", [])?;
        for source in GiveawaySource::ALL {
            transaction.execute(
                "INSERT INTO source_initial_policy(source,mode) VALUES(?1,'silent')",
                [source.storage_name()],
            )?;
            transaction.execute(
                "DELETE FROM app_metadata WHERE key = ?1",
                [source_baseline_key(source)],
            )?;
        }
        set_metadata(&transaction, TRACKING_INITIALIZED_KEY, "false")?;
        set_metadata(&transaction, FORCE_SILENT_BASELINE_KEY, "true")?;
        transaction.commit()?;
        self.connection
            .execute_batch("VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }
}

fn source_baseline_key(source: GiveawaySource) -> String {
    format!("tracking_initialized:{}", source.storage_name())
}

/// Older profiles had only a global flag. Infer source baselines only from
/// actual stored items/history; enabled settings alone do not prove a check.
fn import_known_source_baselines(transaction: &Transaction<'_>) -> Result<(), StorageError> {
    if !metadata_value(transaction, TRACKING_INITIALIZED_KEY)?.is_some_and(|value| value == "true")
    {
        return Ok(());
    }
    for source in GiveawaySource::ALL {
        let has_evidence = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM tracked_giveaways WHERE source = ?1)
                 OR EXISTS(SELECT 1 FROM notification_history WHERE source = ?1)",
            [source.storage_name()],
            |row| row.get::<_, bool>(0),
        )?;
        if has_evidence {
            set_metadata(transaction, &source_baseline_key(source), "true")?;
        }
    }
    Ok(())
}

fn back_up_and_remove_damaged_database(
    path: &Path,
    original_error: &StorageError,
) -> Result<DatabaseRecoveryReport, StorageError> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("giveaway-scout.db");
    let backup_path = path.with_file_name(format!(
        "{file_name}.corrupt-{}-{}.backup",
        OffsetDateTime::now_utc().unix_timestamp(),
        Uuid::new_v4().simple()
    ));

    copy_database_file(path, &backup_path, path, &backup_path)?;
    for suffix in ["-wal", "-shm"] {
        let source = database_sidecar_path(path, suffix);
        if source.is_file() {
            let backup = database_sidecar_path(&backup_path, suffix);
            copy_database_file(&source, &backup, path, &backup_path)?;
        }
    }

    for file in [
        database_sidecar_path(path, "-wal"),
        database_sidecar_path(path, "-shm"),
        path.to_path_buf(),
    ] {
        if file.exists() {
            fs::remove_file(&file)
                .map_err(|source| StorageError::RemoveDamagedDatabase { path: file, source })?;
        }
    }

    Ok(DatabaseRecoveryReport {
        backup_path: backup_path.to_string_lossy().into_owned(),
        original_error: original_error.to_string(),
    })
}

fn copy_database_file(
    source_path: &Path,
    destination_path: &Path,
    database_path: &Path,
    backup_path: &Path,
) -> Result<(), StorageError> {
    fs::copy(source_path, destination_path).map_err(|source| {
        StorageError::BackupDamagedDatabase {
            path: database_path.to_path_buf(),
            backup_path: backup_path.to_path_buf(),
            source,
        }
    })?;
    Ok(())
}

fn database_sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    value.into()
}

fn notification_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<NotificationRecord> {
    Ok(NotificationRecord {
        campaign_id: None,
        origin_source: None,
        origin_name: None,
        detected_sources: Vec::new(),
        store: GameStore::from_storage_name(&row.get::<_, String>(11)?),
        id: row.get(0)?,
        tracking_key: row.get(1)?,
        source: row.get(2)?,
        kind: row.get(3)?,
        title: row.get(4)?,
        url: row.get(5)?,
        received_at_utc: row.get(6)?,
        read_at_utc: row.get(7)?,
        delivery_state: row.get(8)?,
        delivery_attempts: row.get(9)?,
        last_delivery_error: row.get(10)?,
    })
}

fn tracking_key_exists(
    transaction: &Transaction<'_>,
    tracking_key: &str,
) -> Result<bool, StorageError> {
    Ok(transaction
        .query_row(
            "SELECT 1 FROM tracked_giveaways WHERE tracking_key = ?1",
            [tracking_key],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn set_metadata(transaction: &Transaction<'_>, key: &str, value: &str) -> Result<(), StorageError> {
    transaction.execute(
        r#"INSERT INTO app_metadata(key, value) VALUES (?1, ?2)
        ON CONFLICT(key) DO UPDATE SET value = excluded.value"#,
        params![key, value],
    )?;
    Ok(())
}

fn metadata_value(
    transaction: &Transaction<'_>,
    key: &str,
) -> Result<Option<String>, StorageError> {
    transaction
        .query_row(
            "SELECT value FROM app_metadata WHERE key = ?1",
            [key],
            |row| row.get(0),
        )
        .optional()
        .map_err(StorageError::from)
}

fn now_utc() -> Result<String, StorageError> {
    Ok(OffsetDateTime::now_utc().format(&Rfc3339)?)
}
