use std::fs;

use giveaway_scout_core::{Database, GiveawaySource, InitialSyncMode, parse_givee_club};
use tempfile::tempdir;

#[test]
fn damaged_database_is_preserved_before_a_clean_database_is_created() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("giveaway-scout.db");
    let damaged_contents = b"this is not a SQLite database";
    fs::write(&database_path, damaged_contents).unwrap();

    let (database, recovery) = Database::open_with_recovery(&database_path).unwrap();
    let recovery = recovery.expect("damaged database should produce a recovery report");

    assert_eq!(fs::read(&recovery.backup_path).unwrap(), damaged_contents);
    assert!(recovery.original_error.contains("database"));
    assert_eq!(database.counts().unwrap().tracked_giveaways, 0);
    assert!(database_path.is_file());
}

#[test]
fn healthy_database_does_not_produce_a_recovery_report() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("giveaway-scout.db");

    let (_, recovery) = Database::open_with_recovery(database_path).unwrap();

    assert!(recovery.is_none());
}

#[test]
fn update_checkpoint_preserves_settings_and_does_not_reset_the_profile() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("giveaway-scout.db");
    let mut database = Database::open(&path).unwrap();
    let mut settings = database.settings().unwrap();
    settings.check_interval_minutes = 90;
    settings.language_code = Some("en".to_owned());
    settings.initial_sync_mode = Some(InitialSyncMode::NotifyCurrent);
    database.update_settings(&settings).unwrap();
    let items = parse_givee_club(include_str!("fixtures/html/givee-club.html")).unwrap();
    database
        .process_discovered_items(&items, &[GiveawaySource::GiveeClub])
        .unwrap();
    let id = database.notifications().unwrap()[0].id.clone();
    database.mark_notification_read(&id).unwrap();
    let history = database.notifications().unwrap();
    let before = database.counts().unwrap();
    assert!(before.tracked_giveaways > 0 && before.notification_history > 0);
    assert!(database.checkpoint_before_update().unwrap());
    drop(database);
    let database = Database::open(path).unwrap();
    assert_eq!(database.settings().unwrap(), settings);
    let after = database.counts().unwrap();
    assert_eq!(after.tracked_giveaways, before.tracked_giveaways);
    assert_eq!(after.notification_history, before.notification_history);
    assert_eq!(after.unread_notifications, before.unread_notifications);
    assert_eq!(database.notifications().unwrap(), history);
    assert!(database.tracking_initialized().unwrap());
}
