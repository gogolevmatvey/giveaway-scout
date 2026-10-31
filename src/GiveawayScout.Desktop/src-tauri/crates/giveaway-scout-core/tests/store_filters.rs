use giveaway_scout_core::{
    Database, GameStore, GiveawayItem, GiveawayKind, GiveawaySource, InitialSyncMode,
};

fn items() -> Vec<GiveawayItem> {
    [GiveawaySource::GiveeClub, GiveawaySource::GamesForFarm]
        .into_iter()
        .flat_map(|source| {
            [
                GameStore::Steam,
                GameStore::EpicGames,
                GameStore::Gog,
                GameStore::Unknown,
            ]
            .into_iter()
            .map(move |store| GiveawayItem {
                source,
                source_id: format!("test/{}", store.storage_name()),
                title: format!("Test {}", store.storage_name()),
                url: "https://example.com/game".to_owned(),
                kind: GiveawayKind::Giveaway,
                store,
                details: None,
            })
        })
        .collect()
}

#[test]
fn filters_both_sources_on_first_and_later_checks_without_replaying_skipped_items() {
    for first_check in [false, true] {
        for steam in [false, true] {
            for epic in [false, true] {
                for gog in [false, true] {
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
                    settings.gog_notifications_enabled = gog;
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
                    assert_eq!(database.counts().unwrap().tracked_giveaways, 8);

                    settings.steam_notifications_enabled = true;
                    settings.epic_games_notifications_enabled = true;
                    settings.gog_notifications_enabled = true;
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
    settings.gog_notifications_enabled = false;
    database.update_settings(&settings).unwrap();
    assert_eq!(database.notifications().unwrap().len(), 8);
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
             ALTER TABLE settings DROP COLUMN gog_notifications_enabled;
             ALTER TABLE notification_history DROP COLUMN store;
             DELETE FROM schema_migrations WHERE version = 4;",
            )
            .unwrap();
    }
    {
        let database = Database::open(&path).unwrap();
        let mut settings = database.settings().unwrap();
        assert!(
            settings.steam_notifications_enabled
                && settings.epic_games_notifications_enabled
                && settings.gog_notifications_enabled
        );
        assert_eq!(database.notifications().unwrap().len(), 8);
        assert_eq!(database.pending_notifications().unwrap().len(), 8);
        assert!(
            database
                .notifications()
                .unwrap()
                .iter()
                .all(|record| record.store == GameStore::Unknown)
        );
        settings.steam_notifications_enabled = false;
        settings.epic_games_notifications_enabled = false;
        settings.gog_notifications_enabled = false;
        database.update_settings(&settings).unwrap();
    }
    let database = Database::open(&path).unwrap();
    let settings = database.settings().unwrap();
    assert!(
        !settings.steam_notifications_enabled
            && !settings.epic_games_notifications_enabled
            && !settings.gog_notifications_enabled
    );
    assert_eq!(database.counts().unwrap().tracked_giveaways, 8);
    assert_eq!(database.notifications().unwrap().len(), 8);
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

#[test]
fn gog_migration_preserves_known_platforms_history_and_other_preferences() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("v8.db");
    let history;
    {
        let mut database = Database::open(&path).unwrap();
        let mut settings = database.settings().unwrap();
        settings.initial_sync_mode = Some(InitialSyncMode::NotifyCurrent);
        settings.steam_notifications_enabled = false;
        database.update_settings(&settings).unwrap();
        database
            .process_discovered_items(&items(), &GiveawaySource::ALL)
            .unwrap();
        history = database.notifications().unwrap();
        for record in &history {
            let item = items()
                .into_iter()
                .find(|item| Some(item.tracking_key()) == record.tracking_key)
                .unwrap();
            assert_eq!(record.store, item.store);
            assert_eq!(
                serde_json::to_value(record).unwrap()["store"],
                item.store.storage_name()
            );
            assert_eq!(
                database.notification(&record.id).unwrap().unwrap().store,
                item.store
            );
        }
    }
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "ALTER TABLE settings DROP COLUMN gog_notifications_enabled;
            DELETE FROM schema_migrations WHERE version = 9;",
            )
            .unwrap();
    }
    {
        let database = Database::open(&path).unwrap();
        let mut settings = database.settings().unwrap();
        assert!(settings.gog_notifications_enabled);
        assert!(!settings.steam_notifications_enabled);
        assert_eq!(database.notifications().unwrap(), history);
        settings.gog_notifications_enabled = false;
        database.update_settings(&settings).unwrap();
        for record in &history {
            let eligible = database.notification_for_delivery(&record.id).unwrap();
            assert_eq!(eligible.is_some(), record.store != GameStore::Gog);
        }
        assert_eq!(database.notifications().unwrap(), history);
    }
    let database = Database::open(&path).unwrap();
    assert!(!database.settings().unwrap().gog_notifications_enabled);
    assert_eq!(database.notifications().unwrap(), history);
}

#[test]
fn settings_without_gog_preference_keep_notifications_enabled() {
    let mut value = serde_json::to_value(giveaway_scout_core::AppSettings::default()).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .remove("gogNotificationsEnabled");
    let settings: giveaway_scout_core::AppSettings = serde_json::from_value(value).unwrap();
    assert!(settings.gog_notifications_enabled);
}
