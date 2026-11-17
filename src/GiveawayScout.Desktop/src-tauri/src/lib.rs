#[cfg(all(feature = "ui-tests", not(debug_assertions)))]
compile_error!("ui-tests must never be included in Release");
#[cfg(all(feature = "ui-tests", not(target_os = "windows")))]
compile_error!("the native UI test harness currently supports Windows only");
mod attention;
mod coordinator;
#[cfg(target_os = "linux")]
mod linux_activation;
#[cfg(target_os = "linux")]
mod linux_notifications;
mod logging;
#[cfg(target_os = "macos")]
mod macos_notifications;
#[cfg(not(feature = "ui-tests"))]
mod notifications;
#[cfg(feature = "ui-tests")]
#[path = "ui_test_notifications.rs"]
mod notifications;
mod operations;
mod redirect_pipeline;
#[cfg(feature = "ui-tests")]
mod ui_tests;
mod updates;
mod usage_statistics;
#[cfg(target_os = "windows")]
mod windows_installation;

use std::sync::{Arc, Mutex};

use giveaway_scout_core::models::NotificationHistoryPage;
use giveaway_scout_core::{
    AppSettings, AppState, DataCounts, Database, DatabaseRecoveryReport, NotificationRecord,
    SourceFailure, SourcePhase, SourceStatus,
};
use giveaway_scout_platform::ProfileGuard;
#[cfg(test)]
use giveaway_scout_runtime::check_result_diagnostics;
use giveaway_scout_runtime::{
    RunMode, Runtime, RuntimeEvent, SharedDatabase, source_failure_description,
};
use serde::Serialize;
use tauri::UserAttentionType;
use tauri::{
    AppHandle, Emitter, Manager, WindowEvent, Wry,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
#[cfg(not(target_os = "windows"))]
use tauri_plugin_autostart::ManagerExt as AutostartManagerExt;
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

use crate::coordinator::CheckStartResult;
use crate::notifications::NotificationRequest;
use crate::operations::DataOperationGate;

struct StorageState {
    database: SharedDatabase,
    database_recovery: Option<DatabaseRecoveryReport>,
    _profile_guard: Arc<ProfileGuard>,
}

struct MonitorState {
    runtime: Arc<Runtime>,
    data_operation: Arc<DataOperationGate>,
}

struct TrayMenuState {
    open: MenuItem<Wry>,
    check_now: MenuItem<Wry>,
    quit: MenuItem<Wry>,
    #[cfg(target_os = "linux")]
    last_has_unread: Mutex<Option<bool>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MainWindowCloseAction {
    Exit,
    Hide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TrayLabels {
    open: &'static str,
    check_now: &'static str,
    quit: &'static str,
}

const TRAY_OPEN_ID: &str = "tray-open";
const TRAY_ID: &str = "main-tray";
const TRAY_CHECK_NOW_ID: &str = "tray-check-now";
const TRAY_QUIT_ID: &str = "tray-quit";
const AUTOSTART_ARGUMENT: &str = "--autostart";
const DEEP_LINK_SCHEME: &str = "giveaway-scout";

#[cfg(all(target_os = "windows", not(feature = "ui-tests")))]
fn show_profile_open_error(error: &giveaway_scout_platform::PlatformError) {
    use windows::{
        Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW},
        core::{PCWSTR, w},
    };
    let russian =
        sys_locale::get_locale().is_some_and(|locale| locale.to_lowercase().starts_with("ru"));
    let message = if error.is_profile_busy() {
        if russian {
            "Профиль Giveaway Scout уже используется. Завершите работу CLI через Ctrl+C или закройте другую копию приложения и повторите запуск.".to_owned()
        } else {
            "The Giveaway Scout profile is already in use. Stop the CLI with Ctrl+C or quit the other application before starting again.".to_owned()
        }
    } else {
        error.to_string()
    };
    let message: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
    // A native startup error is visible even though the release GUI has no
    // console. Unlike Tauri's blocking dialog, this is safe before its loop.
    unsafe {
        let _ = MessageBoxW(
            None,
            PCWSTR(message.as_ptr()),
            w!("Giveaway Scout"),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DeepLinkAction {
    Show,
    Notification(String),
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StorageSnapshot {
    settings: AppSettings,
    counts: DataCounts,
    data_size_bytes: u64,
    database_recovery: Option<DatabaseRecoveryReport>,
    initial_sync_choice_required: bool,
    monitor: MonitorSnapshot,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct MonitorSnapshot {
    is_checking: bool,
    last_started_at_utc: Option<String>,
    last_completed_at_utc: Option<String>,
    last_successful_check_utc: Option<String>,
    last_error: Option<String>,
    source_failures: Vec<SourceFailure>,
    last_new_items: u64,
    last_failure_count: u64,
    source_states: Vec<SourceStatus>,
}

impl MonitorSnapshot {
    fn from_sources(source_states: Vec<SourceStatus>) -> Self {
        let active: Vec<_> = source_states
            .iter()
            .filter(|state| state.phase != SourcePhase::Disabled)
            .collect();
        let source_failures: Vec<_> = active
            .iter()
            .filter_map(|state| state.last_error.clone())
            .collect();
        let last_error = (!source_failures.is_empty()).then(|| {
            source_failures
                .iter()
                .map(source_failure_description)
                .collect::<Vec<_>>()
                .join("; ")
        });
        Self {
            is_checking: active
                .iter()
                .any(|state| state.phase == SourcePhase::Checking),
            last_started_at_utc: active
                .iter()
                .filter_map(|state| state.last_started_at_utc.clone())
                .max(),
            last_completed_at_utc: active
                .iter()
                .filter_map(|state| state.last_completed_at_utc.clone())
                .max(),
            last_successful_check_utc: active
                .iter()
                .filter_map(|state| state.last_successful_check_utc.clone())
                .max(),
            last_new_items: active.iter().map(|state| state.last_new_items).sum(),
            last_failure_count: source_failures.len() as u64,
            last_error,
            source_failures,
            source_states,
        }
    }
}

#[tauri::command]
fn get_app_state(app: AppHandle) -> AppState {
    AppState::new(
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        development_profile(&app.config().identifier, tauri::is_dev()),
    )
}

#[tauri::command]
fn get_storage_snapshot(
    storage: tauri::State<'_, StorageState>,
    monitor: tauri::State<'_, MonitorState>,
) -> Result<StorageSnapshot, String> {
    let database = storage
        .database
        .lock()
        .map_err(|_| "database lock is unavailable".to_owned())?;
    // Copy storage fields before acquiring runtime state: source completions
    // acquire the coordinator first and then the database.
    let settings = database.settings().map_err(|error| error.to_string())?;
    let counts = database.counts().map_err(|error| error.to_string())?;
    let data_size_bytes = database
        .data_size_bytes()
        .map_err(|error| error.to_string())?;
    let initial_sync_choice_required = database
        .initial_sync_choice_required()
        .map_err(|error| error.to_string())?;
    drop(database);
    let monitor_snapshot = MonitorSnapshot::from_sources(monitor.runtime.source_states()?);
    Ok(StorageSnapshot {
        settings,
        counts,
        data_size_bytes,
        database_recovery: storage.database_recovery.clone(),
        initial_sync_choice_required,
        monitor: monitor_snapshot,
    })
}

#[tauri::command]
fn update_settings(
    app: AppHandle,
    storage: tauri::State<'_, StorageState>,
    monitor: tauri::State<'_, MonitorState>,
    settings: AppSettings,
) -> Result<(), String> {
    monitor.runtime.validate_settings(&settings)?;
    #[cfg(target_os = "windows")]
    {
        let database = storage
            .database
            .lock()
            .map_err(|_| "database lock is unavailable")?;
        giveaway_scout_platform::update_settings(
            &database,
            &settings,
            development_profile(&app.config().identifier, tauri::is_dev()),
        )
        .map_err(|error| error.to_string())?;
    }
    #[cfg(not(target_os = "windows"))]
    let previous = {
        let database = storage
            .database
            .lock()
            .map_err(|_| "database lock is unavailable".to_owned())?;
        let previous = database.settings().map_err(|error| error.to_string())?;
        database
            .update_settings(&settings)
            .map_err(|error| error.to_string())?;
        previous
    };

    #[cfg(not(target_os = "windows"))]
    if previous.start_at_login != settings.start_at_login
        && let Err(error) = sync_autostart(&app, &settings)
    {
        if let Ok(database) = storage.database.lock()
            && let Err(rollback_error) = database.update_settings(&previous)
        {
            log::error!("could not roll back settings after autostart failure: {rollback_error}");
        }
        return Err(error);
    }

    monitor.runtime.reconfigure()?;
    apply_log_level(settings.verbose_logging);
    log::info!(
        "verbose logging {}",
        if settings.verbose_logging {
            "enabled"
        } else {
            "disabled"
        }
    );
    update_tray_labels(&app, effective_language(&settings));
    emit_monitor_changed(&app);
    Ok(())
}

#[tauri::command]
fn get_logging_state(
    storage: tauri::State<'_, StorageState>,
    logging: tauri::State<'_, logging::LoggingState>,
) -> Result<logging::LoggingSnapshot, String> {
    let settings = storage
        .database
        .lock()
        .map_err(|_| "database lock is unavailable".to_owned())?
        .settings()
        .map_err(|error| error.to_string())?;
    Ok(logging.snapshot(&settings))
}

#[tauri::command]
async fn choose_log_directory(
    app: AppHandle,
    window: tauri::WebviewWindow,
) -> Result<Option<String>, String> {
    // Copy state before opening the native dialog: neither database nor logger
    // locks may remain held while the user chooses a folder.
    let (language, directory) = {
        let storage = app.state::<StorageState>();
        let settings = storage
            .database
            .lock()
            .map_err(|_| "database lock is unavailable".to_owned())?
            .settings()
            .map_err(|error| error.to_string())?;
        let snapshot = app.state::<logging::LoggingState>().snapshot(&settings);
        (effective_language(&settings), snapshot.effective_directory)
    };
    let mut dialog = app
        .dialog()
        .file()
        .set_parent(&window)
        .set_title(if language == "en" {
            "Choose log folder"
        } else {
            "Выберите папку журналов"
        });
    if let Some(directory) = directory {
        dialog = dialog.set_directory(directory);
    }
    let (sender, receiver) = tokio::sync::oneshot::channel();
    dialog.pick_folder(move |path| {
        let _ = sender.send(path);
    });
    receiver
        .await
        .map_err(|_| "folder dialog is unavailable".to_owned())?
        .map(|path| {
            path.into_path()
                .map(|path| path.to_string_lossy().into_owned())
                .map_err(|error| error.to_string())
        })
        .transpose()
}

#[tauri::command]
fn open_log_directory(
    app: AppHandle,
    storage: tauri::State<'_, StorageState>,
    logging: tauri::State<'_, logging::LoggingState>,
) -> Result<(), String> {
    let settings = storage
        .database
        .lock()
        .map_err(|_| "database lock is unavailable".to_owned())?
        .settings()
        .map_err(|error| error.to_string())?;
    let directory = logging
        .snapshot(&settings)
        .effective_directory
        .ok_or_else(|| "file logging is unavailable".to_owned())?;
    app.opener()
        .open_path(directory, None::<&str>)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn clear_giveaway_data(monitor: tauri::State<'_, MonitorState>) -> Result<(), String> {
    monitor.runtime.clear_giveaway_data()?;
    log::info!("saved giveaways and notification history cleared; next baseline will be silent");
    Ok(())
}
#[tauri::command]
fn get_notification_history(
    storage: tauri::State<'_, StorageState>,
) -> Result<Vec<NotificationRecord>, String> {
    storage
        .database
        .lock()
        .map_err(|_| "database lock is unavailable".to_owned())?
        .notifications()
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn get_notification_history_page(
    storage: tauri::State<'_, StorageState>,
    cursor: Option<String>,
) -> Result<NotificationHistoryPage, String> {
    let database = storage.database.clone();
    // Paging may wait for a short write or read older rows. Keep both the DB
    // lock and SQLite work off the native window's event thread.
    tauri::async_runtime::spawn_blocking(move || {
        database
            .lock()
            .map_err(|_| "database lock is unavailable".to_owned())?
            .notification_history_page(cursor.as_deref())
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
fn mark_all_notifications_read(
    app: AppHandle,
    storage: tauri::State<'_, StorageState>,
) -> Result<u64, String> {
    let (changed, unread) = {
        let database = storage
            .database
            .lock()
            .map_err(|_| "database lock is unavailable".to_owned())?;
        let changed = database
            .mark_all_notifications_read()
            .map_err(|error| error.to_string())?;
        let unread = database
            .counts()
            .map_err(|error| error.to_string())?
            .unread_notifications;
        (changed, unread)
    };
    sync_attention(&app, unread, false)?;
    emit_history_changed(&app);
    Ok(changed)
}

#[tauri::command]
fn mark_notification_read(
    app: AppHandle,
    storage: tauri::State<'_, StorageState>,
    notification_id: String,
) -> Result<(), String> {
    let unread = {
        let database = storage
            .database
            .lock()
            .map_err(|_| "database lock is unavailable".to_owned())?;
        database
            .mark_notification_read(&notification_id)
            .map_err(|error| error.to_string())?;
        database
            .counts()
            .map_err(|error| error.to_string())?
            .unread_notifications
    };
    sync_attention(&app, unread, false)?;
    emit_history_changed(&app);
    Ok(())
}

#[tauri::command]
fn open_notification(
    app: tauri::AppHandle,
    storage: tauri::State<'_, StorageState>,
    notification_id: String,
) -> Result<(), String> {
    activate_notification(&app, storage.inner(), &notification_id)
}

#[tauri::command]
async fn send_test_notification(
    app: AppHandle,
    storage: tauri::State<'_, StorageState>,
) -> Result<(), String> {
    let settings = storage
        .database
        .lock()
        .map_err(|_| "database lock is unavailable".to_owned())?
        .settings()
        .map_err(|error| error.to_string())?;
    let language = effective_language(&settings);
    let request = test_notification_request(language, settings.notification_sound_enabled);
    let activation_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        notifications::show(&app, &request, Some("giveaway-scout://show"), move || {
            if let Err(error) = show_main_window(&activation_app) {
                log::warn!("could not open Giveaway Scout from test notification: {error}");
            }
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

fn validated_giveaway_url(value: &str) -> Result<url::Url, String> {
    let url = url::Url::parse(value).map_err(|error| error.to_string())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("only HTTP and HTTPS giveaway links can be opened".to_owned());
    }
    Ok(url)
}

fn development_profile(identifier: &str, development_build: bool) -> bool {
    development_build || identifier.ends_with(".dev")
}

#[cfg(target_os = "windows")]
fn sync_autostart(app: &AppHandle, settings: &AppSettings) -> Result<(), String> {
    giveaway_scout_platform::sync_autostart(
        settings,
        development_profile(&app.config().identifier, tauri::is_dev()),
    )
    .map_err(|error| error.to_string())
}

#[cfg(not(target_os = "windows"))]
fn sync_autostart(app: &AppHandle, settings: &AppSettings) -> Result<(), String> {
    let enabled = settings.start_at_login;
    if development_profile(&app.config().identifier, tauri::is_dev()) {
        log::debug!("autostart synchronization skipped for the development build");
        return Ok(());
    }

    let manager = app.autolaunch();
    if enabled {
        // Re-register even when an entry already exists so an existing entry or a moved
        // portable executable is updated to the current Tauri binary and arguments.
        manager.enable().map_err(|error| error.to_string())
    } else {
        let currently_enabled = manager.is_enabled().map_err(|error| error.to_string())?;
        if currently_enabled {
            manager.disable().map_err(|error| error.to_string())
        } else {
            Ok(())
        }
    }
}

fn effective_language(settings: &AppSettings) -> &'static str {
    match settings.language_code.as_deref() {
        Some("ru") => "ru",
        Some("en") => "en",
        _ if sys_locale::get_locale()
            .is_some_and(|locale| locale.to_lowercase().starts_with("ru")) =>
        {
            "ru"
        }
        _ => "en",
    }
}

fn configured_log_level(verbose_logging: bool) -> log::LevelFilter {
    if verbose_logging {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    }
}

fn apply_log_level(verbose_logging: bool) {
    log::set_max_level(configured_log_level(verbose_logging));
}

fn tray_labels(language: &str) -> TrayLabels {
    if language == "ru" {
        TrayLabels {
            open: "Открыть",
            check_now: "Проверить сейчас",
            quit: "Выйти",
        }
    } else {
        TrayLabels {
            open: "Open",
            check_now: "Check now",
            quit: "Quit",
        }
    }
}

fn update_tray_labels(app: &AppHandle, language: &str) {
    let Some(menu) = app.try_state::<TrayMenuState>() else {
        return;
    };
    let labels = tray_labels(language);
    for result in [
        menu.open.set_text(labels.open),
        menu.check_now.set_text(labels.check_now),
        menu.quit.set_text(labels.quit),
    ] {
        if let Err(error) = result {
            log::warn!("could not update tray menu language: {error}");
        }
    }
}

fn main_window_close_action(settings: Option<&AppSettings>) -> MainWindowCloseAction {
    if settings.is_some_and(|value| value.keep_running_in_tray) {
        MainWindowCloseAction::Hide
    } else {
        MainWindowCloseAction::Exit
    }
}

fn is_autostart_launch<I, S>(arguments: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    arguments
        .into_iter()
        .any(|argument| argument.as_ref() == AUTOSTART_ARGUMENT)
}

fn should_show_startup_window(
    autostart_launch: bool,
    service_launch: bool,
    start_in_tray: bool,
    initial_sync_choice_required: bool,
    opened_from_notification: bool,
) -> bool {
    initial_sync_choice_required
        || !(service_launch || opened_from_notification || (autostart_launch && start_in_tray))
}

fn show_main_window(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window("main") {
        window.show()?;
        window.unminimize()?;
        window.set_focus()?;
    }
    Ok(())
}

fn deep_link_action(url: &url::Url) -> Option<DeepLinkAction> {
    if url.scheme() != DEEP_LINK_SCHEME {
        return None;
    }

    let segments = url
        .path_segments()
        .map(|segments| {
            segments
                .filter(|segment| !segment.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    match (url.host_str(), segments.as_slice()) {
        (Some("show"), []) => Some(DeepLinkAction::Show),
        (Some("notification"), [notification_id])
            if uuid::Uuid::parse_str(notification_id).is_ok() =>
        {
            Some(DeepLinkAction::Notification((*notification_id).to_owned()))
        }
        _ => None,
    }
}

fn handle_deep_link(app: &AppHandle, url: &url::Url) -> Result<(), String> {
    match deep_link_action(url) {
        Some(DeepLinkAction::Show) => show_main_window(app).map_err(|error| error.to_string()),
        Some(DeepLinkAction::Notification(notification_id)) => {
            let storage = app.state::<StorageState>();
            activate_notification(app, storage.inner(), &notification_id)
        }
        None => Err("unsupported Giveaway Scout deep link".to_owned()),
    }
}

fn handle_deep_links(app: &AppHandle, urls: impl IntoIterator<Item = url::Url>) {
    for url in urls {
        if let Err(error) = handle_deep_link(app, &url) {
            log::warn!("could not handle deep link {url}: {error}");
        }
    }
}

fn is_notification_deep_link(value: &str) -> bool {
    value
        .parse::<url::Url>()
        .ok()
        .and_then(|url| deep_link_action(&url))
        .is_some_and(|action| matches!(action, DeepLinkAction::Notification(_)))
}

fn activate_notification(
    app: &AppHandle,
    storage: &StorageState,
    notification_id: &str,
) -> Result<(), String> {
    let notification = storage
        .database
        .lock()
        .map_err(|_| "database lock is unavailable".to_owned())?
        .notification(notification_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "notification was not found".to_owned())?;
    let url = validated_giveaway_url(&notification.url)?;

    #[cfg(feature = "ui-tests")]
    ui_tests::record(ui_tests::Event::OpenUrl { url: url.as_str() })?;
    #[cfg(not(feature = "ui-tests"))]
    app.opener()
        .open_url(url.as_str(), None::<&str>)
        .map_err(|error| error.to_string())?;
    let unread = {
        let database = storage
            .database
            .lock()
            .map_err(|_| "database lock is unavailable".to_owned())?;
        database
            .mark_notification_read(notification_id)
            .map_err(|error| error.to_string())?;
        database
            .counts()
            .map_err(|error| error.to_string())?
            .unread_notifications
    };
    sync_attention(app, unread, false)?;
    emit_history_changed(app);
    Ok(())
}

fn unread_notification_count(storage: &StorageState) -> Result<u64, String> {
    storage
        .database
        .lock()
        .map_err(|_| "database lock is unavailable".to_owned())?
        .counts()
        .map(|counts| counts.unread_notifications)
        .map_err(|error| error.to_string())
}

// Window getters synchronously wait for the main thread. Accept only an owned
// count so callers release the database before entering any GUI API: that thread
// can itself be waiting for storage while serving a frontend command.
fn sync_attention(app: &AppHandle, unread: u64, has_new: bool) -> Result<(), String> {
    #[cfg(feature = "ui-tests")]
    ui_tests::record(ui_tests::Event::Attention { unread })?;
    #[cfg(target_os = "linux")]
    if let Some(tray) = app.tray_by_id(TRAY_ID)
        && let Some(icon) = app.default_window_icon()
        && let Some(menu) = app.try_state::<TrayMenuState>()
    {
        let has_unread = unread > 0;
        let mut previous = menu
            .last_has_unread
            .lock()
            .map_err(|_| "tray attention lock is unavailable".to_owned())?;
        // Linux writes changed tray icons to disk. Avoid rewriting on each poll.
        if *previous != Some(has_unread) {
            tray.set_icon(Some(attention::tray_image(icon, has_unread)))
                .map_err(|error| error.to_string())?;
            *previous = Some(has_unread);
        }
    }
    if let Some(window) = app.get_webview_window("main") {
        #[cfg(feature = "ui-tests")]
        ui_tests::before_attention_window_query(app)?;
        let focused = window.is_focused().map_err(|error| error.to_string())?;
        #[cfg(feature = "ui-tests")]
        ui_tests::after_attention_window_query(focused)?;
        let attention = match attention::window_attention(has_new, unread, focused) {
            attention::WindowAttention::Request => Some(UserAttentionType::Informational),
            attention::WindowAttention::Clear => None,
            attention::WindowAttention::Keep => return Ok(()),
        };
        window
            .request_user_attention(attention)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn emit_history_changed(app: &AppHandle) {
    if let Err(error) = app.emit("history-changed", ()) {
        log::warn!("could not emit history change event: {error}");
    }
}

fn notification_request(
    notification: &NotificationRecord,
    language: &str,
    play_sound: bool,
) -> NotificationRequest {
    let discovered_source = match notification.source.as_str() {
        "giveeClub" | "GiveeClub" => "Givee Club",
        "gamesForFarm" | "GamesForFarm" => "Games for Farm",
        "zeros" | "Zeros" => "Zeros",
        "freeSteamKeys" | "FreeSteamKeys" => "FreeSteamKeys",
        value => value,
    };
    let source = notification
        .origin_name
        .as_deref()
        .unwrap_or(discovered_source);
    let is_raffle = notification.kind.eq_ignore_ascii_case("raffle");
    let (prefix, source_label, link_label, action_label) = if language == "ru" {
        (
            if is_raffle {
                "Новый розыгрыш"
            } else {
                "Новая раздача"
            },
            "Источник",
            "Ссылка",
            "Открыть",
        )
    } else {
        (
            if is_raffle {
                "New raffle"
            } else {
                "New giveaway"
            },
            "Source",
            "Link",
            "Open",
        )
    };

    let (platform_label, unknown_platform) = if language == "ru" {
        ("Платформа", "не определена")
    } else {
        ("Platform", "unknown")
    };
    let platform = notification
        .store
        .display_name()
        .unwrap_or(unknown_platform);

    NotificationRequest {
        title: format!("{prefix}: {}", notification.title),
        body: format!(
            "{platform_label}: {platform}\n{source_label}: {source}\n{link_label}: {}",
            notification.url
        ),
        action_label: action_label.to_owned(),
        play_sound,
    }
}

fn test_notification_request(language: &str, play_sound: bool) -> NotificationRequest {
    if language == "ru" {
        NotificationRequest {
            title: "Тестовое уведомление Giveaway Scout".to_owned(),
            body: "Системные уведомления работают. Нажмите, чтобы открыть приложение.".to_owned(),
            action_label: "Открыть".to_owned(),
            play_sound,
        }
    } else {
        NotificationRequest {
            title: "Giveaway Scout test notification".to_owned(),
            body: "System notifications are working. Click to open the app.".to_owned(),
            action_label: "Open".to_owned(),
            play_sound,
        }
    }
}

fn deliver_notification(
    app: &AppHandle,
    notification: &NotificationRecord,
    settings: &AppSettings,
) -> Result<(), String> {
    let request = notification_request(
        notification,
        effective_language(settings),
        settings.notification_sound_enabled,
    );
    let activation_id = notification.id.clone();
    let activation_app = app.clone();
    let activation_url = format!("{DEEP_LINK_SCHEME}://notification/{}", notification.id);
    notifications::show(app, &request, Some(&activation_url), move || {
        let storage = activation_app.state::<StorageState>();
        if let Err(error) = activate_notification(&activation_app, storage.inner(), &activation_id)
        {
            log::warn!("could not activate notification {activation_id}: {error}");
        }
    })
}

fn runtime_event(app: &AppHandle, event: RuntimeEvent) {
    match event {
        RuntimeEvent::HistoryChanged => {
            if let Some(storage) = app.try_state::<StorageState>()
                && let Err(error) = unread_notification_count(storage.inner())
                    .and_then(|unread| sync_attention(app, unread, false))
            {
                log::warn!("could not refresh unread notification attention: {error}");
            }
            emit_history_changed(app);
        }
        RuntimeEvent::MonitorChanged => emit_monitor_changed(app),
        RuntimeEvent::SourceCompleted(completed) => {
            let has_new = completed
                .outcome
                .as_ref()
                .is_ok_and(|outcome| !outcome.new_items.is_empty());
            if has_new
                && let Some(storage) = app.try_state::<StorageState>()
                && let Err(error) = unread_notification_count(storage.inner())
                    .and_then(|unread| sync_attention(app, unread, true))
            {
                log::warn!("could not request new notification attention: {error}");
            }
        }
    }
}

fn spawn_check(app: &AppHandle) {
    let runtime = app.state::<MonitorState>().runtime.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = runtime.check_now().await {
            log::warn!("tray giveaway check did not start: {error}");
        }
    });
}
fn create_tray(app: &tauri::App, language: &str) -> tauri::Result<TrayMenuState> {
    let labels = tray_labels(language);
    let open = MenuItem::with_id(app, TRAY_OPEN_ID, labels.open, true, None::<&str>)?;
    let check_now =
        MenuItem::with_id(app, TRAY_CHECK_NOW_ID, labels.check_now, true, None::<&str>)?;
    let quit = MenuItem::with_id(app, TRAY_QUIT_ID, labels.quit, true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &check_now, &quit])?;
    let mut tray = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .tooltip("Giveaway Scout")
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            TRAY_OPEN_ID => {
                if let Err(error) = show_main_window(app) {
                    log::warn!("could not show the main window from tray: {error}");
                }
            }
            TRAY_CHECK_NOW_ID => spawn_check(app),
            TRAY_QUIT_ID => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) && let Err(error) = show_main_window(tray.app_handle())
            {
                log::warn!("could not show the main window from tray click: {error}");
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;

    Ok(TrayMenuState {
        open,
        check_now,
        quit,
        #[cfg(target_os = "linux")]
        last_has_unread: Mutex::new(None),
    })
}

#[tauri::command]
async fn check_now(app: AppHandle) -> Result<CheckStartResult, String> {
    let runtime = app.state::<MonitorState>().runtime.clone();
    runtime.check_now().await
}

fn emit_monitor_changed(app: &AppHandle) {
    if let Err(error) = app.emit("monitor-changed", ()) {
        log::warn!("could not emit monitor change event: {error}");
    }
}

fn now_utc() -> String {
    giveaway_scout_runtime::now_utc()
}
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut context = tauri::generate_context!();
    #[cfg(feature = "ui-tests")]
    let test_profile = ui_tests::initialize(&context.config().identifier).unwrap_or_else(|error| {
        eprintln!("Unsafe UI test startup refused: {error}");
        std::process::exit(2)
    });
    #[cfg(feature = "ui-tests")]
    {
        // A unique ID also isolates single-instance locks for parallel test runs.
        let suffix = test_profile
            .root
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .trim_start_matches("giveaway-scout-e2e-");
        context.config_mut().identifier = format!("com.giveawayscout.desktop.e2e.{suffix}.dev");
        for window in &mut context.config_mut().app.windows {
            window.data_directory = Some(test_profile.root.join("webview"));
        }
    }
    if tauri::is_dev() && !context.config().identifier.ends_with(".dev") {
        // Keep the development process, WebView profile, logs, SQLite database and
        // single-instance lock separate from installed and portable release builds.
        context.config_mut().identifier.push_str(".dev");
    }

    let builder = tauri::Builder::default();
    #[cfg(feature = "ui-tests")]
    let builder = builder.plugin(tauri_plugin_wdio_webdriver::init_with_port(
        test_profile.port,
    ));
    let application = builder
        .plugin(tauri_plugin_single_instance::init(|app, arguments, _cwd| {
            #[cfg(target_os = "linux")]
            if linux_activation::is_service_launch(&arguments) {
                return;
            }
            if !arguments
                .iter()
                .any(|argument| is_notification_deep_link(argument))
                && let Err(error) = show_main_window(app)
            {
                log::warn!("could not focus the existing application instance: {error}");
            }
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .app_name("Giveaway Scout")
                .arg(AUTOSTART_ARGUMENT)
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            if matches!(event, WindowEvent::Focused(true)) {
                if let Err(error) = window.request_user_attention(None) {
                    log::warn!("could not clear taskbar attention: {error}");
                }
                return;
            }
            let WindowEvent::CloseRequested { api, .. } = event else {
                return;
            };
            let settings = window
                .app_handle()
                .try_state::<StorageState>()
                .and_then(|storage| storage.database.lock().ok()?.settings().ok());
            match main_window_close_action(settings.as_ref()) {
                MainWindowCloseAction::Hide => {
                    api.prevent_close();
                    if let Err(error) = window.hide() {
                        log::warn!("could not hide the main window to tray: {error}");
                    }
                }
                MainWindowCloseAction::Exit => window.app_handle().exit(0),
            }
        })
        .setup(|app| {
            #[cfg(not(feature = "ui-tests"))]
            let data_directory = app.path().app_local_data_dir()?;
            #[cfg(feature = "ui-tests")]
            let data_directory = ui_tests::profile().root.join("data");
            let profile_guard = Arc::new(ProfileGuard::acquire(&data_directory).inspect_err(|error| {
                #[cfg(all(target_os = "windows", not(feature = "ui-tests")))]
                show_profile_open_error(error);
                #[cfg(any(not(target_os = "windows"), feature = "ui-tests"))]
                let _ = error;
            })?);
            #[cfg(feature = "ui-tests")]
            let new_profile = !data_directory.join("giveaway-scout.db").exists();
            let (database, database_recovery) =
                Database::open_with_recovery(data_directory.join("giveaway-scout.db"))?;
            #[cfg(feature = "ui-tests")]
            if new_profile { database.update_settings(&ui_tests::seed_settings())?; }
            let settings = database.settings()?;
            let default_log_directory = app
                .path()
                .app_log_dir()
                .unwrap_or_else(|_| data_directory.join("logs"));
            #[cfg(not(feature = "ui-tests"))]
            let forced_log_directory = None;
            #[cfg(feature = "ui-tests")]
            let forced_log_directory = Some(ui_tests::log_directory(&settings));
            let (logging_state, file_dispatch) =
                logging::initialize(&settings, default_log_directory, forced_log_directory);
            app.manage(logging_state);
            app.handle().plugin(
                tauri_plugin_log::Builder::new()
                    .level(log::LevelFilter::Debug)
                    .targets([
                        tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                        tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Dispatch(
                            file_dispatch,
                        )),
                    ])
                    .build(),
            )?;
            apply_log_level(settings.verbose_logging);
            if let Some(recovery) = database_recovery.as_ref() {
                log::error!(
                    "damaged SQLite database was preserved at {} before creating a clean database: {}",
                    recovery.backup_path,
                    recovery.original_error
                );
            }
            let initial_sync_choice_required = database.initial_sync_choice_required()?;

            let database = Arc::new(Mutex::new(database));
            let delivery_app = app.handle().clone();
            let observer_app = app.handle().clone();
            let runtime = Runtime::new(
                database.clone(),
                redirect_pipeline::dependencies(
                    Arc::new(move |record, settings| deliver_notification(&delivery_app, record, settings)),
                    Arc::new(move |event| runtime_event(&observer_app, event)),
                ),
            ).map_err(std::io::Error::other)?;
            runtime.retain_profile_guard(profile_guard.clone());
            app.manage(StorageState {
                database,
                database_recovery,
                _profile_guard: profile_guard,
            });
            app.manage(MonitorState {
                data_operation: runtime.data_operation(),
                runtime,
            });
            app.manage(create_tray(app, effective_language(&settings))?);
            updates::initialize(app.handle())?;

            #[cfg(target_os = "macos")]
            macos_notifications::attach(app.handle());

            #[cfg(target_os = "linux")]
            let activation_service_ready = match linux_activation::initialize(app.handle()) {
                Ok(()) => true,
                Err(error) => {
                    log::warn!("Linux D-Bus activation is unavailable: {error}");
                    false
                }
            };

            #[cfg(target_os = "linux")]
            if let Err(error) = app.deep_link().register_all() {
                log::warn!("could not register Giveaway Scout deep links: {error}");
            }

            let initial_deep_links = app.deep_link().get_current()?.unwrap_or_default();
            let opened_from_notification = initial_deep_links.iter().any(|url| {
                matches!(
                    deep_link_action(url),
                    Some(DeepLinkAction::Notification(_))
                )
            });
            #[cfg(target_os = "macos")]
            let opened_from_notification = opened_from_notification || macos_notifications::opened_from_notification();
            let activation_app = app.handle().clone();
            app.deep_link().on_open_url(move |event| {
                handle_deep_links(&activation_app, event.urls());
            });
            handle_deep_links(app.handle(), initial_deep_links);

            if let Some(storage) = app.try_state::<StorageState>()
                && let Err(error) = unread_notification_count(storage.inner())
                    .and_then(|unread| sync_attention(app.handle(), unread, false))
            {
                log::warn!("could not restore unread notification attention: {error}");
            }

            if let Err(error) = sync_autostart(app.handle(), &settings) {
                log::error!("could not reconcile autostart setting: {error}");
            }

            #[cfg(target_os = "linux")]
            let service_launch = activation_service_ready
                && linux_activation::is_service_launch(std::env::args_os());
            #[cfg(not(target_os = "linux"))]
            let service_launch = false;
            if should_show_startup_window(
                is_autostart_launch(std::env::args_os()),
                service_launch,
                settings.start_in_tray,
                initial_sync_choice_required,
                opened_from_notification,
            ) {
                show_main_window(app.handle())?;
            } else {
                log::info!("application started hidden in tray by autostart or system activation");
            }

            let runtime = app.state::<MonitorState>().runtime.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = runtime.run(RunMode::Watch).await {
                    log::error!("giveaway monitor stopped: {error}");
                }
            });
            usage_statistics::start(app.handle());
            log::info!("Giveaway Scout backend initialized");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_app_state,
            get_storage_snapshot,
            update_settings,
            get_logging_state,
            choose_log_directory,
            open_log_directory,
            clear_giveaway_data,
            get_notification_history,
            get_notification_history_page,
            mark_all_notifications_read,
            mark_notification_read,
            open_notification,
            send_test_notification,
            check_now,
            #[cfg(feature = "ui-tests")]
            ui_tests::get_ui_test_identity,
            updates::get_update_state,
            updates::check_for_updates,
            updates::install_update
        ])
        .build(context)
        .expect("error while building tauri application");

    // Tao has now created its own NSApplication subclass, but AppKit has not
    // finished launching: Ready/setup happens inside run. Register before that
    // boundary so cold notification responses cannot arrive without a delegate.
    #[cfg(target_os = "macos")]
    if let Err(error) = macos_notifications::register_early(&application.config().identifier) {
        log::warn!("macOS notifications unavailable: {error}");
    }
    application.run(|app, event| {
        if let tauri::RunEvent::ExitRequested { api, code, .. } = event
            && let Some(monitor) = app.try_state::<MonitorState>()
            && !monitor.runtime.is_stopped()
        {
            api.prevent_exit();
            let runtime = monitor.runtime.clone();
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                runtime.shutdown().await;
                app.exit(code.unwrap_or(0));
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use giveaway_scout_core::{GiveawaySource, MonitoringOutcome, SourceFailure};

    #[test]
    fn giveaway_notifications_show_platform_for_both_languages_and_kinds() {
        use giveaway_scout_core::{
            Database, GameStore, GiveawayItem, GiveawayKind, InitialSyncMode,
        };
        for (store, name) in [
            (GameStore::Steam, "Steam"),
            (GameStore::EpicGames, "Epic Games"),
            (GameStore::Gog, "GOG"),
            (GameStore::Unknown, ""),
        ] {
            for kind in [GiveawayKind::Giveaway, GiveawayKind::Raffle] {
                let mut database = Database::open_in_memory().unwrap();
                let mut settings = database.settings().unwrap();
                settings.initial_sync_mode = Some(InitialSyncMode::NotifyCurrent);
                database.update_settings(&settings).unwrap();
                database
                    .process_discovered_items(
                        &[GiveawayItem {
                            source: GiveawaySource::GiveeClub,
                            source_id: "test".to_owned(),
                            title: "Example game".to_owned(),
                            url: "https://givee.club/event/test".to_owned(),
                            kind,
                            store,
                            details: None,
                        }],
                        &GiveawaySource::ALL,
                    )
                    .unwrap();
                let record = database.notifications().unwrap().remove(0);
                for (language, label, unknown) in [
                    ("ru", "Платформа", "не определена"),
                    ("en", "Platform", "unknown"),
                ] {
                    let request = super::notification_request(&record, language, false);
                    let platform = if store == GameStore::Unknown {
                        unknown
                    } else {
                        name
                    };
                    assert!(request.body.starts_with(&format!("{label}: {platform}\n")));
                    assert!(request.body.contains("Givee Club"));
                    assert!(request.body.contains(&record.url));
                    assert!(request.title.contains("Example game"));
                    assert!(!request.play_sound);
                }
            }
        }
    }

    #[test]
    fn free_steam_keys_notification_shows_its_source_and_steam_platform() {
        let record = giveaway_scout_core::NotificationRecord {
            store: giveaway_scout_core::GameStore::Steam,
            id: "fsk-test".into(),
            tracking_key: Some("FreeSteamKeys:258".into()),
            campaign_id: None,
            origin_source: None,
            origin_name: None,
            detected_sources: vec![GiveawaySource::FreeSteamKeys],
            source: "freeSteamKeys".into(),
            kind: "giveaway".into(),
            title: "Sorcerer Adventure".into(),
            url: "https://www.freesteamkeys.com/open/sorcerer-adventure".into(),
            received_at_utc: "2026-11-07T10:00:00Z".into(),
            read_at_utc: None,
            delivery_state: "pending".into(),
            delivery_attempts: 0,
            last_delivery_error: None,
        };
        for language in ["ru", "en"] {
            let request = super::notification_request(&record, language, true);
            assert!(request.body.contains("FreeSteamKeys"));
            assert!(request.body.contains("Steam"));
            assert!(request.body.contains(&record.url));
            assert!(request.title.contains("Sorcerer Adventure"));
            assert!(request.play_sound);
        }
    }

    #[test]
    fn a_bundled_development_profile_keeps_autostart_disabled_without_a_dev_server() {
        assert!(!super::development_profile(
            "com.giveawayscout.desktop",
            false
        ));
        assert!(super::development_profile(
            "com.giveawayscout.desktop",
            true
        ));
        assert!(super::development_profile(
            "com.giveawayscout.desktop.dev",
            false
        ));
        assert!(super::development_profile(
            "com.giveawayscout.desktop.dev",
            true
        ));
        assert!(!super::development_profile(
            "com.giveawayscout.desktop.development",
            false
        ));
    }

    #[test]
    fn startup_visibility_never_hides_the_initial_choice_dialog() {
        for autostart in [false, true] {
            for service in [false, true] {
                for tray in [false, true] {
                    for notification in [false, true] {
                        assert!(super::should_show_startup_window(
                            autostart,
                            service,
                            tray,
                            true,
                            notification
                        ));
                    }
                }
            }
        }
        for (autostart, service, tray, notification, visible) in [
            (false, false, false, false, true),
            (false, false, true, false, true),
            (true, false, false, false, true),
            (true, false, true, false, false),
            (false, true, false, false, false),
            (false, false, false, true, false),
        ] {
            assert_eq!(
                super::should_show_startup_window(autostart, service, tray, false, notification),
                visible
            );
        }
    }

    use giveaway_scout_core::AppSettings;

    use super::{
        AUTOSTART_ARGUMENT, DeepLinkAction, MainWindowCloseAction, MonitorSnapshot,
        configured_log_level, deep_link_action, is_autostart_launch, is_notification_deep_link,
        main_window_close_action, tray_labels, validated_giveaway_url,
    };

    #[test]
    fn detailed_logging_switches_between_info_and_debug() {
        assert_eq!(configured_log_level(false), log::LevelFilter::Info);
        assert_eq!(configured_log_level(true), log::LevelFilter::Debug);
    }

    #[test]
    fn parses_only_expected_deep_link_shapes() {
        assert_eq!(
            deep_link_action(&"giveaway-scout://show".parse().unwrap()),
            Some(DeepLinkAction::Show)
        );
        assert_eq!(
            deep_link_action(
                &"giveaway-scout://notification/45411a99-4b9e-4532-b87a-9203ef6ce2bc"
                    .parse()
                    .unwrap()
            ),
            Some(DeepLinkAction::Notification(
                "45411a99-4b9e-4532-b87a-9203ef6ce2bc".to_owned()
            ))
        );
        assert!(deep_link_action(&"https://givee.club/event/1465".parse().unwrap()).is_none());
        assert!(
            deep_link_action(&"giveaway-scout://notification/not-a-uuid".parse().unwrap())
                .is_none()
        );
        assert!(is_notification_deep_link(
            "giveaway-scout://notification/45411a99-4b9e-4532-b87a-9203ef6ce2bc"
        ));
    }

    #[test]
    fn tray_labels_are_localized() {
        assert_eq!(tray_labels("ru").quit, "Выйти");
        assert_eq!(tray_labels("en").check_now, "Check now");
    }

    #[test]
    fn close_action_follows_tray_setting() {
        assert_eq!(main_window_close_action(None), MainWindowCloseAction::Exit);
        let mut settings = AppSettings::default();
        assert_eq!(
            main_window_close_action(Some(&settings)),
            MainWindowCloseAction::Exit
        );
        settings.keep_running_in_tray = true;
        assert_eq!(
            main_window_close_action(Some(&settings)),
            MainWindowCloseAction::Hide
        );
    }

    #[test]
    fn autostart_visibility_and_window_close_settings_are_independent() {
        for start_in_tray in [false, true] {
            for keep_running_in_tray in [false, true] {
                let settings = AppSettings {
                    start_in_tray,
                    keep_running_in_tray,
                    ..AppSettings::default()
                };
                assert_eq!(
                    main_window_close_action(Some(&settings)),
                    if keep_running_in_tray {
                        MainWindowCloseAction::Hide
                    } else {
                        MainWindowCloseAction::Exit
                    }
                );
                for autostart in [false, true] {
                    assert_eq!(
                        super::should_show_startup_window(
                            autostart,
                            false,
                            settings.start_in_tray,
                            false,
                            false
                        ),
                        !autostart || !start_in_tray
                    );
                }
            }
        }
    }

    #[test]
    fn detects_only_the_explicit_autostart_argument() {
        assert!(is_autostart_launch(["giveaway-scout", AUTOSTART_ARGUMENT]));
        assert!(!is_autostart_launch(["giveaway-scout", "--other"]));
    }

    #[test]
    fn giveaway_links_accept_only_http_and_https() {
        assert!(validated_giveaway_url("https://givee.club/event/42").is_ok());
        assert!(validated_giveaway_url("http://gamesforfarm.com/game?n=42").is_ok());
        assert!(validated_giveaway_url("file:///C:/Windows/System32/calc.exe").is_err());
        assert!(validated_giveaway_url("javascript:alert(1)").is_err());
    }

    #[test]
    fn source_snapshot_uses_active_states_and_keeps_diagnostics_structured() {
        use giveaway_scout_core::{SourceFailureReason, SourcePhase, SourceStatus};
        let mut healthy = SourceStatus::new(GiveawaySource::FreeSteamKeys);
        healthy.phase = SourcePhase::Checking;
        healthy.last_successful_check_utc = Some("2026-11-07T10:00:00Z".into());
        let mut failed = SourceStatus::new(GiveawaySource::GamesForFarm);
        failed.phase = SourcePhase::Error;
        failed.last_error = Some(SourceFailure {
            source: failed.source,
            message: "request timed out".into(),
            reason: SourceFailureReason::Timeout,
        });
        let mut disabled = SourceStatus::new(GiveawaySource::Zeros);
        disabled.phase = SourcePhase::Disabled;
        disabled.last_error = Some(SourceFailure {
            source: disabled.source,
            message: "old failure".into(),
            reason: SourceFailureReason::Unknown,
        });
        let snapshot = MonitorSnapshot::from_sources(vec![healthy, failed, disabled]);
        assert!(snapshot.is_checking);
        assert_eq!(snapshot.last_failure_count, 1);
        assert_eq!(
            snapshot.last_error.as_deref(),
            Some("Games for Farm: request timed out")
        );
        assert_eq!(
            snapshot.last_successful_check_utc.as_deref(),
            Some("2026-11-07T10:00:00Z")
        );
        let json = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(json["sourceFailures"][0]["reason"]["code"], "timeout");
        assert_eq!(json["sourceStates"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn partial_source_failures_produce_individual_warnings_and_a_summary() {
        let messages = super::check_result_diagnostics(&Ok(MonitoringOutcome {
            check_performed: true,
            failures: vec![
                SourceFailure {
                    source: GiveawaySource::GiveeClub,
                    message: "HTTP 503".to_owned(),
                    reason: giveaway_scout_core::SourceFailureReason::Http { status: 503 },
                },
                SourceFailure {
                    source: GiveawaySource::GamesForFarm,
                    message: "bad card\r\ntext".to_owned(),
                    reason: giveaway_scout_core::SourceFailureReason::InvalidFeed,
                },
            ],
            ..MonitoringOutcome::default()
        }));
        assert_eq!(messages.len(), 3);
        assert_eq!(
            messages[0],
            (
                log::Level::Warn,
                "giveaway source check failed: Givee Club: HTTP 503".to_owned()
            )
        );
        assert_eq!(
            messages[1],
            (
                log::Level::Warn,
                "giveaway source check failed: Games for Farm: bad card\\r\\ntext".to_owned()
            )
        );
        assert_eq!(messages[2].0, log::Level::Info);
        assert!(messages[2].1.contains("failures=2"));
    }

    #[test]
    fn healthy_and_fatal_checks_do_not_create_fake_source_warnings() {
        let healthy = super::check_result_diagnostics(&Ok(MonitoringOutcome::default()));
        assert_eq!(healthy.len(), 1);
        assert_eq!(healthy[0].0, log::Level::Info);
        assert!(healthy[0].1.contains("failures=0"));
        assert_eq!(
            super::check_result_diagnostics(&Err("database unavailable".to_owned())),
            vec![(
                log::Level::Error,
                "giveaway check failed: database unavailable".to_owned()
            )]
        );
    }
}
