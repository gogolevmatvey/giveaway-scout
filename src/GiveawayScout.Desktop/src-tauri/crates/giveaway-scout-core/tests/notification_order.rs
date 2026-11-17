use std::{collections::HashSet, path::Path, sync::Mutex};

use giveaway_scout_core::{
    AppSettings, Database, GameStore, GiveawayItem, GiveawayKind, GiveawaySource, InitialSyncMode,
    NotificationRecord, dispatch_pending_notifications,
};
use rusqlite::Connection;

fn item(source: GiveawaySource, title: &str) -> GiveawayItem {
    GiveawayItem {
        source,
        source_id: title.to_owned(),
        title: title.to_owned(),
        url: format!("https://example.com/{}/{title}", source.storage_name()),
        kind: GiveawayKind::Giveaway,
        store: GameStore::Steam,
        details: None,
    }
}

fn enable_all_sources(database: &Database) {
    let mut settings = database.settings().unwrap();
    settings.zeros_enabled = true;
    database.update_settings(&settings).unwrap();
}

fn initialized_database() -> Database {
    let mut database = Database::open_in_memory().unwrap();
    enable_all_sources(&database);
    database
        .process_discovered_items(&[], &GiveawaySource::ALL)
        .unwrap();
    database
}

fn titles(records: &[NotificationRecord]) -> Vec<&str> {
    records.iter().map(|record| record.title.as_str()).collect()
}

fn ids(records: &[NotificationRecord]) -> Vec<&str> {
    records.iter().map(|record| record.id.as_str()).collect()
}

#[test]
fn same_timestamp_keeps_page_order_in_history_and_reverses_native_requests() {
    let mut database = initialized_database();
    let items: Vec<_> = ["A", "B", "C"]
        .map(|title| item(GiveawaySource::GiveeClub, title))
        .into();
    let discovery = database
        .process_discovered_items(&items, &[GiveawaySource::GiveeClub])
        .unwrap();
    assert_eq!(discovery.new_items, items);
    let history = database.notifications().unwrap();
    assert_eq!(titles(&history), ["A", "B", "C"]);
    assert_eq!(
        history
            .iter()
            .map(|record| &record.received_at_utc)
            .collect::<HashSet<_>>()
            .len(),
        1,
        "the order must not depend on fabricated timestamp differences"
    );
    assert_eq!(
        titles(&database.pending_notifications().unwrap()),
        ["C", "B", "A"]
    );

    let database = Mutex::new(database);
    let mut requested = Vec::new();
    let report = dispatch_pending_notifications(&database, |record, _| {
        requested.push(record.title.clone());
        Ok(())
    })
    .unwrap();
    assert_eq!(requested, ["C", "B", "A"]);
    assert_eq!(report.delivered, 3);
    assert_eq!(
        titles(&database.lock().unwrap().notifications().unwrap()),
        ["A", "B", "C"]
    );
}

#[test]
fn sources_use_fixed_priority_and_preserve_their_own_page_order() {
    let mut database = initialized_database();
    let items = vec![
        item(GiveawaySource::FreeSteamKeys, "K-top"),
        item(GiveawaySource::Zeros, "Z-top"),
        item(GiveawaySource::GamesForFarm, "F-top"),
        item(GiveawaySource::GiveeClub, "G-top"),
        item(GiveawaySource::GamesForFarm, "F-bottom"),
        item(GiveawaySource::GiveeClub, "G-bottom"),
        item(GiveawaySource::Zeros, "Z-bottom"),
        item(GiveawaySource::FreeSteamKeys, "K-bottom"),
    ];
    let discovery = database
        .process_discovered_items(&items, &GiveawaySource::ALL)
        .unwrap();
    assert_eq!(discovery.new_items, items);
    assert_eq!(
        titles(&database.notifications().unwrap()),
        [
            "K-top", "K-bottom", "G-top", "G-bottom", "F-top", "F-bottom", "Z-top", "Z-bottom"
        ]
    );
    assert_eq!(
        titles(&database.pending_notifications().unwrap()),
        [
            "K-bottom", "K-top", "G-bottom", "G-top", "F-bottom", "F-top", "Z-bottom", "Z-top"
        ]
    );
}

#[test]
fn later_batches_win_over_equal_or_reversed_clocks_and_survive_reopening() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("profile.db");
    let mut database = Database::open(&path).unwrap();
    enable_all_sources(&database);
    database
        .process_discovered_items(&[], &GiveawaySource::ALL)
        .unwrap();
    for source in [GiveawaySource::GiveeClub, GiveawaySource::Zeros] {
        let prefix = source.storage_name();
        database
            .process_discovered_items(
                &[
                    item(source, &format!("{prefix}-top")),
                    item(source, &format!("{prefix}-bottom")),
                ],
                &[source],
            )
            .unwrap();
    }
    drop(database);
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "UPDATE notification_history SET received_at_utc =
                CASE discovery_batch WHEN 1 THEN '2026-11-05T00:00:00Z'
                    ELSE '2026-11-04T00:00:00Z' END;",
        )
        .unwrap();
    drop(connection);
    let database = Database::open(&path).unwrap();
    let history = database.notifications().unwrap();
    let pending = database.pending_notifications().unwrap();
    assert_eq!(
        titles(&history),
        [
            "zeros-top",
            "zeros-bottom",
            "giveeClub-top",
            "giveeClub-bottom"
        ]
    );
    assert_eq!(
        titles(&pending),
        [
            "giveeClub-bottom",
            "giveeClub-top",
            "zeros-bottom",
            "zeros-top"
        ]
    );
    drop(database);
    let database = Database::open(&path).unwrap();
    assert_eq!(database.notifications().unwrap(), history);
    assert_eq!(database.pending_notifications().unwrap(), pending);
}

#[test]
fn notify_current_initial_sync_uses_the_same_page_order() {
    let mut database = Database::open_in_memory().unwrap();
    let mut settings = database.settings().unwrap();
    settings.initial_sync_mode = Some(InitialSyncMode::NotifyCurrent);
    database.update_settings(&settings).unwrap();
    let items = ["top", "middle", "bottom"].map(|title| item(GiveawaySource::GiveeClub, title));
    let result = database
        .process_discovered_items(&items, &[GiveawaySource::GiveeClub])
        .unwrap();
    assert!(result.is_initial_synchronization);
    assert_eq!(result.new_items, items);
    assert_eq!(
        titles(&database.notifications().unwrap()),
        ["top", "middle", "bottom"]
    );
    assert_eq!(
        titles(&database.pending_notifications().unwrap()),
        ["bottom", "middle", "top"]
    );
}

#[test]
fn deduplication_known_items_and_disabled_stores_do_not_shuffle_survivors() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("profile.db");
    let mut database = Database::open(&path).unwrap();
    let known = item(GiveawaySource::GiveeClub, "known");
    database
        .process_discovered_items(std::slice::from_ref(&known), &[GiveawaySource::GiveeClub])
        .unwrap();
    let mut settings = database.settings().unwrap();
    settings.gog_notifications_enabled = false;
    database.update_settings(&settings).unwrap();
    let top = item(GiveawaySource::GiveeClub, "top");
    let bottom = item(GiveawaySource::GiveeClub, "bottom");
    let mut duplicate = top.clone();
    duplicate.title = "duplicate must not replace first occurrence".into();
    let mut disabled = item(GiveawaySource::GiveeClub, "disabled GOG");
    disabled.store = GameStore::Gog;
    let items = [known, top.clone(), disabled, duplicate, bottom.clone()];
    let result = database
        .process_discovered_items(&items, &[GiveawaySource::GiveeClub])
        .unwrap();
    assert_eq!(result.new_items, [top, bottom]);
    assert_eq!(
        titles(&database.notifications().unwrap()),
        ["top", "bottom"]
    );
    assert_eq!(
        titles(&database.pending_notifications().unwrap()),
        ["bottom", "top"]
    );
    assert_eq!(database.counts().unwrap().tracked_giveaways, 4);
    settings.gog_notifications_enabled = true;
    database.update_settings(&settings).unwrap();
    assert!(
        database
            .process_discovered_items(&items, &[GiveawaySource::GiveeClub])
            .unwrap()
            .new_items
            .is_empty()
    );
    drop(database);
    let connection = Connection::open(path).unwrap();
    let positions: Vec<i64> = connection
        .prepare("SELECT source_position FROM notification_history ORDER BY source_position")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(positions, [1, 4]);
}

fn v10_profile(path: &Path) -> AppSettings {
    let database = Database::open(path).unwrap();
    let mut settings = database.settings().unwrap();
    settings.zeros_enabled = true;
    settings.check_interval_minutes = 45;
    settings.language_code = Some("en".into());
    settings.log_max_file_size_mb = 7;
    settings.log_file_count = 3;
    database.update_settings(&settings).unwrap();
    drop(database);
    let connection = Connection::open(path).unwrap();
    connection.execute_batch(
        "DROP INDEX IF EXISTS idx_notification_history_page_order;
         ALTER TABLE notification_history DROP COLUMN discovery_batch;
         ALTER TABLE notification_history DROP COLUMN source_position;
         DELETE FROM schema_migrations WHERE version IN (11,13);
         DROP TABLE notification_campaigns;
         DROP TABLE campaign_members;
         DROP TABLE campaigns;
         DROP TABLE redirect_cache;
         DROP TABLE source_status;
         DROP TABLE source_initial_policy;
         INSERT INTO app_metadata(key, value) VALUES
            ('tracking_initialized','true'),
            ('tracking_initialized:giveeClub','true'),
            ('tracking_initialized:gamesForFarm','true'),
            ('tracking_initialized:zeros','true');
         INSERT INTO tracked_giveaways VALUES
            ('GiveeClub:known','giveeClub','known','Known game','2026-11-01T00:00:00Z');
         INSERT INTO notification_history(id,tracking_key,source,kind,title,url,received_at_utc,
            read_at_utc,delivery_state,delivery_attempts,last_delivery_error,store) VALUES
            ('legacy-a','GiveeClub:a','giveeClub','giveaway','Legacy A','https://example.com/a',
                '2026-11-01T01:00:00Z',NULL,'pending',2,'Saved error','gog'),
            ('legacy-b','Zeros:b','zeros','raffle','Legacy B','https://example.com/b',
                '2026-11-01T01:00:00Z','2026-11-01T02:00:00Z','delivered',1,NULL,'steam'),
            ('legacy-c','GamesForFarm:c','gamesForFarm','giveaway','Legacy C','https://example.com/c',
                '2026-11-01T00:00:00Z',NULL,'pending',0,NULL,'steam'),
            ('legacy-d','GamesForFarm:d','gamesForFarm','giveaway','Legacy D','https://example.com/d',
                '2026-11-01T01:00:00Z',NULL,'pending',0,NULL,'epicGames');"
    ).unwrap();
    settings
}

#[test]
fn migration_preserves_legacy_order_ids_read_state_delivery_settings_and_tracking() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("profile.db");
    let settings = v10_profile(&path);
    let mut database = Database::open(&path).unwrap();
    assert_eq!(database.settings().unwrap(), settings);
    assert_eq!(database.counts().unwrap().tracked_giveaways, 1);
    for source in [
        GiveawaySource::GiveeClub,
        GiveawaySource::GamesForFarm,
        GiveawaySource::Zeros,
    ] {
        assert!(database.source_tracking_initialized(source).unwrap());
    }
    assert!(
        !database
            .source_tracking_initialized(GiveawaySource::FreeSteamKeys)
            .unwrap()
    );
    let history = database.notifications().unwrap();
    assert_eq!(
        ids(&history),
        ["legacy-d", "legacy-b", "legacy-a", "legacy-c"]
    );
    assert_eq!(
        ids(&database.pending_notifications().unwrap()),
        ["legacy-c", "legacy-a", "legacy-d"]
    );
    let saved = database.notification("legacy-a").unwrap().unwrap();
    assert_eq!(saved.delivery_attempts, 2);
    assert_eq!(saved.last_delivery_error.as_deref(), Some("Saved error"));
    assert_eq!(saved.store, GameStore::Gog);
    assert_eq!(saved.delivery_state, "pending");
    assert_eq!(saved.received_at_utc, "2026-11-01T01:00:00Z");
    let read = database.notification("legacy-b").unwrap().unwrap();
    assert_eq!(read.read_at_utc.as_deref(), Some("2026-11-01T02:00:00Z"));
    assert_eq!(read.delivery_state, "delivered");
    database
        .process_discovered_items(
            &[
                item(GiveawaySource::Zeros, "new-top"),
                item(GiveawaySource::Zeros, "new-bottom"),
            ],
            &[GiveawaySource::Zeros],
        )
        .unwrap();
    assert_eq!(
        titles(&database.notifications().unwrap()),
        [
            "new-top",
            "new-bottom",
            "Legacy D",
            "Legacy B",
            "Legacy A",
            "Legacy C"
        ]
    );
    assert_eq!(
        titles(&database.pending_notifications().unwrap()),
        ["Legacy C", "Legacy A", "Legacy D", "new-bottom", "new-top"]
    );
    drop(database);
    let connection = Connection::open(&path).unwrap();
    let legacy_rank_count: u32 = connection
        .query_row(
            "SELECT COUNT(*) FROM notification_history WHERE id LIKE 'legacy-%'
                AND discovery_batch IS NULL AND source_position IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(legacy_rank_count, 4);
    let migration_count: u32 = connection
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 11",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(migration_count, 1);
}

#[test]
fn failed_v11_migration_rolls_back_columns_and_leaves_legacy_records_intact() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("profile.db");
    let settings = v10_profile(&path);
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER fail_v11 BEFORE INSERT ON schema_migrations WHEN NEW.version = 11
             BEGIN SELECT RAISE(FAIL, 'test migration failure'); END;",
        )
        .unwrap();
    drop(connection);
    assert!(Database::open(&path).is_err());
    let connection = Connection::open(&path).unwrap();
    let rank_columns: u32 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('notification_history')
                WHERE name IN ('discovery_batch', 'source_position')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rank_columns, 0);
    let legacy: Vec<String> = connection
        .prepare("SELECT id FROM notification_history ORDER BY received_at_utc DESC, id DESC")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(legacy, ["legacy-d", "legacy-b", "legacy-a", "legacy-c"]);
    let migration_count: u32 = connection
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 11",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(migration_count, 0);
    connection.execute_batch("DROP TRIGGER fail_v11").unwrap();
    drop(connection);
    let database = Database::open(&path).unwrap();
    assert_eq!(database.settings().unwrap(), settings);
    assert_eq!(database.notifications().unwrap().len(), 4);
    assert_eq!(
        ids(&database.pending_notifications().unwrap()),
        ["legacy-c", "legacy-a", "legacy-d"]
    );
}

#[test]
fn read_and_disabled_sources_are_skipped_without_reordering_the_remaining_queue() {
    let mut database = initialized_database();
    let items = vec![
        item(GiveawaySource::GiveeClub, "G-top"),
        item(GiveawaySource::GiveeClub, "G-middle"),
        item(GiveawaySource::GiveeClub, "G-bottom"),
        item(GiveawaySource::GamesForFarm, "F-top"),
        item(GiveawaySource::GamesForFarm, "F-bottom"),
    ];
    database
        .process_discovered_items(&items, &GiveawaySource::ALL)
        .unwrap();
    let history = database.notifications().unwrap();
    database.mark_notification_read(&history[1].id).unwrap();
    let mut settings = database.settings().unwrap();
    settings.givee_club_enabled = false;
    database.update_settings(&settings).unwrap();
    assert_eq!(
        titles(&database.pending_notifications().unwrap()),
        ["F-bottom", "F-top"]
    );
    settings.givee_club_enabled = true;
    database.update_settings(&settings).unwrap();
    assert_eq!(
        titles(&database.pending_notifications().unwrap()),
        ["G-bottom", "G-top", "F-bottom", "F-top"]
    );
}

#[test]
fn delivery_failure_does_not_block_later_cards_and_retry_keeps_the_original_record() {
    let mut database = initialized_database();
    let items = ["top", "middle", "bottom"].map(|title| item(GiveawaySource::GiveeClub, title));
    database
        .process_discovered_items(&items, &[GiveawaySource::GiveeClub])
        .unwrap();
    let database = Mutex::new(database);
    let mut requested = Vec::new();
    let mut failed_id = String::new();
    let report = dispatch_pending_notifications(&database, |record, _| {
        requested.push(record.title.clone());
        if record.title == "bottom" {
            failed_id = record.id.clone();
            Err("native transport unavailable".into())
        } else {
            Ok(())
        }
    })
    .unwrap();
    assert_eq!(requested, ["bottom", "middle", "top"]);
    assert_eq!(report.failed, 1);
    assert_eq!(report.delivered, 2);
    let retry = dispatch_pending_notifications(&database, |record, _| {
        assert_eq!(record.id, failed_id);
        assert_eq!(record.title, "bottom");
        Ok(())
    })
    .unwrap();
    assert_eq!(retry.delivered, 1);
    assert_eq!(
        titles(&database.lock().unwrap().notifications().unwrap()),
        ["top", "middle", "bottom"]
    );
}
