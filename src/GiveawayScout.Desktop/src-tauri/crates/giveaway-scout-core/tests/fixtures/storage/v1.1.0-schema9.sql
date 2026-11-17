-- Published v1.1.0 (78d2d3ee7569a860652daf77239c1ac2cc5e4282):
-- SCHEMA and the usage_statistics_state DDL from src/storage/mod.rs.
-- Keep this historical schema independent of the current Database initializer.
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
    http_timeout_seconds INTEGER NOT NULL DEFAULT 30 CHECK (http_timeout_seconds BETWEEN 5 AND 120),
    http_retry_count INTEGER NOT NULL DEFAULT 2 CHECK (http_retry_count BETWEEN 0 AND 5),
    givee_club_enabled INTEGER NOT NULL CHECK (givee_club_enabled IN (0, 1)),
    games_for_farm_enabled INTEGER NOT NULL CHECK (games_for_farm_enabled IN (0, 1)),
    zeros_enabled INTEGER NOT NULL DEFAULT 0 CHECK (zeros_enabled IN (0, 1)),
    steam_notifications_enabled INTEGER NOT NULL DEFAULT 1 CHECK (steam_notifications_enabled IN (0, 1)),
    epic_games_notifications_enabled INTEGER NOT NULL DEFAULT 1 CHECK (epic_games_notifications_enabled IN (0, 1)),
    gog_notifications_enabled INTEGER NOT NULL DEFAULT 1 CHECK (gog_notifications_enabled IN (0, 1)),
    notification_sound_enabled INTEGER NOT NULL CHECK (notification_sound_enabled IN (0, 1)),
    start_at_login INTEGER NOT NULL CHECK (start_at_login IN (0, 1)),
    start_in_tray INTEGER NOT NULL DEFAULT 0 CHECK (start_in_tray IN (0, 1)),
    keep_running_in_tray INTEGER NOT NULL CHECK (keep_running_in_tray IN (0, 1)),
    language_code TEXT CHECK (language_code IS NULL OR language_code IN ('ru', 'en')),
    verbose_logging INTEGER NOT NULL CHECK (verbose_logging IN (0, 1)),
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
    store TEXT NOT NULL DEFAULT 'unknown'
);

CREATE INDEX IF NOT EXISTS idx_notification_history_received
    ON notification_history(received_at_utc DESC);

CREATE INDEX IF NOT EXISTS idx_notification_history_unread
    ON notification_history(read_at_utc) WHERE read_at_utc IS NULL;

CREATE TABLE IF NOT EXISTS usage_statistics_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    installation_id TEXT NOT NULL,
    last_attempt_at INTEGER,
    last_success_day TEXT
);
