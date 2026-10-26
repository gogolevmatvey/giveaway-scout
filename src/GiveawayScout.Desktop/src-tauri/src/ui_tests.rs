//! Compiled only with ui-tests; all OS effects stay inside a runner-created temp root.
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use giveaway_scout_core::{AppSettings, InitialSyncMode, PageFetchError, PageFetcher};
use serde::{Deserialize, Serialize};

pub(crate) const IDENTIFIER: &str = "com.giveawayscout.desktop.e2e.dev";
const MARKER: &str = "Giveaway Scout isolated UI tests v1\n";
static PROFILE: OnceLock<Profile> = OnceLock::new();

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
    ZerosGiveaway,
    ZerosConnectionLimit,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
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

impl PageFetcher for UiPageFetcher {
    async fn get_text(&self, url: &'static str) -> Result<String, PageFetchError> {
        let scenario = fs::read(self.root.join("scenario.json"))
            .map_err(|e| PageFetchError::Message(e.to_string()))?;
        let scenario: Scenario = serde_json::from_slice(&scenario)
            .map_err(|e| PageFetchError::Message(e.to_string()))?;
        match url {
            "https://givee.club/ru" if matches!(scenario, Scenario::SourceFailure) => Err(
                PageFetchError::Connection("UI_TEST_SOURCE_UNAVAILABLE".into()),
            ),
            "https://givee.club/ru" => {
                let base = include_str!("../ui-fixtures/givee-baseline.html");
                Ok(if matches!(scenario, Scenario::NewGiveaway) {
                    format!("{base}{}", include_str!("../ui-fixtures/givee-new.html"))
                } else {
                    base.into()
                })
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
            _ => Err(PageFetchError::Message(
                "network is forbidden in UI tests".into(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
