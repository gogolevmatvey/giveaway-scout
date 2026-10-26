use giveaway_scout_core::{
    Database, GameStore, GiveawayItem, GiveawayKind, GiveawaySource, InitialSyncMode,
};

fn items() -> Vec<GiveawayItem> {
    [GiveawaySource::GiveeClub, GiveawaySource::GamesForFarm]
        .into_iter()
        .flat_map(|source| {
            [GameStore::Steam, GameStore::EpicGames, GameStore::Unknown]
                .into_iter()
                .map(move |store| GiveawayItem {
                    source,
                    source_id: format!("test/{}", store.storage_name()),
                    title: format!("Test {}", store.storage_name()),
                    url: "https://example.com/game".to_owned(),
                    kind: GiveawayKind::Giveaway,
                    store,
                    details: None,
                    legacy_tracking_key_prefixes: Vec::new(),
                })
        })
        .collect()
}

#[test]
fn filters_both_sources_on_first_and_later_checks_without_replaying_skipped_items() {
    for first_check in [false, true] {
        for steam in [false, true] {
            for epic in [false, true] {
                let mut database = Database::open_in_memory().unwrap();
                if !first_check {
                    database
                        .process_discovered_items(&[], &GiveawaySource::ALL)
                        .unwrap();
                }
                let mut settings = database.settings().unwrap();
                settings.initial_sync_mode = Some(InitialSyncMode::NotifyCurrent);
                settings.steam_notifications_enabled = steam;
                settings.epic_games_notifications_enabled = epic;
                database.update_settings(&settings).unwrap();

                let items = items();
                let discovered = database
                    .process_discovered_items(&items, &GiveawaySource::ALL)
                    .unwrap();
                let expected = items
                    .iter()
                    .filter(|item| settings.allows_store(item.store))
                    .cloned()
                    .collect::<Vec<_>>();
                assert_eq!(discovered.new_items, expected);
                assert_eq!(
                    database.counts().unwrap().notification_history,
                    expected.len() as u64
                );
                assert_eq!(database.counts().unwrap().tracked_giveaways, 6);

                settings.steam_notifications_enabled = true;
                settings.epic_games_notifications_enabled = true;
                database.update_settings(&settings).unwrap();
                assert!(
                    database
                        .process_discovered_items(&items, &GiveawaySource::ALL)
                        .unwrap()
                        .new_items
                        .is_empty()
                );
                assert_eq!(database.notifications().unwrap().len(), expected.len());
            }
        }
    }
}

#[test]
fn disabled_stores_pause_pending_delivery_and_preserve_history() {
    let mut database = Database::open_in_memory().unwrap();
    database
        .process_discovered_items(&[], &GiveawaySource::ALL)
        .unwrap();
    database
        .process_discovered_items(&items(), &GiveawaySource::ALL)
        .unwrap();
    let mut settings = database.settings().unwrap();
    settings.steam_notifications_enabled = false;
    settings.epic_games_notifications_enabled = false;
    database.update_settings(&settings).unwrap();
    assert_eq!(database.notifications().unwrap().len(), 6);
    assert_eq!(database.pending_notifications().unwrap().len(), 2);
    settings.steam_notifications_enabled = true;
    database.update_settings(&settings).unwrap();
    assert_eq!(database.pending_notifications().unwrap().len(), 4);
}

#[test]
fn store_preferences_survive_reopening_and_old_schema_migrates_without_history_loss() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("test.db");
    {
        let mut database = Database::open(&path).unwrap();
        database
            .process_discovered_items(&[], &GiveawaySource::ALL)
            .unwrap();
        database
            .process_discovered_items(&items(), &GiveawaySource::ALL)
            .unwrap();
    }
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "ALTER TABLE settings DROP COLUMN steam_notifications_enabled;
             ALTER TABLE settings DROP COLUMN epic_games_notifications_enabled;
             ALTER TABLE notification_history DROP COLUMN store;
             DELETE FROM schema_migrations WHERE version = 4;",
            )
            .unwrap();
    }
    {
        let database = Database::open(&path).unwrap();
        let mut settings = database.settings().unwrap();
        assert!(settings.steam_notifications_enabled && settings.epic_games_notifications_enabled);
        assert_eq!(database.notifications().unwrap().len(), 6);
        assert_eq!(database.pending_notifications().unwrap().len(), 6);
        settings.steam_notifications_enabled = false;
        settings.epic_games_notifications_enabled = false;
        database.update_settings(&settings).unwrap();
    }
    let database = Database::open(&path).unwrap();
    let settings = database.settings().unwrap();
    assert!(!settings.steam_notifications_enabled && !settings.epic_games_notifications_enabled);
    assert_eq!(database.counts().unwrap().tracked_giveaways, 6);
    assert_eq!(database.notifications().unwrap().len(), 6);
}

#[test]
fn switching_initial_mode_after_baseline_does_not_replay_giveaways() {
    let mut database = Database::open_in_memory().unwrap();
    database
        .process_discovered_items(&items(), &GiveawaySource::ALL)
        .unwrap();
    let mut settings = database.settings().unwrap();
    settings.initial_sync_mode = Some(InitialSyncMode::NotifyCurrent);
    database.update_settings(&settings).unwrap();
    assert!(
        database
            .process_discovered_items(&items(), &GiveawaySource::ALL)
            .unwrap()
            .new_items
            .is_empty()
    );
    assert!(database.notifications().unwrap().is_empty());
}
