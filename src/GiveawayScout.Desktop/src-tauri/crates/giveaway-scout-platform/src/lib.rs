//! Native integration shared by the desktop and console applications.
use std::{
    fs::{File, OpenOptions, TryLockError},
    io,
    path::{Path, PathBuf},
};

use giveaway_scout_core::{AppSettings, AutostartTarget, Database, StorageError};
use thiserror::Error;

#[cfg(windows)]
mod windows;

pub const PROFILE_IDENTIFIER: &str = "com.giveawayscout.desktop";
pub const GUI_COMPONENT: u32 = 1;
pub const CLI_COMPONENT: u32 = 2;

#[derive(Debug, Error)]
pub enum PlatformError {
    #[error("profile is already in use by Giveaway Scout: {0}")]
    ProfileBusy(PathBuf),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("{0}")]
    InvalidSettings(String),
    #[error("{0}")]
    System(String),
}

impl PlatformError {
    pub const fn is_profile_busy(&self) -> bool {
        matches!(self, Self::ProfileBusy(_))
    }
}

/// Hold this guard until all users of the database have stopped and dropped it.
/// The lock file remains in place: removing it could allow two locks on Unix.
#[derive(Debug)]
pub struct ProfileGuard {
    _file: File,
}

impl ProfileGuard {
    pub fn acquire(directory: &Path) -> Result<Self, PlatformError> {
        std::fs::create_dir_all(directory)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join("giveaway-scout.lock"))?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(TryLockError::WouldBlock) => {
                Err(PlatformError::ProfileBusy(directory.to_path_buf()))
            }
            Err(TryLockError::Error(error)) => Err(error.into()),
        }
    }
}

pub fn default_data_directory(development: bool) -> Result<PathBuf, PlatformError> {
    let identifier = if development {
        format!("{PROFILE_IDENTIFIER}.dev")
    } else {
        PROFILE_IDENTIFIER.to_owned()
    };
    #[cfg(windows)]
    let root = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let root = std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Library/Application Support"));
    #[cfg(not(any(windows, target_os = "macos")))]
    let root = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    root.filter(|root| root.is_absolute())
        .map(|root| root.join(identifier))
        .ok_or_else(|| PlatformError::System("cannot locate the application data directory".into()))
}

pub fn target_executable(directory: &Path, target: AutostartTarget) -> PathBuf {
    let name = match target {
        AutostartTarget::Gui => "giveaway-scout",
        AutostartTarget::Cli => "giveaway-scout-cli",
    };
    directory.join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

pub fn autostart_command(
    directory: &Path,
    target: AutostartTarget,
) -> Result<String, PlatformError> {
    let executable = target_executable(directory, target);
    if !executable.is_file() {
        return Err(PlatformError::InvalidSettings(format!(
            "autostart component is not installed: {}",
            executable.display()
        )));
    }
    let argument = match target {
        AutostartTarget::Gui => "--autostart",
        AutostartTarget::Cli => "watch",
    };
    Ok(format!("\"{}\" {argument}", executable.display()))
}

/// Apply native changes before committing settings; restore native state if the
/// database fails. The caller must hold the profile guard and database mutex.
pub fn update_settings(
    database: &Database,
    settings: &AppSettings,
    development: bool,
) -> Result<(), PlatformError> {
    update_settings_with(database, settings, |settings| {
        sync_autostart(settings, development)
    })
}

fn update_settings_with(
    database: &Database,
    settings: &AppSettings,
    mut sync: impl FnMut(&AppSettings) -> Result<(), PlatformError>,
) -> Result<(), PlatformError> {
    Database::validate_settings(settings)?;
    let previous = database.settings()?;
    let changed = previous.start_at_login != settings.start_at_login
        || previous.autostart_target != settings.autostart_target;
    if changed {
        sync(settings)?;
    }
    if let Err(error) = database.update_settings(settings) {
        if changed && let Err(rollback) = sync(&previous) {
            return Err(PlatformError::System(format!(
                "{error}; could not restore autostart: {rollback}"
            )));
        }
        return Err(error.into());
    }
    Ok(())
}

pub fn sync_autostart(settings: &AppSettings, development: bool) -> Result<(), PlatformError> {
    if development {
        return Ok(());
    }
    #[cfg(windows)]
    {
        let executable = std::env::current_exe()?;
        let directory = executable
            .parent()
            .ok_or_else(|| PlatformError::System("executable has no parent directory".into()))?;
        windows::set_autostart(settings, directory)
    }
    #[cfg(not(windows))]
    {
        let _ = settings;
        Err(PlatformError::System(
            "console autostart is supported on Windows only".into(),
        ))
    }
}

pub fn is_current_installation(executable: &Path) -> bool {
    #[cfg(windows)]
    {
        windows::is_current_installation(executable)
    }
    #[cfg(not(windows))]
    {
        let _ = executable;
        false
    }
}

pub fn same_executable(current: &Path, registered: &Path) -> bool {
    match (current.canonicalize(), registered.canonicalize()) {
        (Ok(current), Ok(registered)) => current == registered,
        _ => false,
    }
}

/// Setup preflight: detect any process sharing the production profile before
/// replacing components, including a portable executable outside the install.
pub fn check_profile_available() -> Result<(), PlatformError> {
    let directory = default_data_directory(false)?;
    let _guard = ProfileGuard::acquire(&directory)?;
    Ok(())
}

/// Called from the temporary setup helper. Never infer installation identity
/// from that executable's name or directory.
pub fn maintain_installation(
    install_directory: &Path,
    components: u32,
    removing: bool,
) -> Result<(), PlatformError> {
    if !install_directory.is_absolute()
        || install_directory.to_string_lossy().contains(';')
        || !(1..=3).contains(&components)
    {
        return Err(PlatformError::InvalidSettings(
            "invalid installation directory or components".into(),
        ));
    }
    #[cfg(windows)]
    {
        let directory = default_data_directory(false)?;
        let _guard = ProfileGuard::acquire(&directory)?;
        let (database, _) = Database::open_with_recovery(directory.join("giveaway-scout.db"))?;
        let previous = database.settings()?;
        let settings = settings_for_components(&previous, components, removing);
        windows::maintain(
            &database,
            &previous,
            &settings,
            install_directory,
            components,
            removing,
        )?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = removing;
        Err(PlatformError::System(
            "installation maintenance is supported on Windows only".into(),
        ))
    }
}

#[cfg(any(windows, test))]
fn settings_for_components(previous: &AppSettings, components: u32, removing: bool) -> AppSettings {
    let mut settings = previous.clone();
    let selected_component = match settings.autostart_target {
        AutostartTarget::Gui => GUI_COMPONENT,
        AutostartTarget::Cli => CLI_COMPONENT,
    };
    if removing || components & selected_component == 0 {
        settings.start_at_login = false;
    }
    settings
}

/// Change an existing camelCase setting, accepting JSON values and unquoted
/// strings. Only per-source intervals accept dotted keys.
pub fn settings_with_value(
    previous: &AppSettings,
    key: &str,
    input: &str,
) -> Result<AppSettings, PlatformError> {
    let mut value = serde_json::to_value(previous)
        .map_err(|error| PlatformError::InvalidSettings(error.to_string()))?;
    let mut keys = key.split('.');
    let first = keys.next().unwrap_or_default();
    let second = keys.next();
    if keys.next().is_some() || (second.is_some() && first != "sourceCheckIntervals") {
        return Err(PlatformError::InvalidSettings(format!(
            "unknown setting: {key}"
        )));
    }
    let selected = if let Some(second) = second {
        value.get_mut(first).and_then(|value| value.get_mut(second))
    } else {
        value.get_mut(first)
    }
    .ok_or_else(|| PlatformError::InvalidSettings(format!("unknown setting: {key}")))?;
    *selected =
        serde_json::from_str(input).unwrap_or_else(|_| serde_json::Value::String(input.to_owned()));
    let mut settings: AppSettings = serde_json::from_value(value)
        .map_err(|error| PlatformError::InvalidSettings(error.to_string()))?;
    if key == "checkIntervalMinutes" {
        let interval = settings.check_interval_minutes;
        settings.source_check_intervals = giveaway_scout_core::SourceCheckIntervals {
            free_steam_keys: interval,
            givee_club: interval,
            games_for_farm: interval,
            zeros: interval,
        };
    }
    Database::validate_settings(&settings)?;
    Ok(settings)
}

fn path_entry_matches(entry: &str, directory: &str) -> bool {
    entry
        .trim()
        .trim_matches('"')
        .trim_end_matches(['\\', '/'])
        .eq_ignore_ascii_case(
            directory
                .trim()
                .trim_matches('"')
                .trim_end_matches(['\\', '/']),
        )
}

/// Preserve existing text, including expandable environment variables.
pub fn add_path_entry(path: &str, directory: &str) -> (String, bool) {
    if path
        .split(';')
        .any(|entry| path_entry_matches(entry, directory))
    {
        return (path.to_owned(), false);
    }
    let separator = if path.is_empty() || path.ends_with(';') {
        ""
    } else {
        ";"
    };
    (format!("{path}{separator}{directory}"), true)
}

pub fn remove_owned_path_entry(path: &str, directory: &str, owned: bool) -> String {
    if !owned {
        return path.to_owned();
    }
    let mut removed = false;
    path.split(';')
        .filter(|entry| {
            if !removed && path_entry_matches(entry, directory) {
                removed = true;
                false
            } else {
                true
            }
        })
        .collect::<Vec<_>>()
        .join(";")
}

#[cfg(test)]
mod tests;
