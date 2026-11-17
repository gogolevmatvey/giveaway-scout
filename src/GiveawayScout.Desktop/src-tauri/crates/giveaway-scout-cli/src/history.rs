use std::io::Write;

use giveaway_scout_core::{Database, NotificationRecord};

fn find(database: &Database, id: &str) -> Result<NotificationRecord, String> {
    database
        .notification(id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("History record not found: {id}"))
}

pub fn read(
    database: &Database,
    id: &str,
    writer: &mut impl Write,
    json: bool,
) -> Result<(), String> {
    let record = find(database, id)?;
    crate::output::read_record(writer, &record, json)?;
    database
        .mark_notification_read(id)
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub fn open_with(
    database: &Database,
    id: &str,
    launch: impl FnOnce(&str) -> Result<(), String>,
) -> Result<String, String> {
    let record = find(database, id)?;
    let url =
        url::Url::parse(&record.url).map_err(|error| format!("Invalid giveaway URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("Only HTTP(S) giveaway URLs can be opened".to_owned());
    }
    launch(url.as_str())?;
    database
        .mark_notification_read(id)
        .map_err(|error| error.to_string())?;
    Ok(url.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use giveaway_scout_core::{
        GameStore, GiveawayItem, GiveawayKind, GiveawaySource, InitialSyncMode,
    };
    use std::{cell::Cell, io};

    fn seed(url: &str) -> (Database, NotificationRecord) {
        let mut database = Database::open_in_memory().unwrap();
        let mut settings = database.settings().unwrap();
        settings.initial_sync_mode = Some(InitialSyncMode::NotifyCurrent);
        database.update_settings(&settings).unwrap();
        database
            .process_discovered_items(
                &[GiveawayItem {
                    source: GiveawaySource::GiveeClub,
                    source_id: "read-fixture".into(),
                    title: "Тестовая игра".into(),
                    url: url.into(),
                    kind: GiveawayKind::Giveaway,
                    store: GameStore::Steam,
                    details: None,
                }],
                &[GiveawaySource::GiveeClub],
            )
            .unwrap();
        let record = database.notifications().unwrap().remove(0);
        (database, record)
    }

    struct BrokenOutput {
        flush_fails: bool,
    }
    impl Write for BrokenOutput {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.flush_fails {
                Ok(bytes.len())
            } else {
                Err(io::ErrorKind::BrokenPipe.into())
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
    }

    #[test]
    fn failed_write_or_flush_does_not_mark_read_in_either_format() {
        for json in [false, true] {
            for flush_fails in [false, true] {
                let (database, record) = seed("https://example.invalid/game");
                assert!(
                    read(
                        &database,
                        &record.id,
                        &mut BrokenOutput { flush_fails },
                        json
                    )
                    .is_err()
                );
                assert_eq!(database.notification(&record.id).unwrap().unwrap(), record);
                let mut output = Vec::new();
                read(&database, &record.id, &mut output, json).unwrap();
                let updated = database.notification(&record.id).unwrap().unwrap();
                assert!(updated.read_at_utc.is_some());
                assert_eq!(updated.delivery_state, record.delivery_state);
                assert_eq!(updated.delivery_attempts, record.delivery_attempts);
                if json {
                    let document: serde_json::Value = serde_json::from_slice(&output).unwrap();
                    assert_eq!(document["id"], record.id);
                    assert!(document.get("readAtUtc").is_none());
                }
            }
        }
    }

    #[test]
    fn browser_failure_preserves_unread_and_success_preserves_delivery() {
        let (database, before) = seed("https://example.invalid/Игра");
        assert!(open_with(&database, &before.id, |_| Err("No browser".into())).is_err());
        assert_eq!(database.notification(&before.id).unwrap().unwrap(), before);
        let url = open_with(&database, &before.id, |url| {
            assert_eq!(url, "https://example.invalid/%D0%98%D0%B3%D1%80%D0%B0");
            assert!(
                database
                    .notification(&before.id)
                    .unwrap()
                    .unwrap()
                    .read_at_utc
                    .is_none()
            );
            Ok(())
        })
        .unwrap();
        assert!(url.starts_with("https://"));
        let mut after = database.notification(&before.id).unwrap().unwrap();
        assert!(after.read_at_utc.take().is_some());
        assert_eq!(after, before);
    }

    #[test]
    fn unsafe_url_and_missing_id_never_reach_launcher() {
        for url in [
            "file:///C:/Windows/notepad.exe",
            "javascript:alert(1)",
            "not a URL",
        ] {
            let (database, before) = seed(url);
            let called = Cell::new(false);
            assert!(
                open_with(&database, &before.id, |_| {
                    called.set(true);
                    Ok(())
                })
                .is_err()
            );
            assert!(!called.get());
            assert_eq!(database.notification(&before.id).unwrap().unwrap(), before);
            assert!(
                open_with(&database, "missing", |_| {
                    called.set(true);
                    Ok(())
                })
                .is_err()
            );
            assert!(!called.get());
        }
    }
}
