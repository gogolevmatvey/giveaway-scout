use giveaway_scout_core::{
    AppSettings, AutostartTarget, Database, GiveawaySource, InitialSyncMode, SourceCheckIntervals,
    StorageError, parse_givee_club,
};
use rusqlite::{Connection, params};
use std::path::Path;

#[test]
fn defaults_and_older_settings_payloads_use_one_hour_per_source() {
    let defaults = AppSettings::default();
    assert_eq!(defaults.check_interval_minutes, 60);
    for source in GiveawaySource::ALL {
        assert_eq!(defaults.check_interval_for(source), 60);
    }
    let mut old_payload = serde_json::to_value(&defaults).unwrap();
    old_payload
        .as_object_mut()
        .unwrap()
        .remove("sourceCheckIntervals");
    old_payload["checkIntervalMinutes"] = 15.into();
    let restored: AppSettings = serde_json::from_value(old_payload).unwrap();
    assert_eq!(restored.check_interval_minutes, 15);
    assert_eq!(
        restored.source_check_intervals,
        SourceCheckIntervals::default()
    );

    let partial: SourceCheckIntervals =
        serde_json::from_str(r#"{"freeSteamKeys":5,"zeros":1440}"#).unwrap();
    assert_eq!(partial.free_steam_keys, 5);
    assert_eq!(partial.givee_club, 60);
    assert_eq!(partial.games_for_farm, 60);
    assert_eq!(partial.zeros, 1440);
}

#[test]
fn a_new_database_keeps_the_one_hour_default() {
    let database = Database::open_in_memory().unwrap();
    assert_eq!(database.settings().unwrap(), AppSettings::default());
}

fn shared_intervals(minutes: u32) -> SourceCheckIntervals {
    SourceCheckIntervals {
        free_steam_keys: minutes,
        givee_club: minutes,
        games_for_farm: minutes,
        zeros: minutes,
    }
}

#[test]
fn independent_intervals_survive_restart_with_disabled_sources() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source-intervals.db");
    let database = Database::open(&path).unwrap();
    let mut settings = database.settings().unwrap();
    settings.source_check_intervals = SourceCheckIntervals {
        free_steam_keys: 5,
        givee_club: 120,
        games_for_farm: 360,
        zeros: 1_440,
    };
    settings.givee_club_enabled = false;
    settings.zeros_enabled = false;
    // The retained setting must not override the individual intervals.
    settings.check_interval_minutes = 17;
    database.update_settings(&settings).unwrap();
    drop(database);

    let mut database = Database::open(&path).unwrap();
    assert_eq!(database.settings().unwrap(), settings);
    let serialized = serde_json::to_value(database.settings().unwrap()).unwrap();
    assert_eq!(
        serialized["sourceCheckIntervals"],
        serde_json::json!({
            "freeSteamKeys": 5,
            "giveeClub": 120,
            "gamesForFarm": 360,
            "zeros": 1440
        })
    );
    database.clear_giveaway_data().unwrap();
    assert_eq!(database.settings().unwrap(), settings);
}

#[test]
fn rejects_each_invalid_interval_without_saving_other_changes() {
    let database = Database::open_in_memory().unwrap();
    let saved = database.settings().unwrap();
    for source in GiveawaySource::ALL {
        for invalid in [0, 4, 1_441, u32::MAX] {
            let mut settings = saved.clone();
            settings.notification_sound_enabled = !saved.notification_sound_enabled;
            let interval = match source {
                GiveawaySource::FreeSteamKeys => {
                    &mut settings.source_check_intervals.free_steam_keys
                }
                GiveawaySource::GiveeClub => &mut settings.source_check_intervals.givee_club,
                GiveawaySource::GamesForFarm => &mut settings.source_check_intervals.games_for_farm,
                GiveawaySource::Zeros => &mut settings.source_check_intervals.zeros,
            };
            *interval = invalid;
            assert!(matches!(
                database.update_settings(&settings),
                Err(StorageError::InvalidSettings(_))
            ));
            assert_eq!(database.settings().unwrap(), saved);
        }
    }
}

fn v13_profile(path: &Path) -> AppSettings {
    let mut database = Database::open(path).unwrap();
    let mut settings = database.settings().unwrap();
    settings.check_interval_minutes = 23;
    settings.http_timeout_seconds = 77;
    settings.http_retry_count = 4;
    settings.games_for_farm_enabled = false;
    settings.free_steam_keys_enabled = false;
    settings.epic_games_notifications_enabled = false;
    settings.notification_sound_enabled = false;
    settings.start_at_login = true;
    settings.start_in_tray = true;
    settings.keep_running_in_tray = true;
    settings.language_code = Some("en".to_owned());
    settings.verbose_logging = true;
    settings.log_max_file_size_mb = 7;
    settings.log_file_count = 3;
    settings.initial_sync_mode = Some(InitialSyncMode::NotifyCurrent);
    database.update_settings(&settings).unwrap();
    let items = parse_givee_club(include_str!("fixtures/html/givee-club.html")).unwrap();
    database
        .process_discovered_items(&items, &[GiveawaySource::GiveeClub])
        .unwrap();
    let history = database.notifications().unwrap();
    assert!(!history.is_empty());
    database.mark_notification_read(&history[0].id).unwrap();
    database
        .record_notification_delivered(&history[0].id)
        .unwrap();
    drop(database);

    let connection = Connection::open(path).unwrap();
    connection
        .execute_batch(
            "ALTER TABLE settings DROP COLUMN source_check_intervals_json;
             DELETE FROM schema_migrations WHERE version = 14;",
        )
        .unwrap();
    settings.source_check_intervals = shared_intervals(settings.check_interval_minutes);
    settings
}

fn raw_history(connection: &Connection) -> Vec<(String, Option<String>, Option<String>, String)> {
    connection
        .prepare("SELECT id,tracking_key,read_at_utc,delivery_state FROM notification_history ORDER BY rowid")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

#[test]
fn migration_from_v13_preserves_the_shared_interval_and_existing_profile() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("migration.db");
    let settings = v13_profile(&path);
    let before = raw_history(&Connection::open(&path).unwrap());

    let database = Database::open(&path).unwrap();
    assert_eq!(database.settings().unwrap(), settings);
    for source in GiveawaySource::ALL {
        assert_eq!(database.settings().unwrap().check_interval_for(source), 23);
    }
    for (id, tracking_key, read_at, delivery_state) in &before {
        let restored = database.notification(id).unwrap().unwrap();
        assert_eq!(&restored.tracking_key, tracking_key);
        assert_eq!(&restored.read_at_utc, read_at);
        assert_eq!(&restored.delivery_state, delivery_state);
    }
    drop(database);
    assert_eq!(Database::open(&path).unwrap().settings().unwrap(), settings);

    let connection = Connection::open(&path).unwrap();
    assert_eq!(raw_history(&connection), before);
    assert_eq!(
        connection
            .query_row(
                "SELECT check_interval_minutes FROM settings WHERE id = 1",
                [],
                |row| row.get::<_, u32>(0),
            )
            .unwrap(),
        23
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = 14",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}

#[test]
fn failed_v14_migration_rolls_back_the_new_column_and_preserves_history() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rollback.db");
    let settings = v13_profile(&path);
    let connection = Connection::open(&path).unwrap();
    let before = raw_history(&connection);
    connection
        .execute_batch(
            "CREATE TRIGGER fail_v14 BEFORE INSERT ON schema_migrations WHEN NEW.version = 14
             BEGIN SELECT RAISE(ABORT,'fixture v14 failure'); END;",
        )
        .unwrap();
    drop(connection);

    assert!(Database::open(&path).is_err());
    let connection = Connection::open(&path).unwrap();
    assert_eq!(raw_history(&connection), before);
    assert_eq!(
        connection
            .query_row(
                "SELECT check_interval_minutes FROM settings WHERE id = 1",
                [],
                |row| row.get::<_, u32>(0),
            )
            .unwrap(),
        23
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('settings') WHERE name = 'source_check_intervals_json'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = 14",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    connection.execute_batch("DROP TRIGGER fail_v14;").unwrap();
    drop(connection);
    assert_eq!(Database::open(&path).unwrap().settings().unwrap(), settings);
}

#[test]
fn existing_v14_and_v15_intervals_are_not_reinterpreted_from_the_legacy_value() {
    for version in [14, 15] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("existing-intervals.db");
        let database = Database::open(&path).unwrap();
        let mut settings = database.settings().unwrap();
        settings.check_interval_minutes = 45;
        settings.source_check_intervals = shared_intervals(60);
        settings.start_at_login = true;
        if version == 15 {
            settings.autostart_target = AutostartTarget::Cli;
        }
        database.update_settings(&settings).unwrap();
        drop(database);
        if version == 14 {
            Connection::open(&path)
                .unwrap()
                .execute_batch(
                    "ALTER TABLE settings DROP COLUMN autostart_target;
                     DELETE FROM schema_migrations WHERE version = 15;",
                )
                .unwrap();
        }
        for _ in 0..2 {
            let database = Database::open(&path).unwrap();
            assert_eq!(database.settings().unwrap(), settings);
        }
    }
}

fn raw_rows(connection: &Connection, sql: &str) -> Vec<Vec<rusqlite::types::Value>> {
    let mut statement = connection.prepare(sql).unwrap();
    let column_count = statement.column_count();
    statement
        .query_map([], |row| {
            (0..column_count)
                .map(|index| row.get(index))
                .collect::<Result<Vec<_>, _>>()
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn published_v110_profile(path: &Path, interval: u32) -> AppSettings {
    // This fixture uses the published DDL, rather than downgrading a database
    // created by today's initializer or touching an installed user profile.
    let connection = Connection::open(path).unwrap();
    connection
        .execute_batch(include_str!("fixtures/storage/v1.1.0-schema9.sql"))
        .unwrap();
    let timestamp = "2026-10-31T12:34:56Z";
    for version in 1..=9 {
        connection
            .execute(
                "INSERT INTO schema_migrations(version,applied_at_utc) VALUES(?1,?2)",
                params![version, timestamp],
            )
            .unwrap();
    }
    let autostart_enabled = interval != 5;
    connection
        .execute(
            "INSERT INTO settings(
                id,check_interval_minutes,http_timeout_seconds,http_retry_count,
                givee_club_enabled,games_for_farm_enabled,zeros_enabled,
                steam_notifications_enabled,epic_games_notifications_enabled,gog_notifications_enabled,
                notification_sound_enabled,start_at_login,start_in_tray,keep_running_in_tray,
                language_code,verbose_logging,initial_sync_mode,updated_at_utc
            ) VALUES(1,?1,77,4,1,0,1,1,0,0,0,?2,1,1,'en',1,'notifyCurrent',?3)",
            params![interval, autostart_enabled, timestamp],
        )
        .unwrap();
    connection
        .execute_batch(
            "INSERT INTO app_metadata(key,value) VALUES
                ('tracking_initialized','true'),
                ('tracking_initialized:giveeClub','true');
             INSERT INTO usage_statistics_state(id,installation_id,last_attempt_at,last_success_day)
                VALUES(1,'fixture-installation',1793448000,'2026-10-31');",
        )
        .unwrap();
    let items = parse_givee_club(include_str!("fixtures/html/givee-club.html")).unwrap();
    assert_eq!(items.len(), 2);
    for (index, item) in items.iter().enumerate() {
        connection
            .execute(
                "INSERT INTO tracked_giveaways(tracking_key,source,source_id,title,first_seen_utc)
                 VALUES(?1,?2,?3,?4,?5)",
                params![
                    item.tracking_key(),
                    item.source.storage_name(),
                    item.source_id,
                    item.title,
                    timestamp,
                ],
            )
            .unwrap();
        let (read_at, delivery_state, delivery_error) = if index == 0 {
            (Some(timestamp), "delivered", None)
        } else {
            (None, "pending", Some("fixture delivery failure"))
        };
        connection
            .execute(
                "INSERT INTO notification_history(
                    id,tracking_key,source,kind,title,url,received_at_utc,read_at_utc,
                    delivery_state,delivery_attempts,last_delivery_error,store
                 ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,2,?10,?11)",
                params![
                    format!("fixture-history-{index}"),
                    item.tracking_key(),
                    item.source.storage_name(),
                    item.kind.storage_name(),
                    item.title,
                    item.url,
                    timestamp,
                    read_at,
                    delivery_state,
                    delivery_error,
                    item.store.storage_name(),
                ],
            )
            .unwrap();
    }
    AppSettings {
        check_interval_minutes: interval,
        source_check_intervals: shared_intervals(interval),
        http_timeout_seconds: 77,
        http_retry_count: 4,
        games_for_farm_enabled: false,
        zeros_enabled: true,
        epic_games_notifications_enabled: false,
        gog_notifications_enabled: false,
        notification_sound_enabled: false,
        start_at_login: autostart_enabled,
        start_in_tray: true,
        keep_running_in_tray: true,
        language_code: Some("en".to_owned()),
        verbose_logging: true,
        initial_sync_mode: Some(InitialSyncMode::NotifyCurrent),
        ..AppSettings::default()
    }
}

#[test]
fn published_v110_schema9_migrates_intervals_without_losing_or_repeating_history() {
    const HISTORY: &str = "SELECT id,tracking_key,source,kind,title,url,received_at_utc,
        read_at_utc,delivery_state,delivery_attempts,last_delivery_error,store
        FROM notification_history ORDER BY rowid";
    for interval in [5, 45, 1_440] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("published-v1.1.0.db");
        let expected_settings = published_v110_profile(&path, interval);
        let connection = Connection::open(&path).unwrap();
        let history = raw_rows(&connection, HISTORY);
        let tracked = raw_rows(
            &connection,
            "SELECT * FROM tracked_giveaways ORDER BY rowid",
        );
        let statistics = raw_rows(&connection, "SELECT * FROM usage_statistics_state");
        assert_eq!(
            connection
                .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            9
        );
        drop(connection);

        for _ in 0..2 {
            let mut database = Database::open(&path).unwrap();
            assert_eq!(database.settings().unwrap(), expected_settings);
            assert_eq!(database.notifications().unwrap().len(), 2);
            assert_eq!(database.pending_notifications().unwrap().len(), 1);
            assert!(database.tracking_initialized().unwrap());
            assert!(
                database
                    .source_tracking_initialized(GiveawaySource::GiveeClub)
                    .unwrap()
            );
            let items = parse_givee_club(include_str!("fixtures/html/givee-club.html")).unwrap();
            let result = database
                .process_discovered_items(&items, &[GiveawaySource::GiveeClub])
                .unwrap();
            assert!(!result.is_initial_synchronization);
            assert!(result.new_items.is_empty());
            assert_eq!(database.notifications().unwrap().len(), 2);
            drop(database);

            let connection = Connection::open(&path).unwrap();
            assert_eq!(raw_rows(&connection, HISTORY), history);
            assert_eq!(
                raw_rows(
                    &connection,
                    "SELECT * FROM tracked_giveaways ORDER BY rowid"
                ),
                tracked
            );
            assert_eq!(
                raw_rows(&connection, "SELECT * FROM usage_statistics_state"),
                statistics
            );
            assert_eq!(
                connection
                    .query_row(
                        "SELECT COUNT(*),MAX(version) FROM schema_migrations",
                        [],
                        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                    )
                    .unwrap(),
                (15, 15)
            );
        }
    }
}
