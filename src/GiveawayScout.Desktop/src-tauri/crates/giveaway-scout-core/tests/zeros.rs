use giveaway_scout_core::{
    Database, GameStore, GiveawaySource, HttpPageFetcher, InitialSyncMode, PageFetchError,
    PageFetcher, ZEROS_FEED_URL, apply_source_check, check_sources, parse_zeros,
};
use serde_json::{Value, json};

fn period(id: &str) -> Value {
    json!({"period_id":id,"period_name":"  新的 Steam 游戏  ","stock_count":1501,
        "claim_count":1442,"is_expired":false,"is_not_started":false,
        "start_time":"2026-10-17T16:54:00","deadline":"2099-10-21T16:24:00"})
}
fn feed(periods: Vec<Value>) -> String {
    json!({"periods":periods}).to_string()
}

#[test]
fn parses_public_period_configuration_without_using_task_games_as_rewards_or_urls() {
    let mut active = period("2026_1018");
    active["games"] = json!([{"appId":"4753620","name":"Wishlist task, not prize"}]);
    active["url"] = json!("file:///C:/untrusted.exe");
    let items = parse_zeros(&feed(vec![active])).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].source, GiveawaySource::Zeros);
    assert_eq!(items[0].title, "新的 Steam 游戏");
    assert_eq!(items[0].store, GameStore::Steam);
    assert_eq!(items[0].url, ZEROS_FEED_URL);
    assert_eq!(items[0].tracking_key(), "Zeros:2026_1018");
}

#[test]
fn ignores_expired_future_and_sold_out_periods_but_accepts_verified_empty_feed() {
    let mut expired = period("expired");
    expired["is_expired"] = json!(true);
    let mut deadline = period("deadline");
    deadline["deadline"] = json!("2000-01-01T00:00:00");
    let mut future = period("future");
    future["is_not_started"] = json!(true);
    let mut countdown = period("countdown");
    countdown["seconds_until_start"] = json!(60);
    let mut empty = period("empty");
    empty["stock_count"] = json!(0);
    let items = parse_zeros(&feed(vec![
        period("active"),
        expired,
        deadline,
        future,
        countdown,
        empty,
    ]))
    .unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].source_id, "active");
    assert!(parse_zeros(&feed(vec![])).unwrap().is_empty());
}

#[test]
fn rejects_malformed_or_incomplete_config_instead_of_establishing_a_false_baseline() {
    for invalid in [
        "<html>blocked</html>",
        "{}",
        r#"{"periods":null}"#,
        r#"{"periods":[{}]}"#,
    ] {
        assert!(parse_zeros(invalid).is_err());
    }
    for (field, value) in [
        ("period_id", json!("")),
        ("period_name", json!(" ")),
        ("stock_count", json!(-1)),
        ("deadline", json!("not a date")),
        ("is_expired", Value::Null),
    ] {
        let mut broken = period("broken");
        broken[field] = value;
        assert!(
            parse_zeros(&feed(vec![period("valid"), broken])).is_err(),
            "{field}"
        );
    }
    assert!(parse_zeros(&feed(vec![period("same"), period("same")])).is_err());
}

#[test]
fn title_image_stock_and_deadline_changes_do_not_change_identity_or_repeat_notifications() {
    let mut db = Database::open_in_memory().unwrap();
    let mut settings = db.settings().unwrap();
    settings.zeros_enabled = true;
    db.update_settings(&settings).unwrap();
    let original = period("one");
    db.process_discovered_items(
        &parse_zeros(&feed(vec![original.clone()])).unwrap(),
        &[GiveawaySource::Zeros],
    )
    .unwrap();
    let mut changed = original;
    changed["period_name"] = json!("Renamed");
    changed["image"] = json!("http://example.invalid/image");
    changed["stock_count"] = json!(10);
    changed["deadline"] = json!("2099-01-01T00:00:00Z");
    let result = db
        .process_discovered_items(
            &parse_zeros(&feed(vec![changed, period("two")])).unwrap(),
            &[GiveawaySource::Zeros],
        )
        .unwrap();
    assert_eq!(result.new_items.len(), 1);
    assert_eq!(result.new_items[0].source_id, "two");
    assert_eq!(db.counts().unwrap().notification_history, 1);
}

#[test]
fn enabling_zeros_on_existing_profile_is_silent_then_source_and_steam_filters_control_delivery() {
    let mut db = Database::open_in_memory().unwrap();
    db.process_discovered_items(&[], &[GiveawaySource::GiveeClub])
        .unwrap();
    let mut settings = db.settings().unwrap();
    assert!(!settings.zeros_enabled);
    assert!(
        !db.source_tracking_initialized(GiveawaySource::Zeros)
            .unwrap()
    );
    settings.zeros_enabled = true;
    db.update_settings(&settings).unwrap();
    let first = parse_zeros(&feed(vec![period("first")])).unwrap();
    assert!(
        db.process_discovered_items(&first, &[GiveawaySource::Zeros])
            .unwrap()
            .new_items
            .is_empty()
    );
    let second = parse_zeros(&feed(vec![period("second")])).unwrap();
    assert_eq!(
        db.process_discovered_items(&second, &[GiveawaySource::Zeros])
            .unwrap()
            .new_items
            .len(),
        1
    );
    assert_eq!(db.pending_notifications().unwrap().len(), 1);
    settings.zeros_enabled = false;
    db.update_settings(&settings).unwrap();
    assert!(db.pending_notifications().unwrap().is_empty());
    settings.zeros_enabled = true;
    settings.steam_notifications_enabled = false;
    db.update_settings(&settings).unwrap();
    assert!(db.pending_notifications().unwrap().is_empty());
    let third = parse_zeros(&feed(vec![period("third")])).unwrap();
    assert!(
        db.process_discovered_items(&third, &[GiveawaySource::Zeros])
            .unwrap()
            .new_items
            .is_empty()
    );
    settings.steam_notifications_enabled = true;
    db.update_settings(&settings).unwrap();
    assert_eq!(db.pending_notifications().unwrap().len(), 1);
    assert!(
        db.process_discovered_items(&third, &[GiveawaySource::Zeros])
            .unwrap()
            .new_items
            .is_empty()
    );
    assert_eq!(db.notifications().unwrap().len(), 1);
}

#[test]
fn v5_profile_migration_preserves_history_settings_and_read_state_with_zeros_off() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("v5.db");
    let mut db = Database::open(&path).unwrap();
    let mut settings = db.settings().unwrap();
    settings.initial_sync_mode = Some(InitialSyncMode::NotifyCurrent);
    db.update_settings(&settings).unwrap();
    let mut item = parse_zeros(&feed(vec![period("old")])).unwrap().remove(0);
    item.source = GiveawaySource::GiveeClub;
    db.process_discovered_items(&[item], &[GiveawaySource::GiveeClub])
        .unwrap();
    db.mark_all_notifications_read().unwrap();
    let history = db.notifications().unwrap();
    drop(db);
    rusqlite::Connection::open(&path).unwrap().execute_batch(
        "ALTER TABLE settings DROP COLUMN zeros_enabled; DELETE FROM schema_migrations WHERE version=6;"
    ).unwrap();
    let db = Database::open(&path).unwrap();
    assert_eq!(db.settings().unwrap(), settings);
    assert_eq!(db.notifications().unwrap(), history);
    assert!(
        !db.source_tracking_initialized(GiveawaySource::Zeros)
            .unwrap()
    );
    let mut settings = db.settings().unwrap();
    settings.zeros_enabled = true;
    db.update_settings(&settings).unwrap();
    drop(db);
    assert!(
        Database::open(&path)
            .unwrap()
            .settings()
            .unwrap()
            .zeros_enabled
    );
}

struct Fixture(Result<String, &'static str>);
impl PageFetcher for Fixture {
    async fn get_text(&self, url: &'static str) -> Result<String, PageFetchError> {
        assert_eq!(url, ZEROS_FEED_URL);
        self.0
            .clone()
            .map_err(|message| PageFetchError::Message(message.into()))
    }
}

#[tokio::test]
async fn disabled_zeros_makes_no_request_and_invalid_or_rejected_data_defers_baseline() {
    let mut db = Database::open_in_memory().unwrap();
    let fixture = Fixture(Err("one connection per IP"));
    assert!(
        check_sources(&fixture, false, false, false)
            .await
            .checked_sources
            .is_empty()
    );
    for fixture in [fixture, Fixture(Ok("{}".into()))] {
        let result = check_sources(&fixture, false, false, true).await;
        assert_eq!(result.failures[0].source, GiveawaySource::Zeros);
        assert!(
            apply_source_check(&mut db, result)
                .unwrap()
                .baseline_deferred
        );
        assert!(
            !db.source_tracking_initialized(GiveawaySource::Zeros)
                .unwrap()
        );
    }
    let result = check_sources(&Fixture(Ok(feed(vec![]))), false, false, true).await;
    assert!(result.failures.is_empty());
    apply_source_check(&mut db, result).unwrap();
    assert!(
        db.source_tracking_initialized(GiveawaySource::Zeros)
            .unwrap()
    );
}

#[tokio::test]
#[ignore = "live read-only check; close Zeros tabs first (one connection per IP)"]
async fn live_zeros_public_configuration_is_supported() {
    let result = check_sources(
        &HttpPageFetcher::with_policy(20, 0).unwrap(),
        false,
        false,
        true,
    )
    .await;
    assert!(result.failures.is_empty(), "{:?}", result.failures);
    println!("Zeros: {} available Steam giveaways", result.items.len());
    assert_eq!(result.checked_sources, [GiveawaySource::Zeros]);
}
