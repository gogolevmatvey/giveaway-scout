use giveaway_scout_core::{AppSettings, Database};
use rusqlite::Connection;

#[test]
fn new_profiles_and_old_json_enable_automatic_update_checks() {
    assert!(AppSettings::default().automatic_update_checks);
    assert!(
        Database::open_in_memory()
            .unwrap()
            .settings()
            .unwrap()
            .automatic_update_checks
    );
    let mut old_json = serde_json::to_value(AppSettings::default()).unwrap();
    old_json
        .as_object_mut()
        .unwrap()
        .remove("automaticUpdateChecks");
    assert!(
        serde_json::from_value::<AppSettings>(old_json)
            .unwrap()
            .automatic_update_checks
    );
}

#[test]
fn schema_15_migration_preserves_preferences_and_round_trips_both_choices() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("profile.db");
    let database = Database::open(&path).unwrap();
    let mut expected = database.settings().unwrap();
    expected.source_check_intervals.givee_club = 17;
    expected.notification_sound_enabled = false;
    expected.language_code = Some("en".into());
    expected.keep_running_in_tray = true;
    database.update_settings(&expected).unwrap();
    database.mark_update_announced("1.2.0").unwrap();
    drop(database);
    Connection::open(&path)
        .unwrap()
        .execute_batch(
            "ALTER TABLE settings DROP COLUMN automatic_update_checks;
         DELETE FROM schema_migrations WHERE version=16;",
        )
        .unwrap();

    let database = Database::open(&path).unwrap();
    assert_eq!(database.settings().unwrap(), expected);
    assert!(database.update_was_announced("1.2.0").unwrap());
    drop(database);
    for enabled in [false, true, false] {
        expected.automatic_update_checks = enabled;
        let database = Database::open(&path).unwrap();
        database.update_settings(&expected).unwrap();
        drop(database);
        assert_eq!(Database::open(&path).unwrap().settings().unwrap(), expected);
    }
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        16
    );
}

#[test]
fn failed_schema_16_migration_rolls_back_and_can_retry() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("profile.db");
    let database = Database::open(&path).unwrap();
    let mut expected = database.settings().unwrap();
    expected.start_in_tray = true;
    database.update_settings(&expected).unwrap();
    drop(database);
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "ALTER TABLE settings DROP COLUMN automatic_update_checks;
         DELETE FROM schema_migrations WHERE version=16;
         CREATE TRIGGER fail_v16 BEFORE INSERT ON schema_migrations WHEN NEW.version=16
         BEGIN SELECT RAISE(ABORT, 'fixture migration failure'); END;",
        )
        .unwrap();
    drop(connection);
    assert!(Database::open(&path).is_err());
    let connection = Connection::open(&path).unwrap();
    assert!(!connection.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('settings') WHERE name='automatic_update_checks')", [], |row| row.get::<_, bool>(0)).unwrap());
    assert_eq!(
        connection
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        15
    );
    connection.execute_batch("DROP TRIGGER fail_v16").unwrap();
    drop(connection);
    assert_eq!(Database::open(&path).unwrap().settings().unwrap(), expected);
}
