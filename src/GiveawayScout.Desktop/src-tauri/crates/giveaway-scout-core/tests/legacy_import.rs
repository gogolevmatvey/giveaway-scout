use std::{fs, path::Path};

use giveaway_scout_core::{
    AppSettings, DataCounts, Database, GiveawaySource, InitialSyncMode, StorageError,
};
use rusqlite::Connection;
use tempfile::TempDir;

const READ_NOTIFICATION_ID: &str = "11111111-1111-4111-8111-111111111111";
const UNREAD_NOTIFICATION_ID: &str = "22222222-2222-4222-8222-222222222222";
const SERVICE_NOTIFICATION_ID: &str = "33333333-3333-4333-8333-333333333333";

#[test]
fn imports_wpf_json_without_service_entries_and_preserves_read_state() {
    let temp = fixture_directory();
    let mut database = Database::open(temp.path().join("giveaway-scout.db")).unwrap();

    let report = database.migrate_legacy_json(temp.path()).unwrap();

    assert!(!report.already_completed);
    assert!(report.settings_imported);
    assert_eq!(report.tracked_imported, 3);
    assert_eq!(report.notifications_imported, 2);
    assert_eq!(report.service_entries_skipped, 2);
    for source in [GiveawaySource::GiveeClub, GiveawaySource::GamesForFarm] {
        assert!(database.source_tracking_initialized(source).unwrap());
    }
    assert_eq!(
        database.settings().unwrap(),
        AppSettings {
            check_interval_minutes: 30,
            http_timeout_seconds: 30,
            http_retry_count: 2,
            givee_club_enabled: true,
            games_for_farm_enabled: false,
            zeros_enabled: false,
            steam_notifications_enabled: true,
            epic_games_notifications_enabled: true,
            notification_sound_enabled: false,
            start_at_login: true,
            start_in_tray: false,
            keep_running_in_tray: false,
            language_code: Some("ru".to_owned()),
            verbose_logging: false,
            initial_sync_mode: None,
        }
    );
    assert_eq!(
        database.counts().unwrap(),
        DataCounts {
            tracked_giveaways: 3,
            notification_history: 2,
            unread_notifications: 1,
        }
    );
    assert!(
        database
            .contains_tracking_key("GamesForFarm:/game?n=205")
            .unwrap()
    );
    assert!(
        !database
            .contains_tracking_key("GamesForFarm:/user#settings")
            .unwrap()
    );
    assert!(
        database
            .notification(SERVICE_NOTIFICATION_ID)
            .unwrap()
            .is_none()
    );

    let read = database
        .notification(READ_NOTIFICATION_ID)
        .unwrap()
        .unwrap();
    assert!(read.read_at_utc.is_some());
    let unread = database
        .notification(UNREAD_NOTIFICATION_ID)
        .unwrap()
        .unwrap();
    assert_eq!(
        unread.tracking_key.as_deref(),
        Some("GamesForFarm:/game?n=205")
    );
    assert!(unread.read_at_utc.is_none());

    let notifications = database.notifications().unwrap();
    assert_eq!(notifications.len(), 2);
    assert!(
        database
            .mark_notification_read(UNREAD_NOTIFICATION_ID)
            .unwrap()
    );
    assert_eq!(database.counts().unwrap().unread_notifications, 0);
    assert!(!database.initial_sync_choice_required().unwrap());
}

#[test]
fn marks_all_notifications_read() {
    let temp = fixture_directory();
    let mut database = Database::open(temp.path().join("giveaway-scout.db")).unwrap();
    database.migrate_legacy_json(temp.path()).unwrap();

    assert_eq!(database.mark_all_notifications_read().unwrap(), 1);
    assert_eq!(database.counts().unwrap().unread_notifications, 0);
    assert_eq!(database.mark_all_notifications_read().unwrap(), 0);
}

#[test]
fn updates_settings_and_rejects_invalid_values() {
    let database = Database::open_in_memory().unwrap();
    let mut settings = database.settings().unwrap();
    settings.check_interval_minutes = 45;
    settings.http_timeout_seconds = 60;
    settings.http_retry_count = 3;
    settings.givee_club_enabled = false;
    settings.language_code = Some("en".to_owned());

    database.update_settings(&settings).unwrap();
    assert_eq!(database.settings().unwrap(), settings);

    settings.check_interval_minutes = 4;
    assert!(matches!(
        database.update_settings(&settings),
        Err(StorageError::InvalidSettings(_))
    ));
    settings.check_interval_minutes = 45;
    settings.http_timeout_seconds = 4;
    assert!(matches!(
        database.update_settings(&settings),
        Err(StorageError::InvalidSettings(_))
    ));
    settings.http_timeout_seconds = 60;
    settings.http_retry_count = 6;
    assert!(matches!(
        database.update_settings(&settings),
        Err(StorageError::InvalidSettings(_))
    ));
    settings.http_retry_count = 3;
    settings.language_code = Some("de".to_owned());
    assert!(matches!(
        database.update_settings(&settings),
        Err(StorageError::InvalidSettings(_))
    ));
}

#[test]
fn migrates_existing_settings_to_network_policy_defaults() {
    let temp = tempfile::tempdir().unwrap();
    let database_path = temp.path().join("giveaway-scout.db");
    let connection = Connection::open(&database_path).unwrap();
    connection
        .execute_batch(
            r#"
            CREATE TABLE settings (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                check_interval_minutes INTEGER NOT NULL,
                givee_club_enabled INTEGER NOT NULL,
                games_for_farm_enabled INTEGER NOT NULL,
                notification_sound_enabled INTEGER NOT NULL,
                start_at_login INTEGER NOT NULL,
                keep_running_in_tray INTEGER NOT NULL,
                language_code TEXT,
                verbose_logging INTEGER NOT NULL,
                updated_at_utc TEXT NOT NULL
            );
            INSERT INTO settings VALUES (1, 45, 1, 0, 1, 0, 1, 'ru', 1, '2026-01-01T00:00:00Z');
            "#,
        )
        .unwrap();
    drop(connection);

    let database = Database::open(&database_path).unwrap();
    let settings = database.settings().unwrap();
    assert_eq!(settings.check_interval_minutes, 45);
    assert_eq!(settings.http_timeout_seconds, 30);
    assert_eq!(settings.http_retry_count, 2);
    assert!(settings.givee_club_enabled);
    assert!(!settings.games_for_farm_enabled);
    assert!(settings.keep_running_in_tray);
    assert!(settings.verbose_logging);
    assert_eq!(settings.initial_sync_mode, None);
}

#[test]
fn legacy_import_is_idempotent() {
    let temp = fixture_directory();
    let mut database = Database::open(temp.path().join("giveaway-scout.db")).unwrap();

    database.migrate_legacy_json(temp.path()).unwrap();
    let second_report = database.migrate_legacy_json(temp.path()).unwrap();

    assert!(second_report.already_completed);
    assert_eq!(second_report.tracked_imported, 0);
    assert_eq!(second_report.notifications_imported, 0);
    assert_eq!(database.counts().unwrap().tracked_giveaways, 3);
}

#[test]
fn malformed_json_does_not_partially_import_data() {
    let temp = fixture_directory();
    fs::write(temp.path().join("tracking-state.json"), "{ not valid json")
        .expect("fixture should be replaceable");
    let mut database = Database::open(temp.path().join("giveaway-scout.db")).unwrap();

    let error = database.migrate_legacy_json(temp.path()).unwrap_err();

    assert!(matches!(error, StorageError::ParseLegacyFile { .. }));
    assert!(!database.legacy_migration_completed().unwrap());
    assert_eq!(database.settings().unwrap(), AppSettings::default());
    assert_eq!(database.counts().unwrap(), DataCounts::default());
}

#[test]
fn clearing_giveaway_data_keeps_settings_and_allows_a_fresh_baseline() {
    let temp = fixture_directory();
    let mut database = Database::open(temp.path().join("giveaway-scout.db")).unwrap();
    database.migrate_legacy_json(temp.path()).unwrap();
    let mut settings_before = database.settings().unwrap();
    settings_before.initial_sync_mode = Some(InitialSyncMode::NotifyCurrent);
    database.update_settings(&settings_before).unwrap();
    assert!(database.data_size_bytes().unwrap() > 0);

    database.clear_giveaway_data().unwrap();

    assert_eq!(database.counts().unwrap(), DataCounts::default());
    assert_eq!(database.settings().unwrap(), settings_before);
    assert!(database.data_size_bytes().unwrap() > 0);
    assert!(database.legacy_migration_completed().unwrap());
    assert!(!database.initial_sync_choice_required().unwrap());
}

fn fixture_directory() -> TempDir {
    let temp = tempfile::tempdir().expect("temporary directory should be created");
    copy_fixture("settings.json", temp.path());
    copy_fixture("tracking-state.json", temp.path());
    copy_fixture("notification-history.json", temp.path());
    temp
}

fn copy_fixture(name: &str, destination: &Path) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("legacy")
        .join(name);
    fs::copy(source, destination.join(name)).expect("fixture should be copied");
}
