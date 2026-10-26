//! Read-only identity checks shared by notifications and the updater.
// The isolated UI harness replaces the notification adapter, but retains these
// read-only identity helpers and their unit tests.
#![cfg_attr(feature = "ui-tests", allow(dead_code))]
use std::path::Path;

pub(crate) fn is_current_installation(app: &tauri::AppHandle) -> bool {
    let Ok(executable) = std::env::current_exe() else {
        return false;
    };
    let key_path = format!(
        r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{}",
        app.config()
            .product_name
            .as_deref()
            .unwrap_or("Giveaway Scout")
    );
    // Build-output EXEs contain a bundle marker too. Require the real NSIS
    // installation entry and this exact executable, not any installed copy.
    [
        windows_registry::CURRENT_USER,
        windows_registry::LOCAL_MACHINE,
    ]
    .into_iter()
    .filter_map(|root| root.open(&key_path).ok())
    .filter_map(|key| key.get_string("InstallLocation").ok())
    .any(|directory| matches_installation(&executable, &directory))
}

pub(crate) fn notification_protocol_available(app: &tauri::AppHandle) -> bool {
    if !is_current_installation(app) {
        return false;
    }
    let Ok(executable) = std::env::current_exe() else {
        return false;
    };
    // HKCR uses the effective user/machine association. A stale or overridden
    // association must not route a toast to a different executable.
    windows_registry::CLASSES_ROOT
        .open(r"giveaway-scout\shell\open\command")
        .ok()
        .and_then(|key| key.get_string("").ok())
        .is_some_and(|command| matches_protocol_command(&executable, &command))
}

fn same_executable(current: &Path, registered: &Path) -> bool {
    match (current.canonicalize(), registered.canonicalize()) {
        (Ok(current), Ok(registered)) => current == registered,
        _ => false,
    }
}

fn matches_installation(executable: &Path, directory: &str) -> bool {
    same_executable(
        executable,
        &Path::new(directory.trim_matches('"')).join("giveaway-scout.exe"),
    )
}

fn matches_protocol_command(executable: &Path, command: &str) -> bool {
    // Match only the quoted command emitted by our NSIS installer. Do not
    // execute or attempt permissive shell parsing of a registry command.
    let Some(command) = command.trim().strip_prefix('"') else {
        return false;
    };
    let Some((registered, arguments)) = command.split_once('"') else {
        return false;
    };
    arguments.trim() == "\"%1\"" && same_executable(executable, Path::new(registered))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installation_identity_requires_the_same_existing_executable() {
        let installed = tempfile::tempdir().unwrap();
        let portable = tempfile::tempdir().unwrap();
        let installed_exe = installed.path().join("giveaway-scout.exe");
        let portable_exe = portable.path().join("giveaway-scout.exe");
        std::fs::write(&installed_exe, b"fixture").unwrap();
        std::fs::write(&portable_exe, b"fixture").unwrap();
        let directory = installed.path().to_str().unwrap();
        assert!(matches_installation(&installed_exe, directory));
        assert!(matches_installation(
            &installed_exe,
            &format!("\"{directory}\"")
        ));
        assert!(!matches_installation(&portable_exe, directory));
        assert!(!matches_installation(&installed_exe, ""));
        assert!(!matches_installation(
            &portable.path().join("missing.exe"),
            directory
        ));
    }

    #[test]
    fn protocol_identity_rejects_other_paths_missing_files_and_extra_arguments() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("giveaway-scout.exe");
        let other = directory.path().join("other.exe");
        std::fs::write(&executable, b"fixture").unwrap();
        std::fs::write(&other, b"fixture").unwrap();
        let correct = format!("\"{}\" \"%1\"", executable.display());
        assert!(matches_protocol_command(&executable, &correct));
        assert!(!matches_protocol_command(&other, &correct));
        for command in [
            String::new(),
            format!("{} \"%1\"", executable.display()),
            format!("\"{}\" %1", executable.display()),
            format!("{correct} --unexpected"),
            format!(
                "\"{}\" \"%1\"",
                directory.path().join("missing.exe").display()
            ),
        ] {
            assert!(!matches_protocol_command(&executable, &command));
        }
    }
}
