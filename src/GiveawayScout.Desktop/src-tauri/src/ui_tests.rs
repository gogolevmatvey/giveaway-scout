//! Compiled only with ui-tests; all OS effects stay inside a runner-created temp root.
use std::{
    cell::RefCell,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock, mpsc},
    time::Duration,
};

use giveaway_scout_core::{AppSettings, InitialSyncMode, PageFetchError, PageFetcher};
use serde::{Deserialize, Serialize};
use tauri::Manager;

pub(crate) const IDENTIFIER: &str = "com.giveawayscout.desktop.e2e.dev";
const MARKER: &str = "Giveaway Scout isolated UI tests v1\n";
static PROFILE: OnceLock<Profile> = OnceLock::new();
static ATTENTION_RACE_CLAIM: Mutex<()> = Mutex::new(());

struct AttentionRace {
    token: String,
    finished: mpsc::Sender<()>,
}

thread_local! {
    // sync_attention has no await points, so its before/after hooks always run
    // on this same worker. Other source completions cannot satisfy its guard.
    static ATTENTION_RACE: RefCell<Option<AttentionRace>> = const { RefCell::new(None) };
}

pub(crate) struct Profile {
    pub root: PathBuf,
    pub port: u16,
    events_lock: Mutex<()>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Scenario {
    Baseline,
    NewGiveaway,
    SourceFailure,
    GamesForFarmFailure,
    GamesForFarmInvalidFeed,
    GamesForFarmEmpty,
    ZerosGiveaway,
    ZerosConnectionLimit,
    OrderFirst,
    OrderSecond,
    FreeSteamKeysGiveaway,
    FreeSteamKeysFailure,
    CrossSourceGiveaway,
    SlowGivee,
    LargeHistory,
    LargeHistoryNew,
}

#[derive(Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum Event<'a> {
    Notification {
        title: &'a str,
        activation: Option<&'a str>,
        sound: bool,
    },
    OpenUrl {
        url: &'a str,
    },
    Attention {
        unread: u64,
    },
    AttentionWindowQuery {
        focused: bool,
    },
    AttentionRaceStarted {
        token: &'a str,
    },
    AttentionRaceMainThreadEntered {
        token: &'a str,
        background_worker: bool,
    },
    AttentionRaceMainThreadPolled {
        token: &'a str,
        unread: u64,
        history_records: usize,
    },
    AttentionRaceCompleted {
        token: &'a str,
        focused: bool,
    },
    AttentionRaceFailed {
        token: &'a str,
        message: &'a str,
    },
}

pub(crate) fn validate_root(root: &Path, temp: &Path) -> Result<PathBuf, String> {
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let temp = temp.canonicalize().map_err(|e| e.to_string())?;
    let name = root
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("invalid test root")?;
    let suffix = name
        .strip_prefix("giveaway-scout-e2e-")
        .ok_or("invalid test root prefix")?;
    uuid::Uuid::parse_str(suffix).map_err(|_| "test root must contain a UUID")?;
    if root.parent() != Some(temp.as_path()) || !root.is_dir() {
        return Err("test root must be an immediate child of the OS temp directory".into());
    }
    let marker = root.join(".giveaway-scout-ui-test");
    if fs::symlink_metadata(&marker)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_symlink()
        || fs::read_to_string(marker).map_err(|e| e.to_string())? != MARKER
    {
        return Err("missing test root ownership marker".into());
    }
    // Never follow pre-existing profile subdirectory links into a real profile.
    for name in ["data", "logs", "webview", "events.jsonl", "scenario.json"] {
        let path = root.join(name);
        if path.exists()
            && (fs::symlink_metadata(&path)
                .map_err(|e| e.to_string())?
                .file_type()
                .is_symlink()
                || !path
                    .canonicalize()
                    .map_err(|e| e.to_string())?
                    .starts_with(&root))
        {
            return Err("test profile contains a link outside its root".into());
        }
    }
    Ok(root)
}

pub(crate) fn initialize(identifier: &str) -> Result<&'static Profile, String> {
    if identifier != IDENTIFIER {
        return Err("ui-tests requires the dedicated Tauri config".into());
    }
    let root =
        std::env::var_os("GIVEAWAY_SCOUT_UI_ROOT").ok_or("missing GIVEAWAY_SCOUT_UI_ROOT")?;
    let root = validate_root(Path::new(&root), &std::env::temp_dir())?;
    let port: u16 = std::env::var("TAURI_WEBDRIVER_PORT")
        .map_err(|_| "missing test WebDriver port")?
        .parse()
        .map_err(|_| "invalid test WebDriver port")?;
    if port < 1024 {
        return Err("test WebDriver port must be unprivileged".into());
    }
    for name in ["data", "logs", "webview"] {
        fs::create_dir_all(root.join(name)).map_err(|e| e.to_string())?;
    }
    PROFILE
        .set(Profile {
            root,
            port,
            events_lock: Mutex::new(()),
        })
        .map_err(|_| "test profile already initialized")?;
    Ok(profile())
}

pub(crate) fn profile() -> &'static Profile {
    PROFILE
        .get()
        .expect("test profile must be validated before app startup")
}

pub(crate) fn log_directory(settings: &AppSettings) -> PathBuf {
    owned_log_directory(&profile().root, settings.log_directory.as_deref())
}

fn owned_log_directory(root: &Path, requested: Option<&str>) -> PathBuf {
    // A native picker may select another directory in this owned profile, but
    // test settings can never redirect a writer into the installed app's data.
    if let Some(requested) = requested {
        let requested = Path::new(requested);
        if requested.is_absolute()
            && let Ok(resolved) = requested.canonicalize()
            && resolved.starts_with(root)
        {
            return resolved;
        }
    }
    root.join("logs")
}

#[tauri::command]
pub(crate) fn get_ui_test_identity() -> serde_json::Value {
    serde_json::json!({
        "processId": std::process::id(),
        "profileId": profile().root.file_name().unwrap().to_str().unwrap(),
    })
}

pub(crate) fn seed_settings() -> AppSettings {
    AppSettings {
        initial_sync_mode: Some(InitialSyncMode::Silent),
        language_code: Some("en".into()),
        ..AppSettings::default()
    }
}

pub(crate) fn record(event: Event<'_>) -> Result<(), String> {
    let profile = profile();
    let _guard = profile
        .events_lock
        .lock()
        .map_err(|_| "test events lock unavailable")?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(profile.root.join("events.jsonl"))
        .map_err(|e| e.to_string())?;
    let line = serde_json::to_vec(&event).map_err(|e| e.to_string())?;
    file.write_all(&line)
        .and_then(|()| file.write_all(b"\n"))
        .map_err(|e| e.to_string())
}

/// Arrange the exact historic lock inversion in a real Tauri event loop. The
/// runner arms this only after the silent baseline in an isolated profile.
/// The main thread starts the real storage/history/logging commands before the
/// source worker proceeds to the production is_focused getter. A caller still
/// holding the database would block both threads. No timing sleeps decide the
/// ordering, and a watchdog terminates only this validated test process if the
/// old cycle is reintroduced.
pub(crate) fn before_attention_window_query(app: &tauri::AppHandle) -> Result<(), String> {
    let token = {
        let _claim = ATTENTION_RACE_CLAIM
            .lock()
            .map_err(|_| "attention regression claim lock unavailable")?;
        let arm = profile().root.join("attention-race-arm.json");
        let contents = match fs::read(&arm) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.to_string()),
        };
        let token: String = serde_json::from_slice(&contents).map_err(|e| e.to_string())?;
        uuid::Uuid::parse_str(&token).map_err(|_| "invalid attention regression token")?;
        fs::remove_file(arm).map_err(|e| e.to_string())?;
        token
    };
    record(Event::AttentionRaceStarted { token: &token })?;
    let (finished, watchdog) = mpsc::channel();
    let watchdog_token = token.clone();
    std::thread::spawn(move || {
        if matches!(
            watchdog.recv_timeout(Duration::from_secs(20)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ) {
            let message = "real window attention/storage regression timed out";
            let _ = record(Event::AttentionRaceFailed {
                token: &watchdog_token,
                message,
            });
            eprintln!("UI_TEST_ATTENTION_DEADLOCK: {message} ({watchdog_token})");
            std::process::exit(86);
        }
    });
    ATTENTION_RACE.with(|race| {
        *race.borrow_mut() = Some(AttentionRace {
            token: token.clone(),
            finished,
        });
    });
    let (entered, main_thread_entered) = mpsc::sync_channel(1);
    let main_app = app.clone();
    let source_thread = std::thread::current().id();
    app.run_on_main_thread(move || {
        let result = (|| {
            let background_worker = source_thread != std::thread::current().id();
            record(Event::AttentionRaceMainThreadEntered {
                token: &token,
                background_worker,
            })?;
            // This acknowledgement precedes the storage lock. If the source
            // worker owns it, this main thread cannot service is_focused.
            entered.send(()).map_err(|error| error.to_string())?;
            if !background_worker {
                return Err("attention regression must start on a background worker".to_owned());
            }
            let snapshot = crate::get_storage_snapshot(main_app.state(), main_app.state())?;
            let history = crate::get_notification_history(main_app.state())?;
            crate::get_logging_state(main_app.state(), main_app.state())?;
            record(Event::AttentionRaceMainThreadPolled {
                token: &token,
                unread: snapshot.counts.unread_notifications,
                history_records: history.len(),
            })
        })();
        if let Err(message) = result {
            let _ = record(Event::AttentionRaceFailed {
                token: &token,
                message: &message,
            });
        }
    })
    .map_err(|error| error.to_string())?;
    main_thread_entered
        .recv_timeout(Duration::from_secs(5))
        .map_err(|error| error.to_string())
}

pub(crate) fn after_attention_window_query(focused: bool) -> Result<(), String> {
    record(Event::AttentionWindowQuery { focused })?;
    ATTENTION_RACE.with(|race| {
        if let Some(race) = race.borrow_mut().take() {
            record(Event::AttentionRaceCompleted {
                token: &race.token,
                focused,
            })?;
            race.finished.send(()).map_err(|error| error.to_string())?;
        }
        Ok(())
    })
}

#[derive(Clone)]
pub(crate) struct UiPageFetcher {
    root: PathBuf,
}

impl UiPageFetcher {
    pub(crate) fn with_policy(_timeout: u32, _retries: u32) -> Result<Self, PageFetchError> {
        Ok(Self {
            root: profile().root.clone(),
        })
    }
}

fn large_history_page(include_new: bool) -> String {
    let mut html = include_str!("../ui-fixtures/givee-baseline.html").to_owned();
    if include_new {
        html.push_str("<a class=\"event-card\" href=\"/event/990240\"><div class=\"event-title\">New arrival while reading older history</div><i class=\"event-platform-steam\"></i></a>");
    }
    for index in 0..240 {
        let extra = if index % 5 == 0 {
            " — A long title with spaces для проверки переноса".repeat(7)
        } else {
            String::new()
        };
        html.push_str(&format!(
            "<a class=\"event-card\" href=\"/event/{}\"><div class=\"event-title\">History fixture {index}{extra}</div><i class=\"event-platform-steam\"></i></a>",
            990000 + index
        ));
    }
    html
}

impl PageFetcher for UiPageFetcher {
    async fn get_text(&self, url: &'static str) -> Result<String, PageFetchError> {
        let scenario = fs::read(self.root.join("scenario.json"))
            .map_err(|e| PageFetchError::Message(e.to_string()))?;
        let scenario: Scenario = serde_json::from_slice(&scenario)
            .map_err(|e| PageFetchError::Message(e.to_string()))?;
        if url == "https://givee.club/ru" && matches!(scenario, Scenario::SlowGivee) {
            tokio::time::sleep(std::time::Duration::from_secs(4)).await;
        }
        match url {
            "https://givee.club/ru"
                if matches!(scenario, Scenario::LargeHistory | Scenario::LargeHistoryNew) =>
            {
                Ok(large_history_page(matches!(scenario, Scenario::LargeHistoryNew)))
            }
            "https://givee.club/ru" if matches!(scenario, Scenario::SourceFailure) => Err(
                PageFetchError::Connection("UI_TEST_SOURCE_UNAVAILABLE".into()),
            ),
            "https://givee.club/ru"
                if matches!(scenario, Scenario::OrderFirst | Scenario::OrderSecond) =>
            {
                let first = include_str!("../ui-fixtures/givee-order-first.html");
                Ok(if matches!(scenario, Scenario::OrderSecond) {
                    format!(
                        "{}{first}",
                        include_str!("../ui-fixtures/givee-order-second.html")
                    )
                } else {
                    first.into()
                })
            }
            "https://givee.club/ru" => {
                let base = include_str!("../ui-fixtures/givee-baseline.html");
                Ok(
                    if matches!(
                        scenario,
                        Scenario::NewGiveaway | Scenario::CrossSourceGiveaway | Scenario::SlowGivee
                    ) {
                        format!("{base}{}", include_str!("../ui-fixtures/givee-new.html"))
                    } else {
                        base.into()
                    },
                )
            }
            "https://freeanywhere.net/"
                if matches!(scenario, Scenario::OrderFirst | Scenario::OrderSecond) =>
            {
                let first = include_str!("../ui-fixtures/gff-order-first.html");
                Ok(if matches!(scenario, Scenario::OrderSecond) {
                    format!(
                        "{}{first}",
                        include_str!("../ui-fixtures/gff-order-second.html")
                    )
                } else {
                    first.into()
                })
            }
            "https://freeanywhere.net/" if matches!(scenario, Scenario::GamesForFarmFailure) => {
                Err(PageFetchError::Connection(
                    "https://freeanywhere.net/: error sending request: connection reset by peer (os error 10054)".into(),
                ))
            }
            "https://freeanywhere.net/" if matches!(scenario, Scenario::GamesForFarmInvalidFeed) => {
                Ok("<html><body>UI_TEST_UNRECOGNIZED_FEED</body></html>".into())
            }
            "https://freeanywhere.net/" if matches!(scenario, Scenario::GamesForFarmEmpty) => {
                Ok(include_str!(
                    "../crates/giveaway-scout-core/tests/fixtures/html/games-for-farm-empty-en.html"
                )
                .into())
            }
            "https://freeanywhere.net/" => {
                Ok(include_str!("../ui-fixtures/gff-baseline.html").into())
            }
            "http://zeros.group/free/" if matches!(scenario, Scenario::ZerosConnectionLimit) => {
                Err(PageFetchError::ZerosConnectionLimit)
            }
            "http://zeros.group/free/" => Ok(if matches!(scenario, Scenario::ZerosGiveaway) {
                include_str!("../ui-fixtures/zeros-new.json").into()
            } else {
                include_str!("../ui-fixtures/zeros-baseline.json").into()
            }),
            "https://www.freesteamkeys.com/api/giveaways?type=game&sort=date"
                if matches!(scenario, Scenario::FreeSteamKeysFailure) =>
            {
                Err(PageFetchError::Http {
                    status: 503,
                    message: "UI_TEST_FREESTEAMKEYS_UNAVAILABLE".into(),
                })
            }
            "https://www.freesteamkeys.com/api/giveaways?type=game&sort=date" => {
                Ok(if matches!(scenario, Scenario::CrossSourceGiveaway) {
                    include_str!("../ui-fixtures/free-steam-keys-givee.json").into()
                } else if matches!(
                    scenario,
                    Scenario::FreeSteamKeysGiveaway | Scenario::SlowGivee
                ) {
                    include_str!("../ui-fixtures/free-steam-keys-new.json").into()
                } else {
                    "[]".into()
                })
            }
            _ => Err(PageFetchError::Message(
                "network is forbidden in UI tests".into(),
            )),
        }
    }
}

/// Every test redirect is an explicit local fixture. Unlisted addresses never
/// leave the isolated test process or contact the live source.
pub(crate) async fn resolve_redirect(url: &str) -> Result<String, String> {
    match url {
        "https://www.freesteamkeys.com/open/e2e-givee-900003" => {
            Ok("https://givee.club/en/event/900003".into())
        }
        _ => Err("UI_TEST_REDIRECT_NOT_RESOLVED".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn log_folder_selection_stays_inside_the_owned_test_profile() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let custom = root.join("custom logs");
        fs::create_dir(&custom).unwrap();
        assert_eq!(
            owned_log_directory(&root, custom.to_str()),
            custom.canonicalize().unwrap()
        );
        let outside = tempfile::tempdir().unwrap();
        for requested in [None, Some("relative"), outside.path().to_str()] {
            assert_eq!(owned_log_directory(&root, requested), root.join("logs"));
        }
        let traversal = custom.join("..").join("..");
        assert_eq!(
            owned_log_directory(&root, traversal.to_str()),
            root.join("logs")
        );
    }

    #[test]
    fn only_owned_direct_temp_children_are_accepted() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp
            .path()
            .join(format!("giveaway-scout-e2e-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        assert!(validate_root(&root, temp.path()).is_err());
        fs::write(root.join(".giveaway-scout-ui-test"), MARKER).unwrap();
        assert!(validate_root(&root, temp.path()).is_ok());
        assert!(validate_root(temp.path(), temp.path()).is_err());
        let other = tempfile::tempdir().unwrap();
        assert!(validate_root(&root, other.path()).is_err());
    }
    #[test]
    fn fixture_pages_use_the_production_parsers() {
        let mut html = include_str!("../ui-fixtures/givee-baseline.html").to_owned();
        assert_eq!(
            giveaway_scout_core::parse_givee_club(&html).unwrap().len(),
            1
        );
        html.push_str(include_str!("../ui-fixtures/givee-new.html"));
        assert_eq!(
            giveaway_scout_core::parse_givee_club(&html).unwrap().len(),
            2
        );
        assert_eq!(
            giveaway_scout_core::parse_games_for_farm(include_str!(
                "../ui-fixtures/gff-baseline.html"
            ))
            .unwrap()
            .len(),
            1
        );
        let large = giveaway_scout_core::parse_givee_club(&large_history_page(false)).unwrap();
        assert_eq!(large.len(), 241);
        assert!(large[1].title.len() > 300);
        let added = giveaway_scout_core::parse_givee_club(&large_history_page(true)).unwrap();
        assert_eq!(added.len(), 242);
        assert_eq!(added[1].title, "New arrival while reading older history");
    }

    #[test]
    fn production_identifier_is_rejected_before_any_profile_access() {
        assert!(initialize("com.giveawayscout.desktop").is_err());
        assert!(initialize("com.giveawayscout.desktop.dev").is_err());
    }

    #[test]
    fn wrong_marker_and_nested_roots_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp
            .path()
            .join(format!("giveaway-scout-e2e-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        fs::write(
            root.join(".giveaway-scout-ui-test"),
            "not owned by the UI runner",
        )
        .unwrap();
        assert!(validate_root(&root, temp.path()).is_err());
        fs::write(root.join(".giveaway-scout-ui-test"), MARKER).unwrap();
        let nested = root.join(format!("giveaway-scout-e2e-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join(".giveaway-scout-ui-test"), MARKER).unwrap();
        assert!(validate_root(&nested, temp.path()).is_err());
    }
}
