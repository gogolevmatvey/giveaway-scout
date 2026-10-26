use giveaway_scout_core::{
    Database, GiveawayItem, GiveawaySource, InitialSyncMode, parse_games_for_farm, parse_givee_club,
};

#[test]
fn first_synchronization_creates_baseline_without_new_items() {
    let mut database = Database::open_in_memory().unwrap();
    let items = all_fixture_items();

    let result = database
        .process_discovered_items(&items, &GiveawaySource::ALL)
        .unwrap();

    assert!(result.is_initial_synchronization);
    assert!(result.new_items.is_empty());
    assert!(database.tracking_initialized().unwrap());
    assert_eq!(database.counts().unwrap().tracked_giveaways, 3);
}

#[test]
fn first_synchronization_can_notify_about_current_items() {
    let mut database = Database::open_in_memory().unwrap();
    assert!(database.initial_sync_choice_required().unwrap());
    let mut settings = database.settings().unwrap();
    settings.initial_sync_mode = Some(InitialSyncMode::NotifyCurrent);
    database.update_settings(&settings).unwrap();
    assert!(!database.initial_sync_choice_required().unwrap());
    let items = all_fixture_items();

    let result = database
        .process_discovered_items(&items, &GiveawaySource::ALL)
        .unwrap();

    assert!(result.is_initial_synchronization);
    assert_eq!(result.new_items.len(), 3);
    assert_eq!(
        result
            .new_items
            .iter()
            .map(GiveawayItem::tracking_key)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3
    );
    assert_eq!(database.counts().unwrap().notification_history, 3);
}

#[test]
fn clearing_data_forces_the_next_baseline_to_stay_silent() {
    let mut database = Database::open_in_memory().unwrap();
    let mut settings = database.settings().unwrap();
    settings.initial_sync_mode = Some(InitialSyncMode::NotifyCurrent);
    database.update_settings(&settings).unwrap();
    database.clear_giveaway_data().unwrap();
    let items = all_fixture_items();

    let result = database
        .process_discovered_items(&items, &GiveawaySource::ALL)
        .unwrap();

    assert!(result.is_initial_synchronization);
    assert!(result.new_items.is_empty());
    assert_eq!(database.counts().unwrap().notification_history, 0);
}

#[test]
fn later_check_returns_only_new_unique_items() {
    let mut database = Database::open_in_memory().unwrap();
    let mut initial_items = all_fixture_items();
    database
        .process_discovered_items(&initial_items, &GiveawaySource::ALL)
        .expect("initial synchronization should succeed");

    let mut new_item = initial_items.remove(0);
    new_item.source_id = "event/102".to_owned();
    new_item.url = "https://givee.club/event/102".to_owned();
    new_item.title = "A new game".to_owned();
    let check_items = [initial_items[0].clone(), new_item.clone(), new_item.clone()];
    let result = database
        .process_discovered_items(&check_items, &GiveawaySource::ALL)
        .unwrap();

    assert!(!result.is_initial_synchronization);
    assert_eq!(result.new_items, [new_item]);
    assert_eq!(database.counts().unwrap().tracked_giveaways, 4);
    assert_eq!(database.counts().unwrap().notification_history, 1);
    assert_eq!(database.counts().unwrap().unread_notifications, 1);
}

#[test]
fn changed_kind_and_image_do_not_create_another_item() {
    let mut database = Database::open_in_memory().unwrap();
    let games = parse_games_for_farm(include_str!("fixtures/html/games-for-farm.html")).unwrap();

    database
        .process_discovered_items(&games[..1], &[GiveawaySource::GamesForFarm])
        .unwrap();
    let result = database
        .process_discovered_items(&games[1..], &[GiveawaySource::GamesForFarm])
        .unwrap();

    assert!(result.new_items.is_empty());
    assert_eq!(database.counts().unwrap().tracked_giveaways, 1);
}

#[test]
fn marking_one_notification_read_does_not_change_the_other() {
    let mut database = Database::open_in_memory().unwrap();
    let initial_items = all_fixture_items();
    database
        .process_discovered_items(&initial_items, &GiveawaySource::ALL)
        .unwrap();

    let mut first = initial_items[0].clone();
    first.source_id = "event/201".to_owned();
    first.url = "https://givee.club/event/201".to_owned();
    first.title = "First new game".to_owned();
    let mut second = initial_items[0].clone();
    second.source_id = "event/202".to_owned();
    second.url = "https://givee.club/event/202".to_owned();
    second.title = "Second new game".to_owned();
    database
        .process_discovered_items(&[first, second], &GiveawaySource::ALL)
        .unwrap();

    let notifications = database.notifications().unwrap();
    assert_eq!(notifications.len(), 2);
    assert!(
        database
            .mark_notification_read(&notifications[0].id)
            .unwrap()
    );

    let updated = database.notifications().unwrap();
    assert!(updated[0].read_at_utc.is_some());
    assert!(updated[1].read_at_utc.is_none());
    assert_eq!(database.counts().unwrap().unread_notifications, 1);
}

#[test]
fn failed_notification_delivery_stays_pending_and_can_be_retried() {
    let mut database = Database::open_in_memory().unwrap();
    let initial_items = all_fixture_items();
    database
        .process_discovered_items(&initial_items, &GiveawaySource::ALL)
        .unwrap();

    let mut new_item = initial_items[0].clone();
    new_item.source_id = "event/301".to_owned();
    new_item.url = "https://givee.club/event/301".to_owned();
    database
        .process_discovered_items(&[new_item], &GiveawaySource::ALL)
        .unwrap();

    let notification = database.pending_notifications().unwrap().remove(0);
    database
        .record_notification_delivery_failure(&notification.id, "notifications disabled")
        .unwrap();
    let failed = database.pending_notifications().unwrap().remove(0);
    assert_eq!(failed.delivery_state, "pending");
    assert_eq!(failed.delivery_attempts, 1);
    assert_eq!(
        failed.last_delivery_error.as_deref(),
        Some("notifications disabled")
    );

    database
        .record_notification_delivered(&notification.id)
        .unwrap();
    assert!(database.pending_notifications().unwrap().is_empty());
    let delivered = database.notification(&notification.id).unwrap().unwrap();
    assert_eq!(delivered.delivery_state, "delivered");
    assert_eq!(delivered.delivery_attempts, 2);
    assert!(delivered.last_delivery_error.is_none());
}

fn all_fixture_items() -> Vec<GiveawayItem> {
    let mut items = parse_givee_club(include_str!("fixtures/html/givee-club.html")).unwrap();
    items.extend(parse_games_for_farm(include_str!("fixtures/html/games-for-farm.html")).unwrap());
    items
}
