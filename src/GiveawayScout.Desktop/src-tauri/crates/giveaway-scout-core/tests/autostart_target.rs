use giveaway_scout_core::{AppSettings, AutostartTarget, Database};
use rusqlite::Connection;

#[test]
fn new_and_older_settings_default_to_gui_without_enabling_autostart() {
    let settings = Database::open_in_memory().unwrap().settings().unwrap();
    assert!(!settings.start_at_login);
    assert_eq!(settings.autostart_target, AutostartTarget::Gui);
    let mut value = serde_json::to_value(AppSettings::default()).unwrap();
    value.as_object_mut().unwrap().remove("autostartTarget");
    assert_eq!(
        serde_json::from_value::<AppSettings>(value)
            .unwrap()
            .autostart_target,
        AutostartTarget::Gui
    );
}

#[test]
fn v14_migration_preserves_enabled_state_and_selection_survives_reopening() {
    for enabled in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("profile.db");
        let database = Database::open(&path).unwrap();
        let mut settings = database.settings().unwrap();
        settings.start_at_login = enabled;
        settings.keep_running_in_tray = true;
        database.update_settings(&settings).unwrap();
        drop(database);
        Connection::open(&path)
            .unwrap()
            .execute_batch(
                "ALTER TABLE settings DROP COLUMN autostart_target;
             DELETE FROM schema_migrations WHERE version=15;",
            )
            .unwrap();
        let database = Database::open(&path).unwrap();
        assert_eq!(database.settings().unwrap(), settings);
        settings.autostart_target = AutostartTarget::Cli;
        database.update_settings(&settings).unwrap();
        drop(database);
        assert_eq!(Database::open(&path).unwrap().settings().unwrap(), settings);
    }
}

#[test]
fn failed_v15_migration_rolls_back_without_changing_startup_preferences() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("profile.db");
    drop(Database::open(&path).unwrap());
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "ALTER TABLE settings DROP COLUMN autostart_target;
         DELETE FROM schema_migrations WHERE version=15;
         UPDATE settings SET start_at_login=1;
         CREATE TRIGGER fail_v15 BEFORE INSERT ON schema_migrations WHEN NEW.version=15
         BEGIN SELECT RAISE(FAIL, 'mock migration error'); END;",
        )
        .unwrap();
    drop(connection);
    assert!(Database::open(&path).is_err());
    let connection = Connection::open(&path).unwrap();
    assert!(
        connection
            .query_row("SELECT start_at_login FROM settings", [], |row| row
                .get::<_, bool>(0))
            .unwrap()
    );
    assert!(!connection.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('settings') WHERE name='autostart_target')", [], |row| row.get::<_, bool>(0)).unwrap());
    connection.execute_batch("DROP TRIGGER fail_v15").unwrap();
    drop(connection);
    let settings = Database::open(&path).unwrap().settings().unwrap();
    assert!(settings.start_at_login);
    assert_eq!(settings.autostart_target, AutostartTarget::Gui);
}
