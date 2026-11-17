use giveaway_scout_core::AppSettings;

pub fn changed(settings: &AppSettings, key: &str, value: &str) -> Result<AppSettings, String> {
    giveaway_scout_platform::settings_with_value(settings, key, value)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_settings_and_updates_legacy_interval_for_all_sources() {
        let current = AppSettings::default();
        let next = changed(&current, "sourceCheckIntervals.giveeClub", "30").unwrap();
        assert_eq!(next.source_check_intervals.givee_club, 30);
        assert_eq!(next.zeros_enabled, current.zeros_enabled);
        let next = changed(&next, "checkIntervalMinutes", "45").unwrap();
        assert_eq!(next.source_check_intervals.givee_club, 45);
        assert_eq!(next.source_check_intervals.free_steam_keys, 45);
        assert_eq!(next.source_check_intervals.games_for_farm, 45);
        assert_eq!(next.source_check_intervals.zeros, 45);
        assert!(changed(&next, "missing", "true").is_err());
        assert!(changed(&next, "startAtLogin", "sometimes").is_err());
        assert_eq!(
            changed(&next, "languageCode", "ru")
                .unwrap()
                .language_code
                .as_deref(),
            Some("ru")
        );
        assert!(
            changed(&next, "languageCode", "null")
                .unwrap()
                .language_code
                .is_none()
        );
    }
}
