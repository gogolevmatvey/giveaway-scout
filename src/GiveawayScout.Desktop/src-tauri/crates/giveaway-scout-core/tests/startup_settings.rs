use giveaway_scout_core::{AppSettings, Database, GiveawaySource};
use rusqlite::Connection;

#[test]
fn new_profile_startup_and_close_defaults_are_independent() {
    let database = Database::open_in_memory().unwrap();
    let settings = database.settings().unwrap();
    assert_eq!(settings, AppSettings::default());
    assert!(!settings.start_in_tray);
    assert!(!settings.keep_running_in_tray);
    assert!(!settings.start_at_login);
}

#[test]
fn omitted_start_in_tray_does_not_enable_it() {
    let mut value = serde_json::to_value(AppSettings::default()).unwrap();
    value.as_object_mut().unwrap().remove("startInTray");
    let settings: AppSettings = serde_json::from_value(value).unwrap();
    assert!(!settings.start_in_tray);
}

#[test]
fn all_startup_and_close_combinations_survive_reopen_and_clear() {
    for startup in [false, true] {
        for close in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("profile.db");
            let database = Database::open(&path).unwrap();
            let mut settings = database.settings().unwrap();
            settings.start_in_tray = startup;
            settings.keep_running_in_tray = close;
            settings.start_at_login = true;
            database.update_settings(&settings).unwrap();
            drop(database);
            let mut reopened = Database::open(&path).unwrap();
            assert_eq!(reopened.settings().unwrap(), settings);
            reopened.clear_giveaway_data().unwrap();
            assert_eq!(reopened.settings().unwrap(), settings);
        }
    }
}

fn v6_profile(path: &std::path::Path, close: bool, autostart: bool) {
    let database = Database::open(path).unwrap();
    let mut settings = database.settings().unwrap();
    settings.keep_running_in_tray = close;
    settings.start_at_login = autostart;
    settings.check_interval_minutes = 45;
    settings.language_code = Some("ru".into());
    database.update_settings(&settings).unwrap();
    drop(database);
    let connection = Connection::open(path).unwrap();
    connection.execute_batch(
        "ALTER TABLE settings DROP COLUMN start_in_tray;
         DELETE FROM schema_migrations WHERE version = 7;
         INSERT INTO tracked_giveaways VALUES ('GiveeClub:event/42','giveeClub','event/42','Saved game','2026-10-20T00:00:00Z');
         INSERT INTO notification_history(id,tracking_key,source,kind,title,url,received_at_utc,read_at_utc,delivery_state,delivery_attempts,last_delivery_error)
         VALUES ('11111111-1111-4111-8111-111111111111','GiveeClub:event/42','giveeClub','giveaway','Saved game','https://givee.club/event/42','2026-10-20T00:00:00Z','2026-10-20T01:00:00Z','pending',3,'Saved delivery error');
         INSERT INTO app_metadata(key,value) VALUES ('tracking_initialized:giveeClub','true');"
    ).unwrap();
}

#[test]
fn v6_migration_preserves_previous_visibility_history_and_delivery_state() {
    for close in [false, true] {
        for autostart in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("profile.db");
            v6_profile(&path, close, autostart);
            let database = Database::open(&path).unwrap();
            let settings = database.settings().unwrap();
            assert_eq!(settings.start_in_tray, close);
            assert_eq!(settings.keep_running_in_tray, close);
            assert_eq!(settings.start_at_login, autostart);
            assert_eq!(settings.check_interval_minutes, 45);
            assert_eq!(settings.language_code.as_deref(), Some("ru"));
            let counts = database.counts().unwrap();
            assert_eq!(counts.tracked_giveaways, 1);
            assert_eq!(counts.notification_history, 1);
            assert_eq!(counts.unread_notifications, 0);
            assert!(
                database
                    .source_tracking_initialized(GiveawaySource::GiveeClub)
                    .unwrap()
            );
            let notification = database
                .notification("11111111-1111-4111-8111-111111111111")
                .unwrap()
                .unwrap();
            assert_eq!(
                notification.read_at_utc.as_deref(),
                Some("2026-10-20T01:00:00Z")
            );
            assert_eq!(notification.delivery_state, "pending");
            assert_eq!(notification.delivery_attempts, 3);
            assert_eq!(
                notification.last_delivery_error.as_deref(),
                Some("Saved delivery error")
            );
            let mut changed = settings;
            changed.start_in_tray = !close;
            database.update_settings(&changed).unwrap();
            drop(database);
            // Opening v7 must not re-apply the old coupling.
            assert_eq!(Database::open(&path).unwrap().settings().unwrap(), changed);
        }
    }
}

#[test]
fn a_failed_v7_migration_rolls_back_the_column_and_does_not_reset_data() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("profile.db");
    v6_profile(&path, true, true);
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER fail_v7 BEFORE INSERT ON schema_migrations WHEN NEW.version = 7
         BEGIN SELECT RAISE(FAIL, 'test migration failure'); END;",
        )
        .unwrap();
    drop(connection);
    assert!(Database::open(&path).is_err());
    let connection = Connection::open(&path).unwrap();
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('settings') WHERE name='start_in_tray')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!exists);
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM notification_history", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 1);
    connection.execute_batch("DROP TRIGGER fail_v7").unwrap();
    drop(connection);
    assert!(
        Database::open(&path)
            .unwrap()
            .settings()
            .unwrap()
            .start_in_tray
    );
}
