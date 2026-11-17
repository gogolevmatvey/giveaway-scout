use giveaway_scout_core::{AppSettings, NotificationRecord};
use serde::Serialize;
use std::io::{self, Write};

/// Flush before acknowledging delivery. stdout and SQLite cannot be committed
/// atomically, so an interrupted process can retry a partially printed record.
pub fn notification(
    writer: &mut impl Write,
    record: &NotificationRecord,
    json: bool,
) -> Result<(), String> {
    if json {
        serde_json::to_writer(
            &mut *writer,
            &serde_json::json!({"type":"notification","record":record}),
        )
        .map_err(|error| error.to_string())?;
        writeln!(writer).map_err(|error| error.to_string())?;
    } else {
        writeln!(
            writer,
            "ID: {}\n[{}] {} ({}, {})\n{}",
            one_line(&record.id),
            record.received_at_utc,
            one_line(&record.title),
            record.store.display_name().unwrap_or("Unknown"),
            one_line(record.origin_name.as_deref().unwrap_or(&record.source)),
            one_line(&record.url)
        )
        .map_err(|error| error.to_string())?;
    }
    writer.flush().map_err(|error| error.to_string())
}

pub fn history_record(
    writer: &mut impl Write,
    record: &NotificationRecord,
    ru: bool,
) -> Result<(), String> {
    let status = if record.read_at_utc.is_some() {
        text(ru, "прочитано", "read")
    } else {
        text(ru, "не прочитано", "unread")
    };
    write!(writer, "[{status}] ").map_err(|error| error.to_string())?;
    notification(writer, record, false)
}

pub fn read_record(
    writer: &mut impl Write,
    record: &NotificationRecord,
    json: bool,
) -> Result<(), String> {
    if !json {
        return notification(writer, record, false);
    }
    let mut contents = serde_json::to_value(record).map_err(|error| error.to_string())?;
    // The read receipt is committed only after this document is flushed. Do not
    // present the old receipt as the result of the command.
    if let Some(fields) = contents.as_object_mut() {
        fields.remove("readAtUtc");
    }
    serde_json::to_writer(&mut *writer, &contents).map_err(|error| error.to_string())?;
    writeln!(writer).map_err(|error| error.to_string())?;
    writer.flush().map_err(|error| error.to_string())
}

fn one_line(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

pub fn safe_notes(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() && !matches!(character, '\n' | '\t') {
                ' '
            } else {
                character
            }
        })
        .collect()
}

pub fn json(value: &impl Serialize) -> Result<(), String> {
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    serde_json::to_writer(&mut writer, value).map_err(|error| error.to_string())?;
    writeln!(writer).map_err(|error| error.to_string())?;
    writer.flush().map_err(|error| error.to_string())
}

pub fn russian(settings: &AppSettings) -> bool {
    match settings.language_code.as_deref() {
        Some("ru") => true,
        Some("en") => false,
        _ => sys_locale::get_locale().is_some_and(|locale| locale.to_lowercase().starts_with("ru")),
    }
}

pub fn text(ru: bool, russian: &'static str, english: &'static str) -> &'static str {
    if ru { russian } else { english }
}

#[cfg(test)]
mod tests {
    use super::*;
    use giveaway_scout_core::{
        Database, GameStore, GiveawayItem, GiveawayKind, GiveawaySource, InitialSyncMode,
        dispatch_pending_notifications,
    };
    use std::sync::Mutex;
    #[test]
    fn terminal_text_does_not_inject_control_sequences() {
        assert_eq!(one_line("Game\n\u{1b}[31m"), "Game  [31m");
        assert_eq!(
            safe_notes("Notes\n\u{1b}]8;;evil\u{7}"),
            "Notes\n ]8;;evil "
        );
    }

    #[test]
    fn failed_flush_stays_pending_and_successful_retry_preserves_unread() {
        struct BrokenFlush(Vec<u8>);
        impl Write for BrokenFlush {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "test pipe closed",
                ))
            }
        }
        let mut database = Database::open_in_memory().unwrap();
        let mut settings = database.settings().unwrap();
        settings.initial_sync_mode = Some(InitialSyncMode::NotifyCurrent);
        database.update_settings(&settings).unwrap();
        database
            .process_discovered_items(
                &[GiveawayItem {
                    source: GiveawaySource::GiveeClub,
                    source_id: "stdout-test".into(),
                    title: "Игра".into(),
                    url: "https://example.invalid/game".into(),
                    kind: GiveawayKind::Giveaway,
                    store: GameStore::Steam,
                    details: None,
                }],
                &[GiveawaySource::GiveeClub],
            )
            .unwrap();
        let database = Mutex::new(database);
        let mut broken = BrokenFlush(Vec::new());
        let report = dispatch_pending_notifications(&database, |record, _| {
            notification(&mut broken, record, true)
        })
        .unwrap();
        assert_eq!(report.failed, 1);
        let pending = database.lock().unwrap().pending_notifications().unwrap();
        assert_eq!(pending.len(), 1);
        assert!(pending[0].read_at_utc.is_none());
        let mut output = Vec::new();
        let report = dispatch_pending_notifications(&database, |record, _| {
            notification(&mut output, record, true)
        })
        .unwrap();
        assert_eq!(report.delivered, 1);
        assert!(
            database
                .lock()
                .unwrap()
                .pending_notifications()
                .unwrap()
                .is_empty()
        );
        assert!(
            database.lock().unwrap().notifications().unwrap()[0]
                .read_at_utc
                .is_none()
        );
        let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(value["type"], "notification");
        assert_eq!(value["record"]["title"], "Игра");
    }
}
