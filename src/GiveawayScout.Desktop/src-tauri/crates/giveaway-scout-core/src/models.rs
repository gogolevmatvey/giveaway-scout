use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GiveawaySource {
    GiveeClub,
    GamesForFarm,
    Zeros,
    FreeSteamKeys,
}

impl GiveawaySource {
    pub const ALL: [Self; 4] = [
        Self::FreeSteamKeys,
        Self::GiveeClub,
        Self::GamesForFarm,
        Self::Zeros,
    ];

    pub const fn tracking_prefix(self) -> &'static str {
        match self {
            Self::GiveeClub => "GiveeClub",
            Self::GamesForFarm => "GamesForFarm",
            Self::Zeros => "Zeros",
            Self::FreeSteamKeys => "FreeSteamKeys",
        }
    }

    pub const fn storage_name(self) -> &'static str {
        match self {
            Self::GiveeClub => "giveeClub",
            Self::GamesForFarm => "gamesForFarm",
            Self::Zeros => "zeros",
            Self::FreeSteamKeys => "freeSteamKeys",
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
    Gog,
    #[default]
    Unknown,
}

impl GameStore {
    pub fn from_storage_name(value: &str) -> Self {
        match value {
            "steam" => Self::Steam,
            "epicGames" => Self::EpicGames,
            "gog" => Self::Gog,
            _ => Self::Unknown,
        }
    }

    pub const fn display_name(self) -> Option<&'static str> {
        match self {
            Self::Steam => Some("Steam"),
            Self::EpicGames => Some("Epic Games"),
            Self::Gog => Some("GOG"),
            Self::Unknown => None,
        }
    }

    pub const fn storage_name(self) -> &'static str {
        match self {
            Self::Steam => "steam",
            Self::EpicGames => "epicGames",
            Self::Gog => "gog",
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
    /// Retained for older settings clients; scheduling uses per-source intervals.
    pub check_interval_minutes: u32,
    #[serde(default)]
    pub source_check_intervals: SourceCheckIntervals,
    pub http_timeout_seconds: u32,
    pub http_retry_count: u32,
    pub givee_club_enabled: bool,
    pub games_for_farm_enabled: bool,
    /// Opt-in: this source does not provide an encrypted connection.
    #[serde(default)]
    pub zeros_enabled: bool,
    #[serde(default = "enabled_by_default")]
    pub free_steam_keys_enabled: bool,
    #[serde(default = "enabled_by_default")]
    pub steam_notifications_enabled: bool,
    #[serde(default = "enabled_by_default")]
    pub epic_games_notifications_enabled: bool,
    #[serde(default = "enabled_by_default")]
    pub gog_notifications_enabled: bool,
    pub notification_sound_enabled: bool,
    pub start_at_login: bool,
    #[serde(default)]
    pub autostart_target: AutostartTarget,
    /// Applies only to OS autostart; ordinary launches still open the window.
    #[serde(default)]
    pub start_in_tray: bool,
    pub keep_running_in_tray: bool,
    pub language_code: Option<String>,
    pub verbose_logging: bool,
    #[serde(default = "default_log_max_file_size_mb")]
    pub log_max_file_size_mb: u32,
    /// Total number of log files, including the current file.
    #[serde(default = "default_log_file_count")]
    pub log_file_count: u32,
    /// An absolute native path; unset means the application's standard log directory.
    #[serde(default)]
    pub log_directory: Option<String>,
    pub initial_sync_mode: Option<InitialSyncMode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SourceCheckIntervals {
    pub free_steam_keys: u32,
    pub givee_club: u32,
    pub games_for_farm: u32,
    pub zeros: u32,
}

impl SourceCheckIntervals {
    pub const fn for_source(&self, source: GiveawaySource) -> u32 {
        match source {
            GiveawaySource::FreeSteamKeys => self.free_steam_keys,
            GiveawaySource::GiveeClub => self.givee_club,
            GiveawaySource::GamesForFarm => self.games_for_farm,
            GiveawaySource::Zeros => self.zeros,
        }
    }
}

impl Default for SourceCheckIntervals {
    fn default() -> Self {
        Self {
            free_steam_keys: 60,
            givee_club: 60,
            games_for_farm: 60,
            zeros: 60,
        }
    }
}

fn enabled_by_default() -> bool {
    true
}

fn default_log_max_file_size_mb() -> u32 {
    2
}

fn default_log_file_count() -> u32 {
    1
}

impl AppSettings {
    pub const fn check_interval_for(&self, source: GiveawaySource) -> u32 {
        self.source_check_intervals.for_source(source)
    }

    pub const fn allows_source(&self, source: GiveawaySource) -> bool {
        match source {
            GiveawaySource::GiveeClub => self.givee_club_enabled,
            GiveawaySource::GamesForFarm => self.games_for_farm_enabled,
            GiveawaySource::Zeros => self.zeros_enabled,
            GiveawaySource::FreeSteamKeys => self.free_steam_keys_enabled,
        }
    }

    pub const fn allows_store(&self, store: GameStore) -> bool {
        match store {
            GameStore::Steam => self.steam_notifications_enabled,
            GameStore::EpicGames => self.epic_games_notifications_enabled,
            GameStore::Gog => self.gog_notifications_enabled,
            GameStore::Unknown => true,
        }
    }
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            check_interval_minutes: 60,
            source_check_intervals: SourceCheckIntervals::default(),
            http_timeout_seconds: 30,
            http_retry_count: 2,
            givee_club_enabled: true,
            games_for_farm_enabled: true,
            zeros_enabled: false,
            free_steam_keys_enabled: true,
            steam_notifications_enabled: true,
            epic_games_notifications_enabled: true,
            gog_notifications_enabled: true,
            notification_sound_enabled: true,
            start_at_login: false,
            autostart_target: AutostartTarget::Gui,
            start_in_tray: false,
            keep_running_in_tray: false,
            language_code: None,
            verbose_logging: false,
            log_max_file_size_mb: default_log_max_file_size_mb(),
            log_file_count: default_log_file_count(),
            log_directory: None,
            initial_sync_mode: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutostartTarget {
    #[default]
    Gui,
    Cli,
}

impl AutostartTarget {
    pub const fn storage_name(self) -> &'static str {
        match self {
            Self::Gui => "gui",
            Self::Cli => "cli",
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
    #[serde(default)]
    pub campaign_id: Option<String>,
    #[serde(default)]
    pub origin_source: Option<GiveawaySource>,
    #[serde(default)]
    pub origin_name: Option<String>,
    #[serde(default)]
    pub detected_sources: Vec<GiveawaySource>,
    #[serde(default)]
    pub store: GameStore,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationHistoryPage {
    pub items: Vec<NotificationRecord>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SourcePhase {
    Disabled,
    Waiting,
    Checking,
    Ok,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceStatus {
    pub source: GiveawaySource,
    pub phase: SourcePhase,
    pub last_started_at_utc: Option<String>,
    pub last_completed_at_utc: Option<String>,
    pub last_successful_check_utc: Option<String>,
    pub next_check_at_utc: Option<String>,
    pub last_error: Option<crate::monitoring::SourceFailure>,
    pub last_new_items: u64,
    pub unresolved_links: u64,
}

impl SourceStatus {
    pub fn new(source: GiveawaySource) -> Self {
        Self {
            source,
            phase: SourcePhase::Waiting,
            last_started_at_utc: None,
            last_completed_at_utc: None,
            last_successful_check_utc: None,
            next_check_at_utc: None,
            last_error: None,
            last_new_items: 0,
            unresolved_links: 0,
        }
    }
}
