use std::{collections::HashSet, sync::Mutex};

use giveaway_scout_core::{
    Database, GameStore, GiveawayItem, GiveawayKind, GiveawaySource, InitialSyncMode,
    dispatch_pending_notifications,
};

fn items() -> Vec<GiveawayItem> {
    // Original two-source fixture; Zeros delivery has its own opt-in regression tests.
    [GiveawaySource::GiveeClub, GiveawaySource::GamesForFarm]
        .into_iter()
        .flat_map(|source| {
            [GameStore::Steam, GameStore::EpicGames, GameStore::Unknown]
                .into_iter()
                .map(move |store| GiveawayItem {
                    source,
                    store,
                    source_id: format!("test/{}", store.storage_name()),
                    title: "Fixture game".to_owned(),
                    url: "https://example.invalid/game".to_owned(),
                    kind: GiveawayKind::Giveaway,
                    details: None,
                })
        })
        .collect()
}

fn database_with_pending() -> Database {
    let mut database = Database::open_in_memory().unwrap();
    let mut settings = database.settings().unwrap();
    settings.initial_sync_mode = Some(InitialSyncMode::NotifyCurrent);
    settings.language_code = Some("ru".to_owned());
    database.update_settings(&settings).unwrap();
    database
        .process_discovered_items(&items(), &GiveawaySource::ALL)
        .unwrap();
    assert_eq!(database.pending_notifications().unwrap().len(), 6);
    database
}

#[test]
fn disabling_a_source_pauses_its_pending_records_without_erasing_history_or_attempts() {
    let database = database_with_pending();
    let history = database.notifications().unwrap();
    let mut settings = database.settings().unwrap();
    settings.givee_club_enabled = false;
    database.update_settings(&settings).unwrap();
    let pending = database.pending_notifications().unwrap();
    assert_eq!(pending.len(), 3);
    assert!(pending.iter().all(|record| record.source == "gamesForFarm"));
    assert_eq!(database.notifications().unwrap(), history);
    settings.givee_club_enabled = true;
    database.update_settings(&settings).unwrap();
    assert_eq!(database.pending_notifications().unwrap().len(), 6);
}

#[test]
fn records_read_in_history_are_not_retried_as_system_notifications() {
    let database = database_with_pending();
    let record = database.pending_notifications().unwrap().remove(0);
    database.mark_notification_read(&record.id).unwrap();
    assert_eq!(database.pending_notifications().unwrap().len(), 5);
    database.mark_all_notifications_read().unwrap();
    assert!(database.pending_notifications().unwrap().is_empty());
    assert_eq!(database.notifications().unwrap().len(), 6);
}

#[test]
fn source_disabled_during_a_fetch_does_not_create_notifications_for_the_late_response() {
    let mut database = Database::open_in_memory().unwrap();
    database
        .process_discovered_items(&[], &GiveawaySource::ALL)
        .unwrap();
    let mut settings = database.settings().unwrap();
    settings.givee_club_enabled = false;
    database.update_settings(&settings).unwrap();
    let response = items();
    let result = database
        .process_discovered_items(&response, &GiveawaySource::ALL)
        .unwrap();
    assert_eq!(result.new_items.len(), 3);
    assert!(
        result
            .new_items
            .iter()
            .all(|item| item.source == GiveawaySource::GamesForFarm)
    );
    assert_eq!(database.counts().unwrap().tracked_giveaways, 6);
    settings.givee_club_enabled = true;
    database.update_settings(&settings).unwrap();
    assert!(
        database
            .process_discovered_items(&response, &GiveawaySource::ALL)
            .unwrap()
            .new_items
            .is_empty()
    );
}

#[test]
fn delivery_rechecks_each_record_after_source_change_and_resumes_without_duplicates() {
    let database = Mutex::new(database_with_pending());
    let mut sent = Vec::new();
    let mut paused_source = String::new();
    let report = dispatch_pending_notifications(&database, |record, _| {
        // A synchronous activation/settings callback must be able to access DB.
        let db = database
            .try_lock()
            .expect("native callback must not hold the database mutex");
        if sent.is_empty() {
            paused_source = record.source.clone();
            let mut settings = db.settings().unwrap();
            settings.givee_club_enabled = paused_source != "giveeClub";
            settings.games_for_farm_enabled = paused_source != "gamesForFarm";
            db.update_settings(&settings).unwrap();
        }
        sent.push((record.id.clone(), record.source.clone()));
        Ok(())
    })
    .unwrap();
    assert_eq!(report.delivered, 4);
    assert_eq!(report.skipped, 2);
    assert_eq!(
        sent.iter()
            .filter(|(_, source)| source == &paused_source)
            .count(),
        1
    );
    {
        let db = database.lock().unwrap();
        assert!(db.pending_notifications().unwrap().is_empty());
        assert_eq!(
            db.notifications()
                .unwrap()
                .iter()
                .filter(|r| r.delivery_attempts == 0)
                .count(),
            2
        );
        let mut settings = db.settings().unwrap();
        settings.givee_club_enabled = true;
        settings.games_for_farm_enabled = true;
        db.update_settings(&settings).unwrap();
    }
    let retry = dispatch_pending_notifications(&database, |record, _| {
        sent.push((record.id.clone(), record.source.clone()));
        Ok(())
    })
    .unwrap();
    assert_eq!(retry.delivered, 2);
    assert_eq!(sent.len(), 6);
    assert_eq!(
        sent.iter().map(|(id, _)| id).collect::<HashSet<_>>().len(),
        6
    );
    assert!(
        database
            .lock()
            .unwrap()
            .notifications()
            .unwrap()
            .iter()
            .all(|r| r.delivery_attempts == 1 && r.delivery_state == "delivered")
    );
}

#[test]
fn mark_all_read_during_delivery_stops_the_remaining_queue_without_fake_delivery_success() {
    let database = Mutex::new(database_with_pending());
    let mut calls = 0;
    let report = dispatch_pending_notifications(&database, |_, _| {
        calls += 1;
        database
            .try_lock()
            .unwrap()
            .mark_all_notifications_read()
            .unwrap();
        Ok(())
    })
    .unwrap();
    assert_eq!(calls, 1);
    assert_eq!(report.attempted, 1);
    assert_eq!(report.skipped, 5);
    let history = database.lock().unwrap().notifications().unwrap();
    assert!(history.iter().all(|r| r.read_at_utc.is_some()));
    assert_eq!(
        history
            .iter()
            .filter(|r| r.delivery_state == "delivered")
            .count(),
        1
    );
    assert!(
        history
            .iter()
            .filter(|r| r.delivery_state == "pending")
            .all(|r| r.delivery_attempts == 0)
    );
    assert_eq!(
        dispatch_pending_notifications(&database, |_, _| panic!("read records must not be shown"))
            .unwrap()
            .attempted,
        0
    );
}

#[test]
fn later_attempts_use_current_language_and_sound_instead_of_the_queue_snapshot() {
    let database = Mutex::new(database_with_pending());
    let mut calls = 0;
    let report = dispatch_pending_notifications(&database, |_, settings| {
        if calls == 0 {
            assert_eq!(settings.language_code.as_deref(), Some("ru"));
            assert!(settings.notification_sound_enabled);
            let db = database.try_lock().unwrap();
            let mut updated = db.settings().unwrap();
            updated.language_code = Some("en".to_owned());
            updated.notification_sound_enabled = false;
            db.update_settings(&updated).unwrap();
        } else {
            assert_eq!(settings.language_code.as_deref(), Some("en"));
            assert!(!settings.notification_sound_enabled);
        }
        calls += 1;
        Ok(())
    })
    .unwrap();
    assert_eq!(calls, 6);
    assert_eq!(report.delivered, 6);
}

#[test]
fn failed_attempt_remains_pending_and_retries_the_same_uuid_only_when_enabled() {
    let database = Mutex::new(database_with_pending());
    let mut failure = None;
    let report = dispatch_pending_notifications(&database, |record, _| {
        if failure.is_none() {
            failure = Some((record.id.clone(), record.source.clone()));
            Err("fixture native transport failure".to_owned())
        } else {
            Ok(())
        }
    })
    .unwrap();
    assert_eq!(report.failed, 1);
    assert_eq!(report.delivered, 5);
    let (id, source) = failure.unwrap();
    {
        let db = database.lock().unwrap();
        let pending = db.notification_for_delivery(&id).unwrap().unwrap();
        assert_eq!(pending.delivery_attempts, 1);
        assert_eq!(
            pending.last_delivery_error.as_deref(),
            Some("fixture native transport failure")
        );
        let mut settings = db.settings().unwrap();
        settings.givee_club_enabled = source != "giveeClub";
        settings.games_for_farm_enabled = source != "gamesForFarm";
        db.update_settings(&settings).unwrap();
    }
    assert_eq!(
        dispatch_pending_notifications(&database, |_, _| panic!("disabled source"))
            .unwrap()
            .attempted,
        0
    );
    {
        let db = database.lock().unwrap();
        let mut settings = db.settings().unwrap();
        settings.givee_club_enabled = true;
        settings.games_for_farm_enabled = true;
        db.update_settings(&settings).unwrap();
    }
    assert_eq!(
        dispatch_pending_notifications(&database, |record, _| {
            assert_eq!(record.id, id);
            Ok(())
        })
        .unwrap()
        .delivered,
        1
    );
    let db = database.lock().unwrap();
    let delivered = db.notification(&id).unwrap().unwrap();
    assert_eq!(delivered.delivery_attempts, 2);
    assert_eq!(delivered.delivery_state, "delivered");
    assert!(delivered.last_delivery_error.is_none());
    assert!(db.notification_for_delivery(&id).unwrap().is_none());
}

#[test]
fn queue_and_single_record_eligibility_agree_for_every_source_and_store_filter_combination() {
    let mut database = database_with_pending();
    let gog_items: Vec<_> = items()
        .into_iter()
        .filter(|item| item.store == GameStore::Unknown)
        .map(|mut item| {
            item.store = GameStore::Gog;
            item.source_id = "test/gog".to_owned();
            item
        })
        .collect();
    database
        .process_discovered_items(&gog_items, &GiveawaySource::ALL)
        .unwrap();
    let history = database.notifications().unwrap();
    for givee in [false, true] {
        for games in [false, true] {
            for steam in [false, true] {
                for epic in [false, true] {
                    for gog in [false, true] {
                        let mut settings = database.settings().unwrap();
                        settings.givee_club_enabled = givee;
                        settings.games_for_farm_enabled = games;
                        settings.steam_notifications_enabled = steam;
                        settings.epic_games_notifications_enabled = epic;
                        settings.gog_notifications_enabled = gog;
                        database.update_settings(&settings).unwrap();
                        let expected: HashSet<_> = items()
                            .into_iter()
                            .chain(gog_items.iter().cloned())
                            .filter(|item| {
                                settings.allows_source(item.source)
                                    && settings.allows_store(item.store)
                            })
                            .map(|item| item.tracking_key())
                            .collect();
                        let queued: HashSet<_> = database
                            .pending_notifications()
                            .unwrap()
                            .into_iter()
                            .map(|r| r.tracking_key.unwrap())
                            .collect();
                        assert_eq!(queued, expected);
                        for record in &history {
                            assert_eq!(
                                database
                                    .notification_for_delivery(&record.id)
                                    .unwrap()
                                    .is_some(),
                                expected.contains(record.tracking_key.as_ref().unwrap())
                            );
                        }
                    }
                }
            }
        }
    }
}
