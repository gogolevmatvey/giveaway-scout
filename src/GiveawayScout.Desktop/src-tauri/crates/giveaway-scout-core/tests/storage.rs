use giveaway_scout_core::{
    DataCounts, Database, GiveawaySource, InitialSyncMode, StorageError, parse_givee_club,
};
use rusqlite::Connection;

#[test]
fn marks_all_notifications_read() {
    let (_directory, database) = populated_database();
    let count = database.counts().unwrap().unread_notifications;
    assert!(count > 0);
    assert_eq!(database.mark_all_notifications_read().unwrap(), count);
    assert_eq!(database.counts().unwrap().unread_notifications, 0);
    assert_eq!(database.mark_all_notifications_read().unwrap(), 0);
}

#[test]
fn clearing_giveaway_data_keeps_settings_and_allows_a_fresh_baseline() {
    let (_directory, mut database) = populated_database();
    let settings_before = database.settings().unwrap();
    assert!(database.data_size_bytes().unwrap() > 0);

    database.clear_giveaway_data().unwrap();

    assert_eq!(database.counts().unwrap(), DataCounts::default());
    assert_eq!(database.settings().unwrap(), settings_before);
    assert!(database.data_size_bytes().unwrap() > 0);
    assert!(!database.initial_sync_choice_required().unwrap());
}

fn populated_database() -> (tempfile::TempDir, Database) {
    let directory = tempfile::tempdir().unwrap();
    let mut database = Database::open(directory.path().join("giveaway-scout.db")).unwrap();
    let mut settings = database.settings().unwrap();
    settings.initial_sync_mode = Some(InitialSyncMode::NotifyCurrent);
    database.update_settings(&settings).unwrap();
    let items = parse_givee_club(include_str!("fixtures/html/givee-club.html")).unwrap();
    database
        .process_discovered_items(&items, &[GiveawaySource::GiveeClub])
        .unwrap();
    (directory, database)
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
