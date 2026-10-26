use giveaway_scout_core::{
    DataCounts, Database, GameStore, GiveawayItem, GiveawayKind, GiveawaySource, InitialSyncMode,
    SourceCheckResult, SourceFailure, StorageError, apply_source_check,
};
use rusqlite::Connection;

fn item(source: GiveawaySource, id: u32) -> GiveawayItem {
    let (source_id, url) = match source {
        GiveawaySource::GiveeClub => (
            format!("event/{id}"),
            format!("https://givee.club/event/{id}"),
        ),
        GiveawaySource::GamesForFarm => (
            format!("/game?n={id}"),
            format!("https://freeanywhere.net/game?n={id}"),
        ),
        GiveawaySource::Zeros => (
            format!("period_{id}"),
            "http://zeros.group/free/".to_owned(),
        ),
    };
    GiveawayItem {
        source,
        source_id,
        url,
        title: format!("Game {id}"),
        kind: GiveawayKind::Giveaway,
        store: GameStore::Steam,
        details: None,
        legacy_tracking_key_prefixes: Vec::new(),
    }
}

fn downgrade_to_global_only_v4(path: &std::path::Path) {
    Connection::open(path)
        .unwrap()
        .execute_batch(
            "DELETE FROM schema_migrations WHERE version = 5;
         DELETE FROM app_metadata WHERE key IN (
           'tracking_initialized:giveeClub', 'tracking_initialized:gamesForFarm', 'tracking_initialized:zeros');",
        )
        .unwrap();
}

#[test]
fn first_check_of_each_source_is_independent_and_later_new_games_notify_once() {
    for mode in [InitialSyncMode::Silent, InitialSyncMode::NotifyCurrent] {
        for first in GiveawaySource::ALL {
            let second = GiveawaySource::ALL
                .into_iter()
                .find(|s| *s != first)
                .unwrap();
            let mut database = Database::open_in_memory().unwrap();
            let mut settings = database.settings().unwrap();
            settings.zeros_enabled = true;
            settings.initial_sync_mode = Some(mode);
            database.update_settings(&settings).unwrap();
            let first_item = item(first, 1);
            let first_result = database
                .process_discovered_items(std::slice::from_ref(&first_item), &[first])
                .unwrap();
            assert_eq!(
                first_result.new_items.len(),
                usize::from(mode == InitialSyncMode::NotifyCurrent)
            );
            let old_history = database.notifications().unwrap();
            let second_item = item(second, 1);
            let baseline = database
                .process_discovered_items(&[first_item, second_item.clone()], &GiveawaySource::ALL)
                .unwrap();
            assert!(baseline.new_items.is_empty());
            assert_eq!(database.notifications().unwrap(), old_history);
            assert!(database.source_tracking_initialized(second).unwrap());
            let new_item = item(second, 2);
            let batch = [second_item, new_item.clone(), new_item.clone()];
            let later = database
                .process_discovered_items(&batch, &[second])
                .unwrap();
            assert_eq!(later.new_items, [new_item]);
            assert!(
                database
                    .process_discovered_items(&batch, &[second])
                    .unwrap()
                    .new_items
                    .is_empty()
            );
            assert_eq!(
                database.notifications().unwrap().len(),
                old_history.len() + 1
            );
        }
    }
}

#[test]
fn previously_checked_source_retains_its_baseline_when_disabled_and_enabled_again() {
    let mut database = Database::open_in_memory().unwrap();
    let old = item(GiveawaySource::GiveeClub, 1);
    database
        .process_discovered_items(std::slice::from_ref(&old), &GiveawaySource::ALL)
        .unwrap();
    let mut settings = database.settings().unwrap();
    settings.givee_club_enabled = false;
    database.update_settings(&settings).unwrap();
    database
        .process_discovered_items(&[], &[GiveawaySource::GamesForFarm])
        .unwrap();
    settings.givee_club_enabled = true;
    database.update_settings(&settings).unwrap();
    let new_item = item(GiveawaySource::GiveeClub, 2);
    let result = database
        .process_discovered_items(&[old, new_item.clone()], &[GiveawaySource::GiveeClub])
        .unwrap();
    assert_eq!(result.new_items, [new_item]);
    assert_eq!(database.counts().unwrap().notification_history, 1);
}

#[test]
fn verified_empty_source_is_initialized_even_without_tracked_items() {
    let mut database = Database::open_in_memory().unwrap();
    database
        .process_discovered_items(&[], &[GiveawaySource::GiveeClub])
        .unwrap();
    assert!(database.tracking_initialized().unwrap());
    assert!(
        database
            .source_tracking_initialized(GiveawaySource::GiveeClub)
            .unwrap()
    );
    assert!(
        !database
            .source_tracking_initialized(GiveawaySource::GamesForFarm)
            .unwrap()
    );
    assert_eq!(
        database
            .process_discovered_items(
                &[item(GiveawaySource::GiveeClub, 1)],
                &[GiveawaySource::GiveeClub]
            )
            .unwrap()
            .new_items
            .len(),
        1
    );
    database
        .process_discovered_items(&[], &[GiveawaySource::GamesForFarm])
        .unwrap();
    assert_eq!(
        database
            .process_discovered_items(
                &[item(GiveawaySource::GamesForFarm, 1)],
                &[GiveawaySource::GamesForFarm]
            )
            .unwrap()
            .new_items
            .len(),
        1
    );
}

#[test]
fn empty_or_inconsistent_success_set_cannot_initialize_or_partially_write_data() {
    let mut database = Database::open_in_memory().unwrap();
    database.process_discovered_items(&[], &[]).unwrap();
    for sources in [&[][..], &[GiveawaySource::GamesForFarm][..]] {
        assert!(matches!(
            database.process_discovered_items(&[item(GiveawaySource::GiveeClub, 1)], sources),
            Err(StorageError::InvalidDiscoveryBatch)
        ));
        assert_eq!(database.counts().unwrap(), DataCounts::default());
        assert!(!database.tracking_initialized().unwrap());
        for source in GiveawaySource::ALL {
            assert!(!database.source_tracking_initialized(source).unwrap());
        }
    }
}

#[test]
fn source_baselines_survive_reopening_and_cleanup_resets_all_sources_silently() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("test.db");
    let mut database = Database::open(&path).unwrap();
    let mut settings = database.settings().unwrap();
    settings.initial_sync_mode = Some(InitialSyncMode::NotifyCurrent);
    database.update_settings(&settings).unwrap();
    let items: Vec<_> = GiveawaySource::ALL
        .into_iter()
        .map(|s| item(s, 1))
        .collect();
    database
        .process_discovered_items(&items, &GiveawaySource::ALL)
        .unwrap();
    let notification = database.notifications().unwrap()[0].id.clone();
    database.mark_notification_read(&notification).unwrap();
    let history = database.notifications().unwrap();
    drop(database);
    let mut database = Database::open(&path).unwrap();
    assert_eq!(database.notifications().unwrap(), history);
    for source in GiveawaySource::ALL {
        assert!(database.source_tracking_initialized(source).unwrap());
    }
    database.clear_giveaway_data().unwrap();
    assert_eq!(database.settings().unwrap(), settings);
    assert_eq!(database.counts().unwrap(), DataCounts::default());
    assert!(!database.tracking_initialized().unwrap());
    for source in GiveawaySource::ALL {
        assert!(!database.source_tracking_initialized(source).unwrap());
        assert!(
            database
                .process_discovered_items(&[item(source, 1)], &[source])
                .unwrap()
                .new_items
                .is_empty()
        );
    }
    assert_eq!(database.counts().unwrap().notification_history, 0);
    assert_eq!(
        database
            .process_discovered_items(
                &[item(GiveawaySource::GiveeClub, 2)],
                &[GiveawaySource::GiveeClub]
            )
            .unwrap()
            .new_items
            .len(),
        1
    );
}

#[test]
fn v4_migration_uses_real_source_evidence_and_preserves_history_settings_and_read_state() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("test.db");
    let mut database = Database::open(&path).unwrap();
    let mut settings = database.settings().unwrap();
    settings.initial_sync_mode = Some(InitialSyncMode::NotifyCurrent);
    database.update_settings(&settings).unwrap();
    database
        .process_discovered_items(
            &[
                item(GiveawaySource::GiveeClub, 1),
                item(GiveawaySource::GiveeClub, 2),
            ],
            &[GiveawaySource::GiveeClub],
        )
        .unwrap();
    database
        .mark_notification_read(&database.notifications().unwrap()[0].id)
        .unwrap();
    let history = database.notifications().unwrap();
    let counts = database.counts().unwrap();
    drop(database);
    downgrade_to_global_only_v4(&path);
    let mut database = Database::open(&path).unwrap();
    assert_eq!(database.settings().unwrap(), settings);
    assert_eq!(database.notifications().unwrap(), history);
    assert_eq!(database.counts().unwrap(), counts);
    assert!(
        database
            .source_tracking_initialized(GiveawaySource::GiveeClub)
            .unwrap()
    );
    assert!(
        !database
            .source_tracking_initialized(GiveawaySource::GamesForFarm)
            .unwrap()
    );
    assert_eq!(
        database
            .process_discovered_items(
                &[item(GiveawaySource::GiveeClub, 3)],
                &[GiveawaySource::GiveeClub]
            )
            .unwrap()
            .new_items
            .len(),
        1
    );
    assert!(
        database
            .process_discovered_items(
                &[item(GiveawaySource::GamesForFarm, 1)],
                &[GiveawaySource::GamesForFarm]
            )
            .unwrap()
            .new_items
            .is_empty()
    );
    drop(database);
    let database = Database::open(&path).unwrap();
    assert_eq!(database.notifications().unwrap().len(), history.len() + 1);
    for source in [GiveawaySource::GiveeClub, GiveawaySource::GamesForFarm] {
        assert!(database.source_tracking_initialized(source).unwrap());
    }
}

#[test]
fn v4_global_flag_without_source_evidence_causes_one_conservative_silent_baseline() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("test.db");
    let mut database = Database::open(&path).unwrap();
    database
        .process_discovered_items(&[], &GiveawaySource::ALL)
        .unwrap();
    drop(database);
    downgrade_to_global_only_v4(&path);
    let mut database = Database::open(&path).unwrap();
    assert!(database.tracking_initialized().unwrap());
    for source in GiveawaySource::ALL {
        assert!(!database.source_tracking_initialized(source).unwrap());
    }
    let items: Vec<_> = GiveawaySource::ALL
        .into_iter()
        .map(|s| item(s, 1))
        .collect();
    assert!(
        database
            .process_discovered_items(&items, &GiveawaySource::ALL)
            .unwrap()
            .new_items
            .is_empty()
    );
    assert_eq!(database.counts().unwrap().notification_history, 0);
    assert_eq!(
        database
            .process_discovered_items(
                &[item(GiveawaySource::GiveeClub, 2)],
                &[GiveawaySource::GiveeClub]
            )
            .unwrap()
            .new_items
            .len(),
        1
    );
}

#[test]
fn metadata_write_failure_rolls_back_items_notifications_and_all_baseline_flags() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("test.db");
    let mut database = Database::open(&path).unwrap();
    let mut settings = database.settings().unwrap();
    settings.initial_sync_mode = Some(InitialSyncMode::NotifyCurrent);
    database.update_settings(&settings).unwrap();
    Connection::open(&path)
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER reject_test_baseline BEFORE INSERT ON app_metadata
         WHEN NEW.key = 'tracking_initialized:gamesForFarm'
         BEGIN SELECT RAISE(ABORT, 'fixture baseline write failure'); END;",
        )
        .unwrap();
    let items: Vec<_> = GiveawaySource::ALL
        .into_iter()
        .map(|s| item(s, 1))
        .collect();
    assert!(
        database
            .process_discovered_items(&items, &GiveawaySource::ALL)
            .is_err()
    );
    assert_eq!(database.counts().unwrap(), DataCounts::default());
    assert!(!database.tracking_initialized().unwrap());
    for source in GiveawaySource::ALL {
        assert!(!database.source_tracking_initialized(source).unwrap());
    }
    Connection::open(&path)
        .unwrap()
        .execute_batch("DROP TRIGGER reject_test_baseline")
        .unwrap();
    assert_eq!(
        database
            .process_discovered_items(&items, &GiveawaySource::ALL)
            .unwrap()
            .new_items
            .len(),
        2
    );
}

#[test]
fn failed_new_source_does_not_gain_a_baseline_or_block_new_items_from_a_known_source() {
    for known in GiveawaySource::ALL {
        let added = GiveawaySource::ALL
            .into_iter()
            .find(|s| *s != known)
            .unwrap();
        let mut database = Database::open_in_memory().unwrap();
        let mut settings = database.settings().unwrap();
        settings.zeros_enabled = true;
        database.update_settings(&settings).unwrap();
        database
            .process_discovered_items(&[item(known, 1)], &[known])
            .unwrap();
        let failure = SourceFailure {
            source: added,
            message: "fixture fetch failure".to_owned(),
            reason: Default::default(),
        };
        apply_source_check(
            &mut database,
            SourceCheckResult {
                checked_sources: vec![added],
                items: Vec::new(),
                failures: vec![failure.clone()],
            },
        )
        .unwrap();
        assert!(!database.source_tracking_initialized(added).unwrap());
        let new_item = item(known, 2);
        let result = apply_source_check(
            &mut database,
            SourceCheckResult {
                checked_sources: GiveawaySource::ALL.to_vec(),
                items: vec![new_item.clone()],
                failures: vec![failure],
            },
        )
        .unwrap();
        assert_eq!(result.new_items, [new_item]);
        assert!(!database.source_tracking_initialized(added).unwrap());
        let restored = apply_source_check(
            &mut database,
            SourceCheckResult {
                checked_sources: GiveawaySource::ALL.to_vec(),
                items: vec![item(known, 1), item(added, 1)],
                failures: Vec::new(),
            },
        )
        .unwrap();
        assert!(restored.new_items.is_empty());
        assert!(database.source_tracking_initialized(added).unwrap());
        assert_eq!(database.counts().unwrap().notification_history, 1);
    }
}

#[test]
fn failed_v5_migration_rolls_back_metadata_without_recovering_or_erasing_a_healthy_profile() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("test.db");
    let mut database = Database::open(&path).unwrap();
    database
        .process_discovered_items(
            &[item(GiveawaySource::GiveeClub, 1)],
            &[GiveawaySource::GiveeClub],
        )
        .unwrap();
    database
        .process_discovered_items(
            &[item(GiveawaySource::GiveeClub, 2)],
            &[GiveawaySource::GiveeClub],
        )
        .unwrap();
    let history = database.notifications().unwrap();
    let counts = database.counts().unwrap();
    drop(database);
    downgrade_to_global_only_v4(&path);
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER reject_test_migration BEFORE INSERT ON app_metadata
         WHEN NEW.key = 'tracking_initialized:giveeClub'
         BEGIN SELECT RAISE(ABORT, 'fixture migration failure'); END;",
        )
        .unwrap();
    drop(connection);
    assert!(Database::open_with_recovery(&path).is_err());
    let connection = Connection::open(&path).unwrap();
    let migration_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 5",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let source_flags: i64 = connection.query_row("SELECT COUNT(*) FROM app_metadata WHERE key IN ('tracking_initialized:giveeClub', 'tracking_initialized:gamesForFarm')", [], |row| row.get(0)).unwrap();
    assert_eq!(migration_count, 0);
    assert_eq!(source_flags, 0);
    assert!(std::fs::read_dir(temporary.path()).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("corrupt")
    }));
    connection
        .execute_batch("DROP TRIGGER reject_test_migration")
        .unwrap();
    drop(connection);
    let database = Database::open(&path).unwrap();
    assert_eq!(database.counts().unwrap(), counts);
    assert_eq!(database.notifications().unwrap(), history);
    assert!(
        database
            .source_tracking_initialized(GiveawaySource::GiveeClub)
            .unwrap()
    );
}
