use super::*;

#[test]
fn profile_lock_excludes_another_owner_and_releases_on_drop() {
    let directory = tempfile::tempdir().unwrap();
    let guard = ProfileGuard::acquire(directory.path()).unwrap();
    assert!(matches!(
        ProfileGuard::acquire(directory.path()),
        Err(PlatformError::ProfileBusy(_))
    ));
    let different = tempfile::tempdir().unwrap();
    assert!(ProfileGuard::acquire(different.path()).is_ok());
    drop(guard);
    assert!(ProfileGuard::acquire(directory.path()).is_ok());
}

#[test]
fn settings_accept_all_existing_fields_and_reject_unknown_or_invalid_changes() {
    let baseline = AppSettings::default();
    for (key, input) in [
        ("autostartTarget", "cli"),
        ("startAtLogin", "true"),
        ("initialSyncMode", "notifyCurrent"),
        ("languageCode", "ru"),
        ("sourceCheckIntervals.giveeClub", "15"),
        ("httpTimeoutSeconds", "45"),
    ] {
        assert!(settings_with_value(&baseline, key, input).is_ok(), "{key}");
    }
    let legacy = settings_with_value(&baseline, "checkIntervalMinutes", "30").unwrap();
    for source in giveaway_scout_core::GiveawaySource::ALL {
        assert_eq!(legacy.check_interval_for(source), 30);
    }
    for (key, input) in [
        ("unknown", "true"),
        ("sourceCheckIntervals.other", "15"),
        ("sourceCheckIntervals.giveeClub.other", "15"),
        ("autostartTarget", "other"),
        ("startAtLogin", "yes"),
        ("httpTimeoutSeconds", "0"),
    ] {
        assert!(settings_with_value(&baseline, key, input).is_err(), "{key}");
    }
}

#[test]
fn failed_os_change_preserves_database_and_validation_runs_first() {
    let database = Database::open_in_memory().unwrap();
    let previous = database.settings().unwrap();
    let mut settings = previous.clone();
    settings.start_at_login = true;
    let result = update_settings_with(&database, &settings, |_| {
        Err(PlatformError::System("mock registry failure".into()))
    });
    assert!(result.is_err());
    assert_eq!(database.settings().unwrap(), previous);
    settings.http_timeout_seconds = 0;
    assert!(
        update_settings_with(&database, &settings, |_| panic!(
            "invalid settings reached the OS"
        ))
        .is_err()
    );
}

#[test]
fn selecting_a_new_target_updates_os_and_keeps_other_settings() {
    let database = Database::open_in_memory().unwrap();
    let mut previous = database.settings().unwrap();
    previous.start_at_login = true;
    previous.keep_running_in_tray = true;
    database.update_settings(&previous).unwrap();
    let mut settings = previous.clone();
    settings.autostart_target = AutostartTarget::Cli;
    let mut calls = Vec::new();
    update_settings_with(&database, &settings, |value| {
        calls.push(value.autostart_target);
        Ok(())
    })
    .unwrap();
    assert_eq!(calls, [AutostartTarget::Cli]);
    assert_eq!(database.settings().unwrap(), settings);
}

#[test]
fn failed_database_commit_restores_the_previous_os_target() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("profile.db");
    let database = Database::open(&path).unwrap();
    let previous = database.settings().unwrap();
    rusqlite::Connection::open(&path).unwrap().execute_batch(
        "CREATE TRIGGER fail_settings BEFORE UPDATE ON settings BEGIN SELECT RAISE(FAIL, 'mock storage failure'); END;"
    ).unwrap();
    let mut settings = previous.clone();
    settings.start_at_login = true;
    settings.autostart_target = AutostartTarget::Cli;
    let mut calls = Vec::new();
    assert!(
        update_settings_with(&database, &settings, |settings| {
            calls.push((settings.start_at_login, settings.autostart_target));
            Ok(())
        })
        .is_err()
    );
    assert_eq!(
        calls,
        [(true, AutostartTarget::Cli), (false, AutostartTarget::Gui)]
    );
    assert_eq!(database.settings().unwrap(), previous);
}

#[test]
fn removing_selected_component_disables_autostart_and_preserves_target() {
    for (target, required) in [
        (AutostartTarget::Gui, GUI_COMPONENT),
        (AutostartTarget::Cli, CLI_COMPONENT),
    ] {
        let previous = AppSettings {
            start_at_login: true,
            autostart_target: target,
            ..AppSettings::default()
        };
        for components in [GUI_COMPONENT, CLI_COMPONENT, GUI_COMPONENT | CLI_COMPONENT] {
            let settings = settings_for_components(&previous, components, false);
            assert_eq!(settings.autostart_target, target);
            assert_eq!(settings.start_at_login, components & required != 0);
            let removed = settings_for_components(&previous, components, true);
            assert!(!removed.start_at_login);
            assert_eq!(removed.autostart_target, target);
        }
    }
}

#[test]
fn autostart_targets_use_exact_binary_and_argument_and_require_component() {
    let directory = tempfile::tempdir().unwrap();
    for (target, argument) in [
        (AutostartTarget::Gui, "--autostart"),
        (AutostartTarget::Cli, "watch"),
    ] {
        assert!(autostart_command(directory.path(), target).is_err());
        let executable = target_executable(directory.path(), target);
        std::fs::write(&executable, b"fixture").unwrap();
        assert_eq!(
            autostart_command(directory.path(), target).unwrap(),
            format!("\"{}\" {argument}", executable.display())
        );
    }
}

#[test]
fn path_ownership_preserves_existing_entries_and_expandable_long_paths() {
    let original = format!("%USERPROFILE%\\bin;{}", "C:\\existing;".repeat(1_000));
    let (path, owned) = add_path_entry(&original, "C:\\Giveaway Scout");
    assert!(owned);
    assert!(path.len() > 10_000);
    assert!(path.starts_with(&original));
    assert_eq!(
        remove_owned_path_entry(&path, "c:\\giveaway scout\\", true),
        original.trim_end_matches(';')
    );
    let existing = "C:\\bin;\"c:\\Giveaway Scout\\\";C:\\other";
    let (path, owned) = add_path_entry(existing, "C:\\GIVEAWAY SCOUT");
    assert_eq!(path, existing);
    assert!(!owned);
    assert_eq!(
        remove_owned_path_entry(existing, "C:\\Giveaway Scout", false),
        existing
    );
    assert_eq!(
        remove_owned_path_entry("C:\\Scout;C:\\Scout", "C:\\Scout", true),
        "C:\\Scout"
    );
}

#[test]
fn installation_identity_requires_the_exact_existing_file() {
    let installed = tempfile::tempdir().unwrap();
    let portable = tempfile::tempdir().unwrap();
    let current = installed.path().join("giveaway-scout-cli.exe");
    let other = portable.path().join("giveaway-scout-cli.exe");
    std::fs::write(&current, b"fixture").unwrap();
    std::fs::write(&other, b"fixture").unwrap();
    assert!(same_executable(&current, &current));
    assert!(!same_executable(&current, &other));
    assert!(!same_executable(
        &current,
        &installed.path().join("missing.exe")
    ));
}
