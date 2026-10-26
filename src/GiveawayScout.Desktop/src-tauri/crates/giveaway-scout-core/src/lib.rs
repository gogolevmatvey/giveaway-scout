pub mod delivery;
pub mod models;
pub mod monitoring;
pub mod sources;
pub mod storage;

use serde::Serialize;

pub use delivery::{DeliveryError, DeliveryReport, dispatch_pending_notifications};

pub use models::{
    AppSettings, DataCounts, DiscoveryResult, GameStore, GiveawayItem, GiveawayKind,
    GiveawaySource, InitialSyncMode, NotificationRecord,
};
pub use monitoring::{
    HttpPageFetcher, MonitoringOutcome, PageFetchError, PageFetcher, SourceCheckResult,
    SourceFailure, SourceFailureReason, apply_source_check, check_sources,
};
pub use sources::{
    GAMES_FOR_FARM_FEED_URL, GIVEE_CLUB_FEED_URL, SourceParseError, ZEROS_FEED_URL,
    parse_games_for_farm, parse_givee_club, parse_zeros,
};
pub use storage::{Database, DatabaseRecoveryReport, LegacyImportReport, StorageError};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    pub app_name: &'static str,
    pub version: &'static str,
    pub platform: &'static str,
    pub architecture: &'static str,
    pub migration_phase: &'static str,
    pub is_development: bool,
}

impl AppState {
    pub const fn new(
        version: &'static str,
        platform: &'static str,
        architecture: &'static str,
        is_development: bool,
    ) -> Self {
        Self {
            app_name: "Giveaway Scout",
            version,
            platform,
            architecture,
            migration_phase: "monitoring",
            is_development,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AppState;

    #[test]
    fn app_state_serializes_with_the_frontend_contract() {
        let state = AppState::new("0.1.0", "linux", "x86_64", true);
        let json = serde_json::to_value(state).expect("app state should serialize");

        assert_eq!(json["appName"], "Giveaway Scout");
        assert_eq!(json["version"], "0.1.0");
        assert_eq!(json["platform"], "linux");
        assert_eq!(json["architecture"], "x86_64");
        assert_eq!(json["migrationPhase"], "monitoring");
        assert_eq!(json["isDevelopment"], true);
    }
}
