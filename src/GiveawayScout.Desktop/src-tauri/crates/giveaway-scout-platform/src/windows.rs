use std::path::Path;

use giveaway_scout_core::{AppSettings, AutostartTarget, Database};
use windows_registry::{CURRENT_USER, Key, Transaction, Type, Value};

use crate::{
    CLI_COMPONENT, GUI_COMPONENT, PlatformError, add_path_entry, autostart_command,
    remove_owned_path_entry, same_executable, target_executable,
};

const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Giveaway Scout";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "Giveaway Scout";
const PATH_OWNED_VALUE: &str = "CliPathEntryOwned";
const PATH_ENTRY_VALUE: &str = "CliPathEntry";

fn registry_error(error: impl std::fmt::Display) -> PlatformError {
    PlatformError::System(format!("Windows registration failed: {error}"))
}

fn optional_value(key: &Key, name: &str) -> Result<Option<Value>, PlatformError> {
    match key.get_value(name) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.code().0 as u32 == 0x8007_0002 => Ok(None),
        Err(error) => Err(registry_error(error)),
    }
}

fn remove_value(key: &Key, name: &str) -> Result<(), PlatformError> {
    if optional_value(key, name)?.is_some() {
        key.remove_value(name).map_err(registry_error)?;
    }
    Ok(())
}

pub(super) fn is_current_installation(executable: &Path) -> bool {
    [CURRENT_USER, windows_registry::LOCAL_MACHINE]
        .into_iter()
        .filter_map(|root| root.open(UNINSTALL_KEY).ok())
        .any(|key| {
            let Ok(directory) = key.get_string("InstallLocation") else {
                return false;
            };
            let components = key.get_u32("Components").unwrap_or(GUI_COMPONENT);
            [
                (GUI_COMPONENT, AutostartTarget::Gui),
                (CLI_COMPONENT, AutostartTarget::Cli),
            ]
            .into_iter()
            .any(|(component, target)| {
                components & component != 0
                    && same_executable(
                        executable,
                        &target_executable(Path::new(directory.trim_matches('"')), target),
                    )
            })
        })
}

fn set_autostart_value(
    key: &Key,
    settings: &AppSettings,
    directory: &Path,
) -> Result<(), PlatformError> {
    if settings.start_at_login {
        let command = autostart_command(directory, settings.autostart_target)?;
        key.set_string(RUN_VALUE, command).map_err(registry_error)
    } else {
        remove_value(key, RUN_VALUE)
    }
}

pub(super) fn set_autostart(settings: &AppSettings, directory: &Path) -> Result<(), PlatformError> {
    // Validate the target before opening any writable registration key.
    if settings.start_at_login {
        autostart_command(directory, settings.autostart_target)?;
    }
    let key = CURRENT_USER.create(RUN_KEY).map_err(registry_error)?;
    set_autostart_value(&key, settings, directory)
}

pub(super) fn maintain(
    database: &Database,
    previous: &AppSettings,
    settings: &AppSettings,
    directory: &Path,
    components: u32,
    removing: bool,
) -> Result<(), PlatformError> {
    let transaction = Transaction::new().map_err(registry_error)?;
    let open = |path: &str| {
        CURRENT_USER
            .options()
            .read()
            .write()
            .create()
            .transaction(&transaction)
            .open(path)
            .map_err(registry_error)
    };
    let run = open(RUN_KEY)?;
    let environment = open("Environment")?;
    let installation = open(UNINSTALL_KEY)?;
    set_autostart_value(&run, settings, directory)?;

    let old_path_value = optional_value(&environment, "Path")?;
    let path_type = old_path_value
        .as_ref()
        .map_or(Type::ExpandString, Value::ty);
    if !matches!(path_type, Type::String | Type::ExpandString) {
        return Err(PlatformError::System(
            "user PATH has an unsupported registry type".into(),
        ));
    }
    let path = old_path_value
        .map(String::try_from)
        .transpose()
        .map_err(registry_error)?
        .unwrap_or_default();
    let old_owned = optional_value(&installation, PATH_OWNED_VALUE)?
        .map(u32::try_from)
        .transpose()
        .map_err(registry_error)?
        .unwrap_or(0)
        != 0;
    let old_entry = optional_value(&installation, PATH_ENTRY_VALUE)?
        .map(String::try_from)
        .transpose()
        .map_err(registry_error)?
        .unwrap_or_default();
    let directory_text = directory
        .to_str()
        .ok_or_else(|| PlatformError::System("installation path is not valid UTF-8".into()))?;
    let mut updated_path = path.clone();
    let mut owned = old_owned;
    if old_owned
        && (!old_entry.eq_ignore_ascii_case(directory_text)
            || removing
            || components & CLI_COMPONENT == 0)
    {
        updated_path = remove_owned_path_entry(&updated_path, &old_entry, true);
        owned = false;
    }
    if !removing && components & CLI_COMPONENT != 0 {
        let (new_path, added) = add_path_entry(&updated_path, directory_text);
        updated_path = new_path;
        owned |= added;
    }
    if updated_path != path {
        match path_type {
            Type::ExpandString => environment.set_expand_string("Path", &updated_path),
            _ => environment.set_string("Path", &updated_path),
        }
        .map_err(registry_error)?;
    }
    installation
        .set_u32(PATH_OWNED_VALUE, u32::from(owned))
        .map_err(registry_error)?;
    installation
        .set_string(PATH_ENTRY_VALUE, if owned { directory_text } else { "" })
        .map_err(registry_error)?;

    // Registry writes remain staged until the settings operation succeeds.
    // A failed registry commit restores the original settings before returning.
    database.update_settings(settings)?;
    if let Err(error) = transaction.commit() {
        database.update_settings(previous).map_err(|rollback| {
            PlatformError::System(format!(
                "Windows registration failed: {error}; could not restore settings: {rollback}"
            ))
        })?;
        return Err(registry_error(error));
    }
    if updated_path != path {
        notify_environment_changed();
    }
    Ok(())
}

fn notify_environment_changed() {
    use windows::Win32::{
        Foundation::{LPARAM, WPARAM},
        UI::WindowsAndMessaging::{
            HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
        },
    };
    let name: Vec<u16> = "Environment\0".encode_utf16().collect();
    // Windows copies this synchronous message before the buffer is released.
    unsafe {
        let _ = SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(name.as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            1_000,
            None,
        );
    }
}
