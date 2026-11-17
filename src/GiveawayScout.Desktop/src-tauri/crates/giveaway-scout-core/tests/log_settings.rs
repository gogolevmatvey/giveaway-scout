use giveaway_scout_core::{AppSettings, Database, GameStore, GiveawaySource, StorageError};
use rusqlite::Connection;
use std::path::Path;

#[test]
fn new_profiles_and_older_json_use_log_defaults() {
    let settings = Database::open_in_memory().unwrap().settings().unwrap();
    assert_eq!(settings.log_max_file_size_mb, 2);
    assert_eq!(settings.log_file_count, 1);
    assert_eq!(settings.log_directory, None);

    let mut old_json = serde_json::to_value(&settings).unwrap();
    for key in ["logMaxFileSizeMb", "logFileCount", "logDirectory"] {
        old_json.as_object_mut().unwrap().remove(key);
    }
    assert_eq!(
        serde_json::from_value::<AppSettings>(old_json).unwrap(),
        settings
    );
}

fn v9_profile(path: &Path) -> AppSettings {
    let database = Database::open(path).unwrap();
    let mut settings = database.settings().unwrap();
    settings.check_interval_minutes = 45;
    settings.givee_club_enabled = false;
    settings.start_at_login = true;
    settings.start_in_tray = true;
    settings.verbose_logging = true;
    settings.language_code = Some("en".into());
    database.update_settings(&settings).unwrap();
    drop(database);
    let connection = Connection::open(path).unwrap();
    connection.execute_batch(
        "ALTER TABLE settings DROP COLUMN log_max_file_size_mb;
         ALTER TABLE settings DROP COLUMN log_file_count;
         ALTER TABLE settings DROP COLUMN log_directory;
         DELETE FROM schema_migrations WHERE version IN (10,13);
         DROP TABLE notification_campaigns;
         DROP TABLE campaign_members;
         DROP TABLE campaigns;
         DROP TABLE redirect_cache;
         DROP TABLE source_status;
         DROP TABLE source_initial_policy;
         INSERT INTO tracked_giveaways VALUES ('GiveeClub:event/42','giveeClub','event/42','Saved game','2026-10-20T00:00:00Z');
         INSERT INTO notification_history(id,tracking_key,source,kind,title,url,received_at_utc,read_at_utc,delivery_state,delivery_attempts,last_delivery_error,store)
         VALUES ('saved-notification','GiveeClub:event/42','giveeClub','giveaway','Saved game','https://givee.club/event/42','2026-10-20T00:00:00Z','2026-10-20T01:00:00Z','pending',3,'Saved delivery error','gog');
         INSERT INTO app_metadata(key,value) VALUES ('tracking_initialized:giveeClub','true');"
    ).unwrap();
    settings
}

#[test]
fn v10_migration_preserves_settings_history_delivery_and_source_baselines() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("profile.db");
    let settings_before = v9_profile(&path);

    let database = Database::open(&path).unwrap();
    assert_eq!(database.settings().unwrap(), settings_before);
    let counts = database.counts().unwrap();
    assert_eq!(counts.tracked_giveaways, 1);
    assert_eq!(counts.notification_history, 1);
    assert_eq!(counts.unread_notifications, 0);
    assert!(
        database
            .source_tracking_initialized(GiveawaySource::GiveeClub)
            .unwrap()
    );
    let record = database
        .notification("saved-notification")
        .unwrap()
        .unwrap();
    assert_eq!(record.store, GameStore::Gog);
    assert_eq!(record.read_at_utc.as_deref(), Some("2026-10-20T01:00:00Z"));
    assert_eq!(record.delivery_state, "pending");
    assert_eq!(record.delivery_attempts, 3);
    assert_eq!(
        record.last_delivery_error.as_deref(),
        Some("Saved delivery error")
    );
    drop(database);

    let connection = Connection::open(&path).unwrap();
    let migration_count: u32 = connection
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 10",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(migration_count, 1);
    drop(connection);
    assert_eq!(
        Database::open(&path).unwrap().settings().unwrap(),
        settings_before
    );
}

#[test]
fn failed_v10_migration_rolls_back_all_log_columns_and_preserves_data() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("profile.db");
    let settings_before = v9_profile(&path);
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER fail_v10 BEFORE INSERT ON schema_migrations WHEN NEW.version = 10
         BEGIN SELECT RAISE(FAIL, 'test migration failure'); END;",
        )
        .unwrap();
    drop(connection);

    assert!(Database::open(&path).is_err());
    let connection = Connection::open(&path).unwrap();
    let log_columns: u32 = connection.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('settings') WHERE name IN ('log_max_file_size_mb', 'log_file_count', 'log_directory')", [], |row| row.get(0)
    ).unwrap();
    assert_eq!(log_columns, 0);
    let history_count: u32 = connection
        .query_row("SELECT COUNT(*) FROM notification_history", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(history_count, 1);
    connection.execute_batch("DROP TRIGGER fail_v10").unwrap();
    drop(connection);
    assert_eq!(
        Database::open(&path).unwrap().settings().unwrap(),
        settings_before
    );
}

#[test]
fn rejects_invalid_log_limits_and_directories_without_changing_saved_settings() {
    let database = Database::open_in_memory().unwrap();
    let original = database.settings().unwrap();
    for size in [0, 101, u32::MAX] {
        let mut invalid = original.clone();
        invalid.log_max_file_size_mb = size;
        assert!(matches!(
            database.update_settings(&invalid),
            Err(StorageError::InvalidSettings(_))
        ));
    }
    for count in [0, 11, u32::MAX] {
        let mut invalid = original.clone();
        invalid.log_file_count = count;
        assert!(matches!(
            database.update_settings(&invalid),
            Err(StorageError::InvalidSettings(_))
        ));
    }
    for directory in ["", " ", "logs", "../logs", "\0"] {
        let mut invalid = original.clone();
        invalid.log_directory = Some(directory.into());
        assert!(matches!(
            database.update_settings(&invalid),
            Err(StorageError::InvalidSettings(_))
        ));
    }
    let absolute_with_nul = std::env::temp_dir().join("invalid\0logs");
    let mut invalid = original.clone();
    invalid.log_directory = Some(absolute_with_nul.to_str().unwrap().into());
    assert!(matches!(
        database.update_settings(&invalid),
        Err(StorageError::InvalidSettings(_))
    ));
    assert_eq!(database.settings().unwrap(), original);
}

#[test]
fn log_settings_roundtrip_without_creating_the_selected_directory() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("profile.db");
    let directory = temp.path().join("new logs");
    let database = Database::open(&path).unwrap();
    let mut settings = database.settings().unwrap();
    settings.log_max_file_size_mb = 100;
    settings.log_file_count = 10;
    settings.log_directory = Some(directory.to_str().unwrap().into());
    database.update_settings(&settings).unwrap();
    drop(database);
    let database = Database::open(&path).unwrap();
    assert_eq!(database.settings().unwrap(), settings);
    assert!(!directory.exists());

    settings.log_max_file_size_mb = 1;
    settings.log_file_count = 1;
    settings.log_directory = None;
    database.update_settings(&settings).unwrap();
    assert_eq!(database.settings().unwrap(), settings);
}

#[test]
fn json_log_limits_must_be_unsigned_integers() {
    for key in ["logMaxFileSizeMb", "logFileCount"] {
        for value in [
            serde_json::json!(1.5),
            serde_json::json!(-1),
            serde_json::json!("2"),
        ] {
            let mut settings = serde_json::to_value(AppSettings::default()).unwrap();
            settings[key] = value;
            assert!(serde_json::from_value::<AppSettings>(settings).is_err());
        }
    }
}

#[cfg(windows)]
#[test]
fn windows_log_directories_require_a_drive_or_unc_root() {
    let database = Database::open_in_memory().unwrap();
    let mut settings = database.settings().unwrap();
    for directory in [r"\logs", r"C:logs"] {
        settings.log_directory = Some(directory.into());
        assert!(matches!(
            database.update_settings(&settings),
            Err(StorageError::InvalidSettings(_))
        ));
    }
    for directory in [r"C:\logs", r"\\server\share\logs"] {
        settings.log_directory = Some(directory.into());
        database.update_settings(&settings).unwrap();
    }
}
