use std::collections::HashSet;

use giveaway_scout_core::{
    Database, GameStore, GiveawayItem, GiveawayKind, GiveawaySource, InitialSyncMode,
    NotificationRecord, StorageError,
};
use rusqlite::Connection;

fn item(source: GiveawaySource, id: &str) -> GiveawayItem {
    GiveawayItem {
        source,
        source_id: id.into(),
        title: id.into(),
        url: format!("https://example.com/{}/{id}", source.storage_name()),
        kind: GiveawayKind::Giveaway,
        store: GameStore::Steam,
        details: None,
    }
}

fn notifying_database(path: &std::path::Path) -> Database {
    let database = Database::open(path).unwrap();
    let mut settings = database.settings().unwrap();
    settings.initial_sync_mode = Some(InitialSyncMode::NotifyCurrent);
    settings.zeros_enabled = true;
    database.update_settings(&settings).unwrap();
    database
}

fn discover(database: &mut Database, items: &[GiveawayItem]) {
    database
        .process_discovered_items(items, &GiveawaySource::ALL)
        .unwrap();
}

fn all_pages(database: &Database) -> Vec<NotificationRecord> {
    let mut cursor = None;
    let mut records = Vec::new();
    loop {
        let page = database
            .notification_history_page(cursor.as_deref())
            .unwrap();
        assert!(page.items.len() <= 50);
        if page.next_cursor.is_some() {
            assert_eq!(page.items.len(), 50);
        }
        records.extend(page.items);
        cursor = page.next_cursor;
        if cursor.is_none() {
            return records;
        }
    }
}

#[test]
fn pages_match_full_history_across_batches_sources_legacy_rows_and_null_positions() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("profile.db");
    let mut database = notifying_database(&path);
    for batch in 0..3 {
        let items: Vec<_> = (0..137)
            .map(|index| {
                item(
                    GiveawaySource::ALL[index % GiveawaySource::ALL.len()],
                    &format!("batch-{batch}-item-{index:03}"),
                )
            })
            .collect();
        discover(&mut database, &items);
    }
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "UPDATE notification_history SET received_at_utc='2026-01-01T00:00:00Z';
         UPDATE notification_history SET discovery_batch=NULL,source_position=NULL
            WHERE title LIKE 'batch-0-%';
         UPDATE notification_history SET source_position=NULL
            WHERE title LIKE '%item-00%';
         UPDATE notification_history SET source_position=17
            WHERE title LIKE '%item-01%';",
        )
        .unwrap();
    drop(connection);
    let expected = database.notifications().unwrap();
    let paged = all_pages(&database);
    assert_eq!(paged, expected);
    assert_eq!(paged.len(), 411);
    assert_eq!(
        paged
            .iter()
            .map(|record| &record.id)
            .collect::<HashSet<_>>()
            .len(),
        paged.len()
    );
    drop(database);
    assert_eq!(all_pages(&Database::open(&path).unwrap()), expected);
}

#[test]
fn empty_and_exact_page_boundary_have_no_unnecessary_next_request() {
    let directory = tempfile::tempdir().unwrap();
    let mut database = notifying_database(&directory.path().join("profile.db"));
    let empty = database.notification_history_page(None).unwrap();
    assert!(empty.items.is_empty());
    assert!(empty.next_cursor.is_none());
    discover(
        &mut database,
        &(0..50)
            .map(|index| item(GiveawaySource::GiveeClub, &index.to_string()))
            .collect::<Vec<_>>(),
    );
    let page = database.notification_history_page(None).unwrap();
    assert_eq!(page.items.len(), 50);
    assert!(page.next_cursor.is_none());
}

#[test]
fn new_batch_between_pages_does_not_repeat_or_skip_original_cards() {
    let directory = tempfile::tempdir().unwrap();
    let mut database = notifying_database(&directory.path().join("profile.db"));
    discover(
        &mut database,
        &(0..123)
            .map(|index| item(GiveawaySource::GiveeClub, &format!("old-{index}")))
            .collect::<Vec<_>>(),
    );
    let expected = database.notifications().unwrap();
    let first = database.notification_history_page(None).unwrap();
    discover(&mut database, &[item(GiveawaySource::GiveeClub, "new")]);
    let second = database
        .notification_history_page(first.next_cursor.as_deref())
        .unwrap();
    let third = database
        .notification_history_page(second.next_cursor.as_deref())
        .unwrap();
    let paged: Vec<_> = first
        .items
        .into_iter()
        .chain(second.items)
        .chain(third.items)
        .collect();
    assert_eq!(paged, expected);
    assert!(third.next_cursor.is_none());
    assert_eq!(
        database.notification_history_page(None).unwrap().items[0].title,
        "new"
    );
}

#[test]
fn cursor_survives_hidden_or_deleted_boundary_row_and_read_changes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("profile.db");
    let mut database = notifying_database(&path);
    discover(
        &mut database,
        &(0..123)
            .map(|index| item(GiveawaySource::GiveeClub, &format!("item-{index}")))
            .collect::<Vec<_>>(),
    );
    let expected = database.notifications().unwrap();
    let first = database.notification_history_page(None).unwrap();
    let boundary_id = &first.items.last().unwrap().id;
    database.mark_all_notifications_read().unwrap();
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE campaigns SET anchor_history_id=NULL WHERE anchor_history_id=?1",
            [boundary_id],
        )
        .unwrap();
    connection
        .execute(
            "DELETE FROM notification_campaigns WHERE notification_id=?1",
            [boundary_id],
        )
        .unwrap();
    connection
        .execute(
            "DELETE FROM notification_history WHERE id=?1",
            [boundary_id],
        )
        .unwrap();
    drop(connection);
    let second = database
        .notification_history_page(first.next_cursor.as_deref())
        .unwrap();
    assert_eq!(
        second
            .items
            .iter()
            .map(|record| &record.id)
            .collect::<Vec<_>>(),
        expected[50..100]
            .iter()
            .map(|record| &record.id)
            .collect::<Vec<_>>()
    );
    assert!(
        second
            .items
            .iter()
            .all(|record| record.read_at_utc.is_some())
    );
    database.clear_giveaway_data().unwrap();
    let cleared = database
        .notification_history_page(second.next_cursor.as_deref())
        .unwrap();
    assert!(cleared.items.is_empty());
    assert!(cleared.next_cursor.is_none());
}

#[test]
fn merging_the_boundary_card_keeps_continuation_valid_and_aliases_hidden() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("profile.db");
    let mut database = notifying_database(&path);
    let mut origin = item(GiveawaySource::GiveeClub, "event/42");
    origin.url = "https://givee.club/en/event/42".into();
    discover(&mut database, &[origin.clone()]);
    let mut items: Vec<_> = (0..122)
        .map(|index| item(GiveawaySource::FreeSteamKeys, &format!("item-{index}")))
        .collect();
    items[49].url = "https://www.freesteamkeys.com/open/42/".into();
    let aggregate_url = items[49].url.clone();
    discover(&mut database, &items);
    let first = database.notification_history_page(None).unwrap();
    let boundary = first.items.last().unwrap();
    assert_eq!(boundary.url, aggregate_url);
    database
        .apply_resolved_url(&aggregate_url, &origin.url)
        .unwrap();
    let second = database
        .notification_history_page(first.next_cursor.as_deref())
        .unwrap();
    let third = database
        .notification_history_page(second.next_cursor.as_deref())
        .unwrap();
    assert_eq!(second.items.len(), 50);
    assert_eq!(
        third.items.len(),
        23,
        "the surviving older anchor remains in history"
    );
    let surviving = database.notifications().unwrap();
    let remaining: Vec<_> = second
        .items
        .into_iter()
        .chain(third.items)
        .map(|record| record.id)
        .collect();
    let expected: Vec<_> = surviving
        .iter()
        .filter(|record| !first.items.iter().any(|loaded| loaded.id == record.id))
        .map(|record| record.id.clone())
        .collect();
    assert_eq!(remaining, expected);
    assert!(surviving.iter().all(|record| record.id != boundary.id));
}

#[test]
fn malformed_oversized_and_out_of_range_cursors_fail_without_reading_history() {
    let directory = tempfile::tempdir().unwrap();
    let mut database = notifying_database(&directory.path().join("profile.db"));
    discover(
        &mut database,
        &(0..51)
            .map(|index| item(GiveawaySource::GiveeClub, &index.to_string()))
            .collect::<Vec<_>>(),
    );
    let valid = database
        .notification_history_page(None)
        .unwrap()
        .next_cursor
        .unwrap();
    for invalid in [
        "".to_owned(),
        "null".to_owned(),
        "{}".to_owned(),
        "x".repeat(2_049),
        valid.replace("\"version\":1", "\"version\":2"),
        valid.replace("\"batch\":1", "\"batch\":-1"),
    ] {
        assert!(
            matches!(
                database.notification_history_page(Some(&invalid)),
                Err(StorageError::InvalidHistoryCursor)
            ),
            "{invalid}"
        );
    }
    let json = serde_json::to_value(database.notification_history_page(None).unwrap()).unwrap();
    assert_eq!(json["items"].as_array().unwrap().len(), 50);
    assert!(json["nextCursor"].is_string());
    assert!(json.get("next_cursor").is_none());
}
