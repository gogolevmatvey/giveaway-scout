use std::path::Path;

use giveaway_scout_core::{
    AppSettings, Database, GameStore, GiveawayItem, GiveawayKind, GiveawaySource, InitialSyncMode,
    NotificationRecord,
};
use rusqlite::Connection;

fn item(id: &str) -> GiveawayItem {
    GiveawayItem {
        source: GiveawaySource::FreeSteamKeys,
        source_id: id.to_owned(),
        title: format!("Steam game {id}"),
        url: format!("https://www.freesteamkeys.com/game-{id}/"),
        kind: GiveawayKind::Giveaway,
        store: GameStore::Steam,
        details: None,
    }
}

#[test]
fn defaults_and_older_serialized_settings_enable_free_steam_keys() {
    let settings = AppSettings::default();
    assert!(settings.free_steam_keys_enabled);
    assert!(settings.allows_source(GiveawaySource::FreeSteamKeys));
    let mut legacy = serde_json::to_value(&settings).unwrap();
    assert_eq!(legacy["freeSteamKeysEnabled"], true);
    legacy
        .as_object_mut()
        .unwrap()
        .remove("freeSteamKeysEnabled");
    assert!(
        serde_json::from_value::<AppSettings>(legacy)
            .unwrap()
            .free_steam_keys_enabled
    );
    assert_eq!(
        serde_json::to_string(&GiveawaySource::FreeSteamKeys).unwrap(),
        "\"freeSteamKeys\""
    );
    assert_eq!(item("123").tracking_key(), "FreeSteamKeys:123");
}

#[test]
fn source_setting_persists_and_filters_pending_records_without_erasing_history() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("profile.db");
    let mut database = Database::open(&path).unwrap();
    let mut settings = database.settings().unwrap();
    assert!(settings.free_steam_keys_enabled);
    settings.initial_sync_mode = Some(InitialSyncMode::NotifyCurrent);
    database.update_settings(&settings).unwrap();
    database
        .process_discovered_items(&[item("1")], &[GiveawaySource::FreeSteamKeys])
        .unwrap();
    let record = database.notifications().unwrap().pop().unwrap();
    assert_eq!(record.source, "freeSteamKeys");
    assert_eq!(record.store, GameStore::Steam);
    assert!(
        database
            .notification_for_delivery(&record.id)
            .unwrap()
            .is_some()
    );

    settings.free_steam_keys_enabled = false;
    database.update_settings(&settings).unwrap();
    drop(database);
    let database = Database::open(&path).unwrap();
    assert_eq!(database.settings().unwrap(), settings);
    assert!(database.pending_notifications().unwrap().is_empty());
    assert!(
        database
            .notification_for_delivery(&record.id)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        database.notifications().unwrap(),
        std::slice::from_ref(&record)
    );

    settings.free_steam_keys_enabled = true;
    settings.steam_notifications_enabled = false;
    database.update_settings(&settings).unwrap();
    assert!(database.pending_notifications().unwrap().is_empty());
    assert!(
        database
            .notification_for_delivery(&record.id)
            .unwrap()
            .is_none()
    );
    settings.steam_notifications_enabled = true;
    database.update_settings(&settings).unwrap();
    assert_eq!(
        database.pending_notifications().unwrap(),
        std::slice::from_ref(&record)
    );
    assert_eq!(
        database.notification_for_delivery(&record.id).unwrap(),
        Some(record)
    );
}

#[test]
fn disabled_source_and_steam_filters_skip_new_notifications_but_keep_tracking() {
    let mut database = Database::open_in_memory().unwrap();
    let mut settings = database.settings().unwrap();
    settings.initial_sync_mode = Some(InitialSyncMode::NotifyCurrent);
    settings.free_steam_keys_enabled = false;
    database.update_settings(&settings).unwrap();
    let disabled = item("disabled");
    assert!(
        database
            .process_discovered_items(
                std::slice::from_ref(&disabled),
                &[GiveawaySource::FreeSteamKeys]
            )
            .unwrap()
            .new_items
            .is_empty()
    );
    settings.free_steam_keys_enabled = true;
    settings.steam_notifications_enabled = false;
    database.update_settings(&settings).unwrap();
    let filtered = item("filtered");
    assert!(
        database
            .process_discovered_items(
                std::slice::from_ref(&filtered),
                &[GiveawaySource::FreeSteamKeys]
            )
            .unwrap()
            .new_items
            .is_empty()
    );
    settings.steam_notifications_enabled = true;
    database.update_settings(&settings).unwrap();
    let new = item("new");
    assert_eq!(
        database
            .process_discovered_items(
                &[disabled, filtered, new.clone()],
                &[GiveawaySource::FreeSteamKeys]
            )
            .unwrap()
            .new_items,
        [new]
    );
    assert_eq!(database.counts().unwrap().tracked_giveaways, 3);
    assert_eq!(database.counts().unwrap().notification_history, 1);
}

#[test]
fn enabling_a_new_source_takes_one_silent_baseline_then_notifies_new_items_once() {
    let mut database = Database::open_in_memory().unwrap();
    let mut settings = database.settings().unwrap();
    settings.free_steam_keys_enabled = false;
    settings.initial_sync_mode = Some(InitialSyncMode::NotifyCurrent);
    database.update_settings(&settings).unwrap();
    database
        .process_discovered_items(&[], &[GiveawaySource::GiveeClub])
        .unwrap();
    assert!(
        !database
            .source_tracking_initialized(GiveawaySource::FreeSteamKeys)
            .unwrap()
    );
    settings.free_steam_keys_enabled = true;
    database.update_settings(&settings).unwrap();
    let old = item("old");
    assert!(
        database
            .process_discovered_items(std::slice::from_ref(&old), &[GiveawaySource::FreeSteamKeys])
            .unwrap()
            .new_items
            .is_empty()
    );
    assert!(
        database
            .source_tracking_initialized(GiveawaySource::FreeSteamKeys)
            .unwrap()
    );
    let new = item("new");
    let batch = [old, new.clone(), new.clone()];
    assert_eq!(
        database
            .process_discovered_items(&batch, &[GiveawaySource::FreeSteamKeys])
            .unwrap()
            .new_items,
        [new]
    );
    assert!(
        database
            .process_discovered_items(&batch, &[GiveawaySource::FreeSteamKeys])
            .unwrap()
            .new_items
            .is_empty()
    );
}

fn v11_profile(path: &Path) -> (AppSettings, Vec<NotificationRecord>) {
    let mut database = Database::open(path).unwrap();
    let mut settings = database.settings().unwrap();
    settings.check_interval_minutes = 45;
    settings.http_timeout_seconds = 60;
    settings.http_retry_count = 3;
    settings.games_for_farm_enabled = false;
    settings.zeros_enabled = true;
    settings.epic_games_notifications_enabled = false;
    settings.start_in_tray = true;
    settings.log_max_file_size_mb = 7;
    settings.log_file_count = 3;
    settings.language_code = Some("en".into());
    settings.initial_sync_mode = Some(InitialSyncMode::NotifyCurrent);
    database.update_settings(&settings).unwrap();
    let mut old = item("legacy");
    old.source = GiveawaySource::GiveeClub;
    database
        .process_discovered_items(&[old], &[GiveawaySource::GiveeClub])
        .unwrap();
    let record = database.notifications().unwrap().pop().unwrap();
    database.mark_notification_read(&record.id).unwrap();
    let history = database.notifications().unwrap();
    drop(database);
    Connection::open(path)
        .unwrap()
        .execute_batch(
            "ALTER TABLE settings DROP COLUMN free_steam_keys_enabled;
             DELETE FROM schema_migrations WHERE version = 12;",
        )
        .unwrap();
    (settings, history)
}

#[test]
fn v12_migration_preserves_v11_settings_history_tracking_and_source_baselines() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("profile.db");
    let (settings, history) = v11_profile(&path);
    let database = Database::open(&path).unwrap();
    assert_eq!(database.settings().unwrap(), settings);
    assert_eq!(database.notifications().unwrap(), history);
    assert_eq!(database.counts().unwrap().tracked_giveaways, 1);
    assert_eq!(database.counts().unwrap().unread_notifications, 0);
    assert!(
        database
            .source_tracking_initialized(GiveawaySource::GiveeClub)
            .unwrap()
    );
    assert!(
        !database
            .source_tracking_initialized(GiveawaySource::FreeSteamKeys)
            .unwrap()
    );
    drop(database);
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = 12",
                [],
                |row| row.get::<_, u32>(0)
            )
            .unwrap(),
        1
    );
    drop(connection);
    let database = Database::open(&path).unwrap();
    assert_eq!(database.settings().unwrap(), settings);
    assert_eq!(database.notifications().unwrap(), history);
}

#[test]
fn failed_v12_migration_rolls_back_new_column_and_preserves_v11_profile() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("profile.db");
    let (settings, history) = v11_profile(&path);
    Connection::open(&path)
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER fail_v12 BEFORE INSERT ON schema_migrations WHEN NEW.version = 12
             BEGIN SELECT RAISE(FAIL, 'test migration failure'); END;",
        )
        .unwrap();
    assert!(Database::open(&path).is_err());
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection.query_row("SELECT COUNT(*) FROM pragma_table_info('settings') WHERE name = 'free_steam_keys_enabled'", [], |row| row.get::<_, u32>(0)).unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = 12",
                [],
                |row| row.get::<_, u32>(0)
            )
            .unwrap(),
        0
    );
    connection.execute_batch("DROP TRIGGER fail_v12").unwrap();
    drop(connection);
    let database = Database::open(&path).unwrap();
    assert_eq!(database.settings().unwrap(), settings);
    assert_eq!(database.notifications().unwrap(), history);
    assert_eq!(database.counts().unwrap().tracked_giveaways, 1);
}
