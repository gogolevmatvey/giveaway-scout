use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GiveawaySource {
    GiveeClub,
    GamesForFarm,
    Zeros,
}

impl GiveawaySource {
    pub const ALL: [Self; 3] = [Self::GiveeClub, Self::GamesForFarm, Self::Zeros];

    pub const fn tracking_prefix(self) -> &'static str {
        match self {
            Self::GiveeClub => "GiveeClub",
            Self::GamesForFarm => "GamesForFarm",
            Self::Zeros => "Zeros",
        }
    }

    pub const fn storage_name(self) -> &'static str {
        match self {
            Self::GiveeClub => "giveeClub",
            Self::GamesForFarm => "gamesForFarm",
            Self::Zeros => "zeros",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GiveawayKind {
    Giveaway,
    Raffle,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GameStore {
    Steam,
    EpicGames,
    #[default]
    Unknown,
}

impl GameStore {
    pub const fn storage_name(self) -> &'static str {
        match self {
            Self::Steam => "steam",
            Self::EpicGames => "epicGames",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InitialSyncMode {
    Silent,
    NotifyCurrent,
}

impl InitialSyncMode {
    pub const fn storage_name(self) -> &'static str {
        match self {
            Self::Silent => "silent",
            Self::NotifyCurrent => "notifyCurrent",
        }
    }

    pub fn from_storage_name(value: &str) -> Option<Self> {
        match value {
            "silent" => Some(Self::Silent),
            "notifyCurrent" => Some(Self::NotifyCurrent),
            _ => None,
        }
    }
}

impl GiveawayKind {
    pub const fn legacy_name(self) -> &'static str {
        match self {
            Self::Giveaway => "Giveaway",
            Self::Raffle => "Raffle",
        }
    }

    pub const fn storage_name(self) -> &'static str {
        match self {
            Self::Giveaway => "giveaway",
            Self::Raffle => "raffle",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GiveawayItem {
    pub source: GiveawaySource,
    pub source_id: String,
    pub title: String,
    pub url: String,
    pub kind: GiveawayKind,
    #[serde(default)]
    pub store: GameStore,
    pub details: Option<String>,
    pub legacy_tracking_key_prefixes: Vec<String>,
}

impl GiveawayItem {
    pub fn tracking_key(&self) -> String {
        format!("{}:{}", self.source.tracking_prefix(), self.source_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryResult {
    pub is_initial_synchronization: bool,
    pub new_items: Vec<GiveawayItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub check_interval_minutes: u32,
    pub http_timeout_seconds: u32,
    pub http_retry_count: u32,
    pub givee_club_enabled: bool,
    pub games_for_farm_enabled: bool,
    /// Opt-in: this source does not provide an encrypted connection.
    #[serde(default)]
    pub zeros_enabled: bool,
    #[serde(default = "enabled_by_default")]
    pub steam_notifications_enabled: bool,
    #[serde(default = "enabled_by_default")]
    pub epic_games_notifications_enabled: bool,
    pub notification_sound_enabled: bool,
    pub start_at_login: bool,
    /// Applies only to OS autostart; ordinary launches still open the window.
    #[serde(default)]
    pub start_in_tray: bool,
    pub keep_running_in_tray: bool,
    pub language_code: Option<String>,
    pub verbose_logging: bool,
    pub initial_sync_mode: Option<InitialSyncMode>,
}

fn enabled_by_default() -> bool {
    true
}

impl AppSettings {
    pub const fn allows_source(&self, source: GiveawaySource) -> bool {
        match source {
            GiveawaySource::GiveeClub => self.givee_club_enabled,
            GiveawaySource::GamesForFarm => self.games_for_farm_enabled,
            GiveawaySource::Zeros => self.zeros_enabled,
        }
    }

    pub const fn allows_store(&self, store: GameStore) -> bool {
        match store {
            GameStore::Steam => self.steam_notifications_enabled,
            GameStore::EpicGames => self.epic_games_notifications_enabled,
            GameStore::Unknown => true,
        }
    }
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            check_interval_minutes: 15,
            http_timeout_seconds: 30,
            http_retry_count: 2,
            givee_club_enabled: true,
            games_for_farm_enabled: true,
            zeros_enabled: false,
            steam_notifications_enabled: true,
            epic_games_notifications_enabled: true,
            notification_sound_enabled: true,
            start_at_login: false,
            start_in_tray: false,
            keep_running_in_tray: false,
            language_code: None,
            verbose_logging: false,
            initial_sync_mode: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataCounts {
    pub tracked_giveaways: u64,
    pub notification_history: u64,
    pub unread_notifications: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationRecord {
    pub id: String,
    pub tracking_key: Option<String>,
    pub source: String,
    pub kind: String,
    pub title: String,
    pub url: String,
    pub received_at_utc: String,
    pub read_at_utc: Option<String>,
    pub delivery_state: String,
    pub delivery_attempts: u32,
    pub last_delivery_error: Option<String>,
}
