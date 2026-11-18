//! CLI subprocess checks use only isolated profiles and never access a feed,
//! the installed profile, Windows autostart or the user's PATH.
use giveaway_scout_core::{
    Database, GameStore, GiveawayItem, GiveawayKind, GiveawaySource, InitialSyncMode,
    NotificationRecord,
};
use giveaway_scout_platform::ProfileGuard;
use serde_json::Value;
use std::{
    path::Path,
    process::{Command, Output, Stdio},
};

fn cli(profile: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_giveaway-scout-cli"))
        .args(["--json", "--data-dir"])
        .arg(profile)
        .args(arguments)
        .output()
        .unwrap()
}

fn document(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn notifying_database(profile: &Path) -> Database {
    let database = Database::open(profile.join("giveaway-scout.db")).unwrap();
    let mut settings = database.settings().unwrap();
    settings.initial_sync_mode = Some(InitialSyncMode::NotifyCurrent);
    database.update_settings(&settings).unwrap();
    database
}

fn item(source: GiveawaySource, id: &str, store: GameStore, url: &str) -> GiveawayItem {
    GiveawayItem {
        source,
        store,
        source_id: id.into(),
        title: format!("Игра {id}"),
        url: url.into(),
        kind: GiveawayKind::Giveaway,
        details: None,
    }
}

fn discover(database: &mut Database, item: &GiveawayItem) -> NotificationRecord {
    database
        .process_discovered_items(std::slice::from_ref(item), &[item.source])
        .unwrap();
    database.notifications().unwrap().remove(0)
}

fn assert_only_read_changed(before: &NotificationRecord, after: &NotificationRecord) {
    assert!(after.read_at_utc.is_some());
    let mut normalized = after.clone();
    normalized.read_at_utc = before.read_at_utc.clone();
    assert_eq!(&normalized, before);
}

#[test]
fn help_version_and_errors_do_not_open_a_profile() {
    let directory = tempfile::tempdir().unwrap();
    let profile = directory.path().join("не созданный профиль");
    let output = cli(&profile, &["--help"]);
    assert!(output.status.success());
    assert!(!profile.exists());
    let output = cli(&profile, &["--version"]);
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        concat!("giveaway-scout-cli ", env!("CARGO_PKG_VERSION"))
    );
    assert!(!profile.exists());
    assert_eq!(cli(&profile, &["not-a-command"]).status.code(), Some(2));
    assert!(!profile.exists());
}

#[test]
fn settings_round_trip_utf8_nested_intervals_and_validation() {
    let directory = tempfile::tempdir().unwrap();
    let profile = directory.path().join("Профиль CLI с пробелами");
    let before = document(&cli(&profile, &["config", "show"]));
    assert_eq!(before["autostartTarget"], "gui");
    assert_eq!(before["startAtLogin"], false);
    assert_eq!(before["automaticUpdateChecks"], true);
    for enabled in ["false", "true"] {
        let changed = document(&cli(
            &profile,
            &["config", "set", "automaticUpdateChecks", enabled],
        ));
        assert_eq!(changed["automaticUpdateChecks"], enabled == "true");
        assert_eq!(
            document(&cli(&profile, &["config", "show"]))["automaticUpdateChecks"],
            enabled == "true"
        );
    }
    assert_eq!(
        document(&cli(
            &profile,
            &["config", "set", "sourceCheckIntervals.giveeClub", "15"]
        ))["sourceCheckIntervals"]["giveeClub"],
        15
    );
    let all = document(&cli(
        &profile,
        &["config", "set", "checkIntervalMinutes", "30"],
    ));
    for source in ["freeSteamKeys", "giveeClub", "gamesForFarm", "zeros"] {
        assert_eq!(all["sourceCheckIntervals"][source], 30);
    }
    assert_eq!(
        cli(
            &profile,
            &["config", "set", "sourceCheckIntervals.giveeClub", "1"]
        )
        .status
        .code(),
        Some(2)
    );
    assert_eq!(
        document(&cli(&profile, &["config", "show"]))["sourceCheckIntervals"]["giveeClub"],
        30
    );
    assert_eq!(
        document(&cli(&profile, &["config", "set", "autostartTarget", "cli"]))["autostartTarget"],
        "cli"
    );
    assert_eq!(
        cli(&profile, &["config", "set", "startAtLogin", "true"])
            .status
            .code(),
        Some(2)
    );
}

#[test]
fn occupied_profile_is_refused_then_reopens_after_owner_exits() {
    let directory = tempfile::tempdir().unwrap();
    let guard = ProfileGuard::acquire(directory.path()).unwrap();
    let output = cli(directory.path(), &["config", "show"]);
    assert_eq!(
        output.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!directory.path().join("giveaway-scout.db").exists());
    drop(guard);
    document(&cli(directory.path(), &["config", "show"]));
}

#[test]
fn closed_diagnostic_pipe_preserves_the_usage_exit_code() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_giveaway-scout-cli"))
        .arg("not-a-command")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stderr.take());
    assert_eq!(child.wait().unwrap().code(), Some(2));
}

#[test]
fn history_filters_match_origin_detectors_store_search_and_unread_without_delivery() {
    let directory = tempfile::tempdir().unwrap();
    let mut database = Database::open(directory.path().join("giveaway-scout.db")).unwrap();
    let mut settings = database.settings().unwrap();
    settings.initial_sync_mode = Some(InitialSyncMode::NotifyCurrent);
    database.update_settings(&settings).unwrap();
    let items = [GameStore::Steam, GameStore::Gog].map(|store| GiveawayItem {
        source: GiveawaySource::GiveeClub,
        store,
        source_id: format!("fixture/{}", store.storage_name()),
        title: format!("Игра {}", store.storage_name()),
        url: format!("https://example.invalid/{}", store.storage_name()),
        kind: GiveawayKind::Giveaway,
        details: None,
    });
    database
        .process_discovered_items(&items, &[GiveawaySource::GiveeClub])
        .unwrap();
    let records = database.notifications().unwrap();
    database
        .mark_notification_read(
            &records
                .iter()
                .find(|record| record.store == GameStore::Gog)
                .unwrap()
                .id,
        )
        .unwrap();
    let saved = database.notifications().unwrap();
    drop(database);
    let history = document(&cli(
        directory.path(),
        &[
            "history",
            "--source",
            "giveeClub",
            "--store",
            "steam",
            "--search",
            "ИГРА",
            "--unread",
        ],
    ));
    assert_eq!(history["items"].as_array().unwrap().len(), 1);
    assert_eq!(history["items"][0]["store"], "steam");
    let database = Database::open(directory.path().join("giveaway-scout.db")).unwrap();
    assert_eq!(database.notifications().unwrap(), saved);
}

#[test]
fn read_displays_one_record_then_persists_read_without_recording_delivery() {
    let directory = tempfile::tempdir().unwrap();
    let mut database = notifying_database(directory.path());
    let record = discover(
        &mut database,
        &item(
            GiveawaySource::GiveeClub,
            "read-one",
            GameStore::Steam,
            "https://example.invalid/read-one",
        ),
    );
    let other = discover(
        &mut database,
        &item(
            GiveawaySource::GiveeClub,
            "leave-unread",
            GameStore::Gog,
            "https://example.invalid/leave-unread",
        ),
    );
    database
        .record_notification_delivery_failure(&record.id, "previous delivery failure")
        .unwrap();
    let before = database.notification(&record.id).unwrap().unwrap();
    drop(database);

    let shown = document(&cli(directory.path(), &["read", &record.id]));
    assert_eq!(shown["id"], record.id);
    assert_eq!(shown["title"], record.title);
    assert_eq!(shown["url"], record.url);
    let database = Database::open(directory.path().join("giveaway-scout.db")).unwrap();
    let saved = database.notification(&record.id).unwrap().unwrap();
    assert_only_read_changed(&before, &saved);
    assert_eq!(database.notification(&other.id).unwrap().unwrap(), other);
    assert_eq!(database.pending_notifications().unwrap(), vec![other]);
    drop(database);

    document(&cli(directory.path(), &["read", &record.id]));
    let database = Database::open(directory.path().join("giveaway-scout.db")).unwrap();
    assert_eq!(database.notification(&record.id).unwrap().unwrap(), saved);
}

#[test]
fn read_all_marks_all_sources_and_stores_and_is_idempotent() {
    let directory = tempfile::tempdir().unwrap();
    let mut database = notifying_database(directory.path());
    let mut saved = Vec::new();
    for (source, id, store) in [
        (GiveawaySource::GiveeClub, "steam", GameStore::Steam),
        (GiveawaySource::GiveeClub, "gog", GameStore::Gog),
        (
            GiveawaySource::FreeSteamKeys,
            "other-source",
            GameStore::Steam,
        ),
    ] {
        saved.push(discover(
            &mut database,
            &item(source, id, store, &format!("https://example.invalid/{id}")),
        ));
    }
    database.mark_notification_read(&saved[0].id).unwrap();
    let already_read = database.notification(&saved[0].id).unwrap().unwrap();
    let mut settings = database.settings().unwrap();
    settings.free_steam_keys_enabled = false;
    settings.gog_notifications_enabled = false;
    database.update_settings(&settings).unwrap();
    drop(database);

    assert_eq!(
        document(&cli(directory.path(), &["read", "--all"]))["markedRead"],
        2
    );
    let database = Database::open(directory.path().join("giveaway-scout.db")).unwrap();
    assert_eq!(database.counts().unwrap().unread_notifications, 0);
    assert_eq!(
        database.notification(&saved[0].id).unwrap().unwrap(),
        already_read
    );
    for before in &saved[1..] {
        assert_only_read_changed(before, &database.notification(&before.id).unwrap().unwrap());
    }
    let after = database.notifications().unwrap();
    drop(database);

    assert_eq!(
        document(&cli(directory.path(), &["read", "--all"]))["markedRead"],
        0
    );
    let database = Database::open(directory.path().join("giveaway-scout.db")).unwrap();
    assert_eq!(database.notifications().unwrap(), after);
}

#[test]
fn read_by_a_saved_alias_marks_the_merged_campaign_once() {
    let directory = tempfile::tempdir().unwrap();
    let mut database = notifying_database(directory.path());
    let aggregate_item = item(
        GiveawaySource::FreeSteamKeys,
        "aggregate",
        GameStore::Steam,
        "https://www.freesteamkeys.com/open/aggregate/",
    );
    let aggregate = discover(&mut database, &aggregate_item);
    let origin_item = item(
        GiveawaySource::GiveeClub,
        "event/42",
        GameStore::Steam,
        "https://givee.club/event/42",
    );
    let origin = discover(&mut database, &origin_item);
    assert_ne!(origin.id, aggregate.id);
    database
        .apply_resolved_url(&aggregate_item.url, &origin_item.url)
        .unwrap();
    assert_eq!(database.notifications().unwrap().len(), 1);
    drop(database);

    document(&cli(directory.path(), &["read", &origin.id]));
    let database = Database::open(directory.path().join("giveaway-scout.db")).unwrap();
    let aggregate = database.notification(&aggregate.id).unwrap().unwrap();
    let origin = database.notification(&origin.id).unwrap().unwrap();
    assert!(aggregate.read_at_utc.is_some());
    assert_eq!(aggregate.read_at_utc, origin.read_at_utc);
    assert_eq!(database.counts().unwrap().unread_notifications, 0);
    assert!(database.pending_notifications().unwrap().is_empty());
}

#[test]
fn unknown_ids_and_unsafe_open_urls_leave_history_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let mut database = notifying_database(directory.path());
    // Every open request in this test is rejected before launching a browser.
    let unsafe_record = discover(
        &mut database,
        &item(
            GiveawaySource::FreeSteamKeys,
            "unsafe",
            GameStore::Steam,
            "file:///C:/should-not-open.txt",
        ),
    );
    assert_eq!(unsafe_record.url, "file:///C:/should-not-open.txt");
    let saved = database.notifications().unwrap();
    drop(database);

    for arguments in [
        ["read", "missing-id"],
        ["open", "missing-id"],
        ["open", unsafe_record.id.as_str()],
    ] {
        let output = cli(directory.path(), &arguments);
        assert_eq!(
            output.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty());
    }
    let database = Database::open(directory.path().join("giveaway-scout.db")).unwrap();
    assert_eq!(database.notifications().unwrap(), saved);
}

#[test]
fn invalid_read_open_arguments_do_not_create_a_profile() {
    let directory = tempfile::tempdir().unwrap();
    let profile = directory.path().join("not-created");
    for arguments in [
        vec!["read"],
        vec!["open"],
        vec!["read", "first", "second"],
        vec!["open", "first", "second"],
        vec!["read", "first", "--all"],
        vec!["read", "--all", "--unread"],
        vec!["read", "--all", "--source", "giveeClub"],
        vec!["open", "--all"],
    ] {
        let output = cli(&profile, &arguments);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{arguments:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!profile.exists());
    }
}

#[test]
fn read_and_open_refuse_an_occupied_profile_before_accessing_history() {
    let directory = tempfile::tempdir().unwrap();
    let _guard = ProfileGuard::acquire(directory.path()).unwrap();
    for arguments in [
        ["read", "missing-id"],
        ["read", "--all"],
        ["open", "missing-id"],
    ] {
        let output = cli(directory.path(), &arguments);
        assert_eq!(
            output.status.code(),
            Some(3),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!directory.path().join("giveaway-scout.db").exists());
    }
}

#[test]
fn text_history_includes_full_ids_and_localized_read_status_without_changing_records() {
    let directory = tempfile::tempdir().unwrap();
    let mut database = notifying_database(directory.path());
    let read = discover(
        &mut database,
        &item(
            GiveawaySource::GiveeClub,
            "already-read",
            GameStore::Steam,
            "https://example.invalid/already-read",
        ),
    );
    database.mark_notification_read(&read.id).unwrap();
    let unread = discover(
        &mut database,
        &item(
            GiveawaySource::GiveeClub,
            "still-unread",
            GameStore::Steam,
            "https://example.invalid/still-unread",
        ),
    );
    let saved = database.notifications().unwrap();
    drop(database);

    for (language, read_label, unread_label) in [
        ("en", "[read]", "[unread]"),
        ("ru", "[прочитано]", "[не прочитано]"),
    ] {
        document(&cli(
            directory.path(),
            &["config", "set", "languageCode", language],
        ));
        let output = Command::new(env!("CARGO_BIN_EXE_giveaway-scout-cli"))
            .arg("--data-dir")
            .arg(directory.path())
            .arg("history")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains(&read.id), "{text}");
        assert!(text.contains(&unread.id), "{text}");
        assert!(text.contains(read_label), "{text}");
        assert!(text.contains(unread_label), "{text}");
    }
    let database = Database::open(directory.path().join("giveaway-scout.db")).unwrap();
    assert_eq!(database.notifications().unwrap(), saved);
}
