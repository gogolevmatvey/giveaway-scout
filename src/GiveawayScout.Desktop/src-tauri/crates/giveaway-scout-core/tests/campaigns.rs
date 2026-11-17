use giveaway_scout_core::{
    Database, GameStore, GiveawayItem, GiveawayKind, GiveawaySource, InitialSyncMode, SourcePhase,
    SourceStatus,
};
use rusqlite::Connection;

fn item(source: GiveawaySource, id: &str, url: &str, title: &str) -> GiveawayItem {
    GiveawayItem {
        source,
        source_id: id.into(),
        url: url.into(),
        title: title.into(),
        kind: GiveawayKind::Giveaway,
        store: GameStore::Steam,
        details: None,
    }
}
fn notifying_database() -> Database {
    let database = Database::open_in_memory().unwrap();
    let mut settings = database.settings().unwrap();
    settings.initial_sync_mode = Some(InitialSyncMode::NotifyCurrent);
    database.update_settings(&settings).unwrap();
    database
}
fn discover(database: &mut Database, item: &GiveawayItem) -> usize {
    database
        .process_discovered_items(std::slice::from_ref(item), &[item.source])
        .unwrap()
        .new_items
        .len()
}

#[test]
fn confirmed_duplicates_choose_origin_and_one_queue_in_both_arrival_orders() {
    let origin = item(
        GiveawaySource::GiveeClub,
        "event/42",
        "https://givee.club/en/event/42",
        "Original title",
    );
    let aggregate = item(
        GiveawaySource::FreeSteamKeys,
        "100",
        "https://www.freesteamkeys.com/open/100/",
        "Aggregate title",
    );
    for aggregate_first in [false, true] {
        let mut database = notifying_database();
        database
            .cache_redirect(&aggregate.url, Some("https://givee.club/ru/event/42"), None)
            .unwrap();
        let (first, second) = if aggregate_first {
            (&aggregate, &origin)
        } else {
            (&origin, &aggregate)
        };
        assert_eq!(discover(&mut database, first), 1);
        let old_id = database.notifications().unwrap()[0].id.clone();
        assert_eq!(discover(&mut database, second), 0);
        let cards = database.notifications().unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].id, old_id);
        assert_eq!(cards[0].title, "Original title");
        assert_eq!(cards[0].origin_source, Some(GiveawaySource::GiveeClub));
        assert!(cards[0].url.contains("givee.club"));
        assert_eq!(
            cards[0].detected_sources,
            [GiveawaySource::FreeSteamKeys, GiveawaySource::GiveeClub]
        );
        assert_eq!(database.pending_notifications().unwrap().len(), 1);
        database.record_notification_delivered(&old_id).unwrap();
        assert!(database.pending_notifications().unwrap().is_empty());
    }
}

#[test]
fn an_enabled_detector_keeps_group_pending_after_representative_source_is_disabled() {
    let mut database = notifying_database();
    let origin = item(
        GiveawaySource::GiveeClub,
        "event/42",
        "https://givee.club/event/42",
        "Origin",
    );
    discover(&mut database, &origin);
    let id = database.notifications().unwrap()[0].id.clone();
    let aggregate = item(
        GiveawaySource::FreeSteamKeys,
        "copy",
        "https://givee.club/en/event/42",
        "Copy",
    );
    assert_eq!(discover(&mut database, &aggregate), 0);
    let mut settings = database.settings().unwrap();
    settings.givee_club_enabled = false;
    database.update_settings(&settings).unwrap();
    assert_eq!(database.pending_notifications().unwrap().len(), 1);
    assert!(database.notification_for_delivery(&id).unwrap().is_some());
    settings.steam_notifications_enabled = false;
    database.update_settings(&settings).unwrap();
    assert!(database.pending_notifications().unwrap().is_empty());
    settings.steam_notifications_enabled = true;
    settings.free_steam_keys_enabled = false;
    database.update_settings(&settings).unwrap();
    assert!(database.pending_notifications().unwrap().is_empty());
    assert_eq!(database.notifications().unwrap().len(), 1);
}

#[test]
fn origin_link_is_used_even_when_origin_monitoring_is_disabled() {
    let mut database = notifying_database();
    let mut settings = database.settings().unwrap();
    settings.givee_club_enabled = false;
    database.update_settings(&settings).unwrap();
    let aggregate = item(
        GiveawaySource::FreeSteamKeys,
        "1",
        "https://www.freesteamkeys.com/open/1/",
        "Game",
    );
    database
        .cache_redirect(&aggregate.url, Some("https://givee.club/en/event/42"), None)
        .unwrap();
    assert_eq!(discover(&mut database, &aggregate), 1);
    let card = database.pending_notifications().unwrap().remove(0);
    assert_eq!(card.origin_source, Some(GiveawaySource::GiveeClub));
    assert!(card.url.contains("givee.club"));
    assert_eq!(card.detected_sources, [GiveawaySource::FreeSteamKeys]);
}

#[test]
fn late_resolution_merges_read_and_delivery_state_preserving_old_ids_and_position() {
    let mut database = notifying_database();
    let aggregate = item(
        GiveawaySource::FreeSteamKeys,
        "1",
        "https://www.freesteamkeys.com/open/1/",
        "Aggregate",
    );
    let origin = item(
        GiveawaySource::GiveeClub,
        "event/42",
        "https://givee.club/en/event/42",
        "Origin",
    );
    assert_eq!(discover(&mut database, &aggregate), 1);
    let aggregate_id = database.notifications().unwrap()[0].id.clone();
    database
        .record_notification_delivered(&aggregate_id)
        .unwrap();
    database.mark_notification_read(&aggregate_id).unwrap();
    let earliest_read = database
        .notification(&aggregate_id)
        .unwrap()
        .unwrap()
        .read_at_utc;
    assert_eq!(discover(&mut database, &origin), 1);
    let origin_id = database.notifications().unwrap()[0].id.clone();
    assert_ne!(aggregate_id, origin_id);
    let other = item(
        GiveawaySource::FreeSteamKeys,
        "2",
        "https://gleam.io/AbC12/other",
        "Other",
    );
    assert_eq!(discover(&mut database, &other), 1);
    database
        .apply_resolved_url(&aggregate.url, &origin.url)
        .unwrap();
    let cards = database.notifications().unwrap();
    assert_eq!(cards.len(), 2);
    assert_eq!(cards[0].title, "Other");
    assert_eq!(cards[1].id, aggregate_id);
    assert_eq!(cards[1].title, "Origin");
    assert_eq!(cards[1].read_at_utc, earliest_read);
    assert_eq!(cards[1].delivery_state, "delivered");
    assert_eq!(
        database.notification(&origin_id).unwrap().unwrap().url,
        cards[1].url
    );
    assert!(
        database
            .notification_for_delivery(&origin_id)
            .unwrap()
            .is_none()
    );
    assert_eq!(database.counts().unwrap().unread_notifications, 1);
    assert_eq!(database.pending_notifications().unwrap().len(), 1);
    assert!(database.pending_redirects(10).unwrap().is_empty());
}

#[test]
fn reading_without_opening_excludes_pending_delivery_and_duplicate_does_not_unread() {
    let mut database = notifying_database();
    let first = item(
        GiveawaySource::GiveeClub,
        "event/1",
        "https://givee.club/ru/event/1",
        "Game",
    );
    discover(&mut database, &first);
    let id = database.notifications().unwrap()[0].id.clone();
    database.mark_notification_read(&id).unwrap();
    let copy = item(
        GiveawaySource::FreeSteamKeys,
        "copy",
        "https://givee.club/en/event/1",
        "Copy",
    );
    assert_eq!(discover(&mut database, &copy), 0);
    assert_eq!(database.counts().unwrap().unread_notifications, 0);
    assert!(database.pending_notifications().unwrap().is_empty());
}

#[test]
fn names_app_ids_and_shared_zeros_url_do_not_merge_independent_campaigns() {
    let mut database = notifying_database();
    let mut settings = database.settings().unwrap();
    settings.zeros_enabled = true;
    database.update_settings(&settings).unwrap();
    database
        .process_discovered_items(&[], &[GiveawaySource::Zeros])
        .unwrap();
    for item in [
        item(
            GiveawaySource::FreeSteamKeys,
            "1",
            "https://gleam.io/AbC12/game",
            "Same game",
        ),
        item(
            GiveawaySource::FreeSteamKeys,
            "2",
            "https://gleam.io/abc12/game",
            "Same game",
        ),
        item(
            GiveawaySource::Zeros,
            "period-1",
            "http://zeros.group/free/",
            "Same game",
        ),
        item(
            GiveawaySource::Zeros,
            "period-2",
            "http://zeros.group/free/",
            "Same game",
        ),
    ] {
        assert_eq!(discover(&mut database, &item), 1);
    }
    assert_eq!(database.notifications().unwrap().len(), 4);
}

#[test]
fn initial_notify_choice_survives_other_sources_completion_and_restart() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("profile.db");
    let mut database = Database::open(&path).unwrap();
    let mut settings = database.settings().unwrap();
    settings.initial_sync_mode = Some(InitialSyncMode::NotifyCurrent);
    database.update_settings(&settings).unwrap();
    database
        .process_discovered_items(&[], &[GiveawaySource::FreeSteamKeys])
        .unwrap();
    drop(database);
    let mut database = Database::open(&path).unwrap();
    let origin = item(
        GiveawaySource::GiveeClub,
        "event/1",
        "https://givee.club/event/1",
        "Late first result",
    );
    assert_eq!(discover(&mut database, &origin), 1);
    let mut settings = database.settings().unwrap();
    settings.zeros_enabled = true;
    database.update_settings(&settings).unwrap();
    let zeros = item(
        GiveawaySource::Zeros,
        "period-1",
        "http://zeros.group/free/",
        "Later enabled",
    );
    assert_eq!(discover(&mut database, &zeros), 0);
    database.clear_giveaway_data().unwrap();
    assert_eq!(discover(&mut database, &origin), 0);
    assert_eq!(discover(&mut database, &zeros), 0);
}

#[test]
fn pending_initial_choice_survives_disable_enable_and_clear_restart_is_silent_for_all_sources() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("profile.db");
    let mut database = Database::open(&path).unwrap();
    let mut settings = database.settings().unwrap();
    settings.zeros_enabled = true;
    settings.initial_sync_mode = Some(InitialSyncMode::NotifyCurrent);
    database.update_settings(&settings).unwrap();
    database
        .process_discovered_items(&[], &[GiveawaySource::FreeSteamKeys])
        .unwrap();
    settings.givee_club_enabled = false;
    database.update_settings(&settings).unwrap();
    drop(database);
    let mut database = Database::open(&path).unwrap();
    let mut settings = database.settings().unwrap();
    settings.givee_club_enabled = true;
    database.update_settings(&settings).unwrap();
    let origin = item(
        GiveawaySource::GiveeClub,
        "event/42",
        "https://givee.club/event/42",
        "Late source",
    );
    assert_eq!(
        discover(&mut database, &origin),
        1,
        "temporary disable must preserve the original choice until this source succeeds"
    );
    database.clear_giveaway_data().unwrap();
    drop(database);
    let mut database = Database::open(&path).unwrap();
    assert!(!database.initial_sync_choice_required().unwrap());
    for candidate in [
        item(
            GiveawaySource::FreeSteamKeys,
            "43",
            "https://www.freesteamkeys.com/open/43/",
            "Aggregate baseline",
        ),
        origin,
        item(
            GiveawaySource::GamesForFarm,
            "/game?n=44",
            "https://freeanywhere.net/game?n=44",
            "Farm baseline",
        ),
        item(
            GiveawaySource::Zeros,
            "period-45",
            "http://zeros.group/free/",
            "Zeros baseline",
        ),
    ] {
        assert_eq!(discover(&mut database, &candidate), 0);
        assert!(
            database
                .source_tracking_initialized(candidate.source)
                .unwrap()
        );
    }
    assert_eq!(database.counts().unwrap().notification_history, 0);
    assert!(database.pending_notifications().unwrap().is_empty());
}

fn v12_profile(path: &std::path::Path) {
    drop(Database::open(path).unwrap());
    let connection = Connection::open(path).unwrap();
    connection.execute_batch("DELETE FROM schema_migrations WHERE version=13;
       DROP TABLE notification_campaigns; DROP TABLE campaign_members; DROP TABLE campaigns;
       DROP TABLE redirect_cache; DROP TABLE source_status; DROP TABLE source_initial_policy;
       INSERT INTO notification_history(id,tracking_key,source,kind,title,url,received_at_utc,read_at_utc,delivery_state,delivery_attempts,store)
       VALUES ('old-aggregate','FreeSteamKeys:11','freeSteamKeys','giveaway','Aggregate','https://givee.club/en/event/42','2026-01-01T00:00:00Z',NULL,'pending',0,'steam'),
       ('old-origin','GiveeClub:event/42','giveeClub','giveaway','Original','https://givee.club/ru/event/42','2026-01-02T00:00:00Z','2026-01-02T01:00:00Z','delivered',1,'steam');").unwrap();
}

#[test]
fn migration_groups_offline_preserves_uuid_read_and_delivered_state() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("profile.db");
    v12_profile(&path);
    let database = Database::open(&path).unwrap();
    let cards = database.notifications().unwrap();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].id, "old-aggregate");
    assert_eq!(cards[0].title, "Original");
    assert_eq!(
        cards[0].read_at_utc.as_deref(),
        Some("2026-01-02T01:00:00Z")
    );
    assert_eq!(cards[0].delivery_state, "delivered");
    assert_eq!(database.counts().unwrap().notification_history, 1);
    assert_eq!(database.counts().unwrap().unread_notifications, 0);
    for id in ["old-aggregate", "old-origin"] {
        assert_eq!(
            database.notification(id).unwrap().unwrap().url,
            cards[0].url
        );
    }
    assert!(database.pending_notifications().unwrap().is_empty());
}

#[test]
fn failed_v13_migration_rolls_back_new_schema_and_leaves_legacy_rows() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("profile.db");
    v12_profile(&path);
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_v13 BEFORE INSERT ON schema_migrations WHEN NEW.version=13 BEGIN SELECT RAISE(ABORT,'fixture v13 failure'); END;").unwrap();
    drop(connection);
    assert!(Database::open_with_recovery(&path).is_err());
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM notification_history", [], |row| row
                .get::<_, i64>(
                0
            ))
            .unwrap(),
        2
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='campaigns'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT title FROM notification_history WHERE id='old-aggregate'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "Aggregate"
    );
    connection.execute_batch("DROP TRIGGER fail_v13").unwrap();
    drop(connection);
    assert_eq!(
        Database::open(path).unwrap().notifications().unwrap().len(),
        1
    );
}

#[test]
fn statuses_and_redirect_retry_policy_persist_and_clear() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("profile.db");
    let database = Database::open(&path).unwrap();
    let mut status = SourceStatus::new(GiveawaySource::FreeSteamKeys);
    status.phase = SourcePhase::Ok;
    status.last_completed_at_utc = Some("2026-11-07T00:00:00Z".into());
    status.last_successful_check_utc = status.last_completed_at_utc.clone();
    database.save_source_status(&status).unwrap();
    drop(database);
    let mut database = Database::open(&path).unwrap();
    assert_eq!(database.source_statuses().unwrap()[0], status);
    let aggregate = item(
        GiveawaySource::FreeSteamKeys,
        "1",
        "https://www.freesteamkeys.com/open/1/",
        "Game",
    );
    discover(&mut database, &aggregate);
    assert_eq!(
        database.pending_redirects(1).unwrap(),
        std::slice::from_ref(&aggregate.url)
    );
    database
        .cache_redirect(&aggregate.url, None, Some("2999-01-01T00:00:00Z"))
        .unwrap();
    assert!(database.pending_redirects(10).unwrap().is_empty());
    database
        .cache_redirect(&aggregate.url, None, Some("2000-01-01T00:00:00Z"))
        .unwrap();
    assert_eq!(database.pending_redirects(1).unwrap(), [aggregate.url]);
    database.clear_giveaway_data().unwrap();
    assert_eq!(
        database.source_statuses().unwrap()[0],
        SourceStatus::new(GiveawaySource::FreeSteamKeys)
    );
}

#[test]
fn a_late_failed_lookup_cannot_overwrite_a_confirmed_destination() {
    let mut database = notifying_database();
    let aggregate = item(
        GiveawaySource::FreeSteamKeys,
        "1",
        "https://www.freesteamkeys.com/open/1/",
        "Game",
    );
    discover(&mut database, &aggregate);
    let origin = "https://givee.club/event/42";
    database.apply_resolved_url(&aggregate.url, origin).unwrap();
    let canonical = database.notifications().unwrap().remove(0);
    // Simulate another batch timing out after the successful result committed.
    database
        .cache_redirect(&aggregate.url, None, Some("2999-01-01T00:00:00Z"))
        .unwrap();
    let cached = database.cached_redirect(&aggregate.url).unwrap().unwrap();
    assert_eq!(cached.url.as_deref(), Some(origin));
    assert_eq!(cached.retry_after_utc, None);
    assert!(database.pending_redirects(10).unwrap().is_empty());
    assert_eq!(discover(&mut database, &aggregate), 0);
    assert_eq!(database.notifications().unwrap(), [canonical]);
    // Even a conflicting success finishing later must preserve the first
    // mapping, including when enrichment is applied against another campaign.
    let other = item(
        GiveawaySource::GiveeClub,
        "event/43",
        "https://givee.club/event/43",
        "Independent campaign",
    );
    assert_eq!(discover(&mut database, &other), 1);
    let before = database.notifications().unwrap();
    database
        .cache_redirect(&aggregate.url, Some(&other.url), None)
        .unwrap();
    assert_eq!(
        database
            .cached_redirect(&aggregate.url)
            .unwrap()
            .unwrap()
            .url
            .as_deref(),
        Some(origin)
    );
    database
        .apply_resolved_url(&aggregate.url, &other.url)
        .unwrap();
    assert_eq!(
        database
            .cached_redirect(&aggregate.url)
            .unwrap()
            .unwrap()
            .url
            .as_deref(),
        Some(origin)
    );
    assert_eq!(database.notifications().unwrap(), before);
    assert_ne!(before[0].campaign_id, before[1].campaign_id);
    // A changed original URL has its own negative entry and cannot inherit the
    // previous URL's successful destination.
    let changed = "https://www.freesteamkeys.com/open/changed/";
    database
        .cache_redirect(changed, None, Some("2999-01-01T00:00:00Z"))
        .unwrap();
    assert_eq!(
        database.cached_redirect(changed).unwrap().unwrap().url,
        None
    );
}

#[test]
fn editing_an_aggregate_link_cannot_union_two_confirmed_campaigns() {
    for newer_origin_already_known in [false, true] {
        let mut database = notifying_database();
        let old_origin = item(
            GiveawaySource::GiveeClub,
            "event/42",
            "https://givee.club/event/42",
            "Older campaign",
        );
        let new_origin = item(
            GiveawaySource::GiveeClub,
            "event/43",
            "https://givee.club/event/43",
            "Newer campaign",
        );
        let mut aggregate = item(
            GiveawaySource::FreeSteamKeys,
            "100",
            "https://www.freesteamkeys.com/open/original/",
            "Aggregate",
        );
        discover(&mut database, &old_origin);
        let old_card = database.notifications().unwrap().remove(0);
        database.mark_notification_read(&old_card.id).unwrap();
        database
            .record_notification_delivered(&old_card.id)
            .unwrap();
        database
            .cache_redirect(&aggregate.url, Some(&old_origin.url), None)
            .unwrap();
        assert_eq!(discover(&mut database, &aggregate), 0);
        let new_id = if newer_origin_already_known {
            assert_eq!(discover(&mut database, &new_origin), 1);
            Some(database.notifications().unwrap()[0].id.clone())
        } else {
            None
        };
        // The same post now has a different input URL resolving to event 43.
        aggregate.url = "https://www.freesteamkeys.com/open/edited/".into();
        database
            .cache_redirect(&aggregate.url, Some(&new_origin.url), None)
            .unwrap();
        assert_eq!(discover(&mut database, &aggregate), 0);
        let old_after = database.notification(&old_card.id).unwrap().unwrap();
        assert_eq!(old_after.url, old_origin.url);
        assert_eq!(old_after.title, "Older campaign");
        assert!(old_after.read_at_utc.is_some());
        assert_eq!(old_after.delivery_state, "delivered");
        assert!(
            old_after
                .detected_sources
                .contains(&GiveawaySource::FreeSteamKeys)
        );
        if let Some(new_id) = new_id {
            let new_after = database.notification(&new_id).unwrap().unwrap();
            assert_ne!(new_after.campaign_id, old_after.campaign_id);
            assert_eq!(new_after.url, new_origin.url);
            assert_eq!(new_after.title, "Newer campaign");
            assert_eq!(new_after.read_at_utc, None);
            assert_eq!(new_after.delivery_state, "pending");
            assert!(
                new_after
                    .detected_sources
                    .contains(&GiveawaySource::FreeSteamKeys)
            );
            assert_eq!(database.notifications().unwrap().len(), 2);
        } else {
            assert_eq!(database.notifications().unwrap().len(), 1);
            // When the new origin feed arrives later it enriches that current
            // membership silently, without changing the historical campaign.
            assert_eq!(discover(&mut database, &new_origin), 0);
            assert_eq!(database.notifications().unwrap().len(), 1);
        }
        assert_eq!(database.counts().unwrap().tracked_giveaways, 3);
    }
}
