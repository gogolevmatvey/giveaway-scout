use std::{collections::BTreeMap, fs, path::Path};

use serde::Deserialize;
use serde::de::DeserializeOwned;

use super::StorageError;

pub(super) const SETTINGS_FILE: &str = "settings.json";
pub(super) const TRACKING_FILE: &str = "tracking-state.json";
pub(super) const HISTORY_FILE: &str = "notification-history.json";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub(super) struct LegacySettings {
    pub check_interval_minutes: u32,
    pub givee_club_enabled: bool,
    pub games_for_farm_enabled: bool,
    pub notification_sound_enabled: bool,
    pub notification_history: Vec<LegacyNotification>,
    pub start_with_windows: bool,
    pub language_code: Option<String>,
}

impl Default for LegacySettings {
    fn default() -> Self {
        Self {
            check_interval_minutes: 15,
            givee_club_enabled: true,
            games_for_farm_enabled: true,
            notification_sound_enabled: true,
            notification_history: Vec::new(),
            start_with_windows: false,
            language_code: None,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub(super) struct LegacyTrackingState {
    pub is_initialized: bool,
    pub items: BTreeMap<String, LegacyTrackedItem>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub(super) struct LegacyTrackedItem {
    pub title: String,
    pub first_seen_utc: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub(super) struct LegacyNotification {
    pub id: String,
    pub source: i64,
    pub kind: i64,
    pub title: String,
    pub url: String,
    pub received_at: String,
    pub is_read: bool,
}

pub(super) struct LegacyData {
    pub settings: Option<LegacySettings>,
    pub tracking: Option<LegacyTrackingState>,
    pub notifications: Vec<LegacyNotification>,
}

impl LegacyData {
    pub fn load(directory: &Path) -> Result<Self, StorageError> {
        let settings_path = directory.join(SETTINGS_FILE);
        let tracking_path = directory.join(TRACKING_FILE);
        let history_path = directory.join(HISTORY_FILE);

        let settings = read_optional_json::<LegacySettings>(&settings_path)?;
        let tracking = read_optional_json::<LegacyTrackingState>(&tracking_path)?;
        let separate_history = read_optional_json::<Vec<LegacyNotification>>(&history_path)?;

        let mut notifications = settings
            .as_ref()
            .map(|value| value.notification_history.clone())
            .unwrap_or_default();

        if let Some(history) = separate_history {
            for notification in history {
                if let Some(existing) = notifications
                    .iter_mut()
                    .find(|existing| existing.id == notification.id)
                {
                    *existing = notification;
                } else {
                    notifications.push(notification);
                }
            }
        }

        Ok(Self {
            settings,
            tracking,
            notifications,
        })
    }
}

fn read_optional_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, StorageError> {
    if !path.exists() {
        return Ok(None);
    }

    let json = fs::read_to_string(path).map_err(|source| StorageError::ReadLegacyFile {
        path: path.to_path_buf(),
        source,
    })?;

    serde_json::from_str(&json)
        .map(Some)
        .map_err(|source| StorageError::ParseLegacyFile {
            path: path.to_path_buf(),
            source,
        })
}
