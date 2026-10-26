#[cfg(all(feature = "ui-tests", not(debug_assertions)))]
compile_error!("ui-tests must never be included in Release");
#[cfg(all(feature = "ui-tests", not(target_os = "windows")))]
compile_error!("the native UI test harness currently supports Windows only");
#[cfg(any(target_os = "linux", test))]
mod attention;
#[cfg(target_os = "linux")]
mod linux_activation;
#[cfg(target_os = "linux")]
mod linux_notifications;
#[cfg(target_os = "macos")]
mod macos_notifications;
#[cfg(not(feature = "ui-tests"))]
mod notifications;
#[cfg(feature = "ui-tests")]
#[path = "ui_test_notifications.rs"]
mod notifications;
mod operations;
#[cfg(feature = "ui-tests")]
mod ui_tests;
mod updates;
mod usage_statistics;
#[cfg(target_os = "windows")]
mod windows_installation;

use std::{path::PathBuf, sync::Mutex, time::Duration};

#[cfg(not(feature = "ui-tests"))]
use giveaway_scout_core::HttpPageFetcher;
use giveaway_scout_core::{
    AppSettings, AppState, DataCounts, Database, DatabaseRecoveryReport, GiveawaySource,
    LegacyImportReport, MonitoringOutcome, NotificationRecord, SourceFailure, apply_source_check,
    check_sources, dispatch_pending_notifications,
};
use serde::Serialize;
#[cfg(not(feature = "ui-tests"))]
use tauri::UserAttentionType;
use tauri::{
    AppHandle, Emitter, Manager, WindowEvent, Wry,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use tauri_plugin_autostart::ManagerExt as AutostartManagerExt;
use tauri_plugin_deep_link::DeepLinkExt;
#[cfg(not(feature = "ui-tests"))]
use tauri_plugin_opener::OpenerExt;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio::sync::Notify;
#[cfg(feature = "ui-tests")]
use ui_tests::UiPageFetcher as HttpPageFetcher;

use crate::notifications::NotificationRequest;
use crate::operations::DataOperationGate;

struct StorageState {
    database: Mutex<Database>,
    database_recovery: Option<DatabaseRecoveryReport>,
    legacy_import: Option<LegacyImportReport>,
    migration_error: Option<String>,
}

struct MonitorState {
    fetcher: Mutex<HttpPageFetcher>,
    data_operation: DataOperationGate,
    status: Mutex<MonitorSnapshot>,
    schedule_changed: Notify,
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
    legacy_import: Option<LegacyImportReport>,
    migration_error: Option<String>,
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
}

impl MonitorSnapshot {
    fn record_started(&mut self, timestamp: String) {
        self.is_checking = true;
        self.last_started_at_utc = Some(timestamp);
        self.last_error = None;
        self.source_failures.clear();
    }

    fn record_finished(&mut self, timestamp: String, result: &Result<MonitoringOutcome, String>) {
        self.is_checking = false;
        self.last_completed_at_utc = Some(timestamp.clone());

        match result {
            Ok(outcome) => {
                self.last_new_items = outcome.new_items.len() as u64;
                self.last_failure_count = outcome.failures.len() as u64;
                self.source_failures = outcome.failures.clone();
                if outcome.check_performed && outcome.failures.is_empty() {
                    self.last_successful_check_utc = Some(timestamp);
                    self.last_error = None;
                } else if !outcome.failures.is_empty() {
                    self.last_error = Some(
                        outcome
                            .failures
                            .iter()
                            .map(source_failure_description)
                            .collect::<Vec<_>>()
                            .join("; "),
                    );
                }
            }
            Err(error) => {
                self.last_error = Some(error.clone());
                self.last_new_items = 0;
                self.last_failure_count = 0;
                self.source_failures.clear();
            }
        }
    }
}

fn source_failure_description(failure: &SourceFailure) -> String {
    let source = match failure.source {
        GiveawaySource::GiveeClub => "Givee Club",
        GiveawaySource::GamesForFarm => "Games for Farm",
        GiveawaySource::Zeros => "Zeros",
    };
    let message = failure.message.replace('\r', "\\r").replace('\n', "\\n");
    format!("{source}: {message}")
}

fn check_result_diagnostics(
    result: &Result<MonitoringOutcome, String>,
) -> Vec<(log::Level, String)> {
    match result {
        Ok(outcome) => {
            let mut messages: Vec<_> = outcome
                .failures
                .iter()
                .map(|failure| {
                    (
                        log::Level::Warn,
                        format!(
                            "giveaway source check failed: {}",
                            source_failure_description(failure)
                        ),
                    )
                })
                .collect();
            messages.push((log::Level::Info, format!(
                "giveaway check finished: new_items={}, failures={}, initial_sync={}, baseline_deferred={}",
                outcome.new_items.len(), outcome.failures.len(),
                outcome.is_initial_synchronization, outcome.baseline_deferred,
            )));
            messages
        }
        Err(error) => vec![(log::Level::Error, format!("giveaway check failed: {error}"))],
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
    let monitor_snapshot = monitor
        .status
        .lock()
        .map_err(|_| "monitor status lock is unavailable".to_owned())?
        .clone();

    Ok(StorageSnapshot {
        settings: database.settings().map_err(|error| error.to_string())?,
        counts: database.counts().map_err(|error| error.to_string())?,
        data_size_bytes: database
            .data_size_bytes()
            .map_err(|error| error.to_string())?,
        database_recovery: storage.database_recovery.clone(),
        initial_sync_choice_required: database
            .initial_sync_choice_required()
            .map_err(|error| error.to_string())?,
        legacy_import: storage.legacy_import,
        migration_error: storage.migration_error.clone(),
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
    let updated_fetcher =
        HttpPageFetcher::with_policy(settings.http_timeout_seconds, settings.http_retry_count)
            .map_err(|error| error.to_string())?;
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

    if previous.start_at_login != settings.start_at_login
        && let Err(error) = sync_autostart(&app, settings.start_at_login)
    {
        if let Ok(database) = storage.database.lock()
            && let Err(rollback_error) = database.update_settings(&previous)
        {
            log::error!("could not roll back settings after autostart failure: {rollback_error}");
        }
        return Err(error);
    }

    *monitor
        .fetcher
        .lock()
        .map_err(|_| "HTTP client lock is unavailable".to_owned())? = updated_fetcher;
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
    monitor.schedule_changed.notify_one();
    Ok(())
}

#[tauri::command]
fn clear_giveaway_data(
    app: AppHandle,
    storage: tauri::State<'_, StorageState>,
    monitor: tauri::State<'_, MonitorState>,
) -> Result<(), String> {
    let _guard = monitor
        .data_operation
        .try_acquire()
        .ok_or_else(|| "data operation is already running".to_owned())?;
    let mut database = storage
        .database
        .lock()
        .map_err(|_| "database lock is unavailable".to_owned())?;
    database
        .clear_giveaway_data()
        .map_err(|error| error.to_string())?;
    log::info!("saved giveaways and notification history cleared; next baseline will be silent");
    sync_attention(&app, &database)?;
    emit_history_changed(&app);
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
fn mark_all_notifications_read(
    app: AppHandle,
    storage: tauri::State<'_, StorageState>,
) -> Result<u64, String> {
    let database = storage
        .database
        .lock()
        .map_err(|_| "database lock is unavailable".to_owned())?;
    let changed = database
        .mark_all_notifications_read()
        .map_err(|error| error.to_string())?;
    sync_attention(&app, &database)?;
    emit_history_changed(&app);
    Ok(changed)
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

fn sync_autostart(app: &AppHandle, enabled: bool) -> Result<(), String> {
    if development_profile(&app.config().identifier, tauri::is_dev()) {
        log::debug!("autostart synchronization skipped for the development build");
        return Ok(());
    }

    let manager = app.autolaunch();
    if enabled {
        // Re-register even when an entry already exists so an imported WPF entry or a moved
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
    let database = storage
        .database
        .lock()
        .map_err(|_| "database lock is unavailable".to_owned())?;
    database
        .mark_notification_read(notification_id)
        .map_err(|error| error.to_string())?;
    sync_attention(app, &database)?;
    emit_history_changed(app);
    Ok(())
}

#[cfg(feature = "ui-tests")]
fn sync_attention(_app: &AppHandle, database: &Database) -> Result<(), String> {
    let unread = database
        .counts()
        .map_err(|e| e.to_string())?
        .unread_notifications;
    ui_tests::record(ui_tests::Event::Attention { unread })
}

#[cfg(not(feature = "ui-tests"))]
fn sync_attention(app: &AppHandle, database: &Database) -> Result<(), String> {
    let unread = database
        .counts()
        .map_err(|error| error.to_string())?
        .unread_notifications;
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
        let attention = (unread > 0).then_some(UserAttentionType::Informational);
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
    let source = match notification.source.as_str() {
        "giveeClub" | "GiveeClub" => "Givee Club",
        "gamesForFarm" | "GamesForFarm" => "Games for Farm",
        "zeros" | "Zeros" => "Zeros",
        value => value,
    };
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

    NotificationRequest {
        title: format!("{prefix}: {}", notification.title),
        body: format!(
            "{source_label}: {source}\n{link_label}: {}",
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

fn deliver_pending_notifications(app: &AppHandle, storage: &StorageState) -> Result<u64, String> {
    let report = dispatch_pending_notifications(&storage.database, |notification, settings| {
        let language = effective_language(settings);
        let request =
            notification_request(notification, language, settings.notification_sound_enabled);
        let notification_id = notification.id.clone();
        let activation_id = notification_id.clone();
        let activation_app = app.clone();
        let activation_url = format!("{DEEP_LINK_SCHEME}://notification/{notification_id}");
        notifications::show(app, &request, Some(&activation_url), move || {
            let storage = activation_app.state::<StorageState>();
            if let Err(error) =
                activate_notification(&activation_app, storage.inner(), &activation_id)
            {
                log::warn!("could not activate notification {activation_id}: {error}");
            }
        })
    })
    .map_err(|error| error.to_string())?;

    let database = storage
        .database
        .lock()
        .map_err(|_| "database lock is unavailable".to_owned())?;
    sync_attention(app, &database)?;
    if report.attempted > 0 {
        emit_history_changed(app);
    }
    Ok(report.delivered)
}

fn spawn_check(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let storage = app.state::<StorageState>();
        let monitor = app.state::<MonitorState>();
        if let Err(error) = perform_check(&app, storage.inner(), monitor.inner()).await {
            log::warn!("tray giveaway check did not complete: {error}");
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
async fn check_now(
    app: AppHandle,
    storage: tauri::State<'_, StorageState>,
    monitor: tauri::State<'_, MonitorState>,
) -> Result<MonitoringOutcome, String> {
    perform_check(&app, storage.inner(), monitor.inner()).await
}

async fn perform_check(
    app: &AppHandle,
    storage: &StorageState,
    monitor: &MonitorState,
) -> Result<MonitoringOutcome, String> {
    let Some(_guard) = monitor.data_operation.try_acquire() else {
        log::warn!("giveaway check skipped because a check or data cleanup is already running");
        return Err("data operation is already running".to_owned());
    };
    if let Some(error) = storage.migration_error.as_ref() {
        let result = Err(format!(
            "legacy data migration must be resolved first: {error}"
        ));
        log::error!("giveaway check blocked by legacy migration error: {error}");
        record_finished(monitor, &result);
        return result;
    }
    if storage
        .database
        .lock()
        .map_err(|_| "database lock is unavailable".to_owned())?
        .initial_sync_choice_required()
        .map_err(|error| error.to_string())?
    {
        return Err("initial synchronization mode must be selected first".to_owned());
    }
    if let Ok(mut status) = monitor.status.lock() {
        status.record_started(now_utc());
    }
    log::info!("giveaway check started");

    let result = perform_check_inner(storage, monitor).await;
    if result.is_ok()
        && let Err(error) = deliver_pending_notifications(app, storage)
    {
        log::error!("pending notification delivery could not complete: {error}");
    }
    for (level, message) in check_result_diagnostics(&result) {
        log::log!(level, "{message}");
    }
    record_finished(monitor, &result);
    result
}

async fn perform_check_inner(
    storage: &StorageState,
    monitor: &MonitorState,
) -> Result<MonitoringOutcome, String> {
    let settings = storage
        .database
        .lock()
        .map_err(|_| "database lock is unavailable".to_owned())?
        .settings()
        .map_err(|error| error.to_string())?;
    let fetcher = monitor
        .fetcher
        .lock()
        .map_err(|_| "HTTP client lock is unavailable".to_owned())?
        .clone();
    let source_result = check_sources(
        &fetcher,
        settings.givee_club_enabled,
        settings.games_for_farm_enabled,
        settings.zeros_enabled,
    )
    .await;
    let mut database = storage
        .database
        .lock()
        .map_err(|_| "database lock is unavailable".to_owned())?;
    apply_source_check(&mut database, source_result).map_err(|error| error.to_string())
}

fn record_finished(monitor: &MonitorState, result: &Result<MonitoringOutcome, String>) {
    if let Ok(mut status) = monitor.status.lock() {
        status.record_finished(now_utc(), result);
    }
}

fn now_utc() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .expect("RFC 3339 timestamp formatting should succeed")
}

async fn monitor_loop(app_handle: tauri::AppHandle) {
    loop {
        let initial_sync_choice_required = app_handle
            .state::<StorageState>()
            .database
            .lock()
            .ok()
            .and_then(|database| database.initial_sync_choice_required().ok())
            .unwrap_or(false);
        if initial_sync_choice_required {
            log::info!("waiting for the initial synchronization mode to be selected");
            app_handle
                .state::<MonitorState>()
                .schedule_changed
                .notified()
                .await;
            continue;
        }

        {
            let storage = app_handle.state::<StorageState>();
            let monitor = app_handle.state::<MonitorState>();
            let _ = perform_check(&app_handle, storage.inner(), monitor.inner()).await;
        }

        let interval_minutes = app_handle
            .state::<StorageState>()
            .database
            .lock()
            .ok()
            .and_then(|database| database.settings().ok())
            .map_or(15, |settings| settings.check_interval_minutes.max(5));
        let monitor = app_handle.state::<MonitorState>();
        log::info!("next giveaway check in {interval_minutes} minutes");
        tokio::select! {
            () = tokio::time::sleep(Duration::from_secs(u64::from(interval_minutes) * 60)) => {}
            () = monitor.schedule_changed.notified() => {
                log::info!("giveaway check schedule changed; recalculating interval");
            }
        }
    }
}

#[cfg(all(target_os = "windows", not(feature = "ui-tests")))]
fn legacy_data_directory() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|path| path.join("GiveawayScout"))
}

#[cfg(any(not(target_os = "windows"), feature = "ui-tests"))]
fn legacy_data_directory() -> Option<PathBuf> {
    None
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

    let log_builder = tauri_plugin_log::Builder::new()
        .level(log::LevelFilter::Debug)
        .max_file_size(2_000_000)
        .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepOne);
    #[cfg(feature = "ui-tests")]
    let log_builder = log_builder.targets([
        tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
        tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Folder {
            path: test_profile.root.join("logs"),
            file_name: Some("ui-tests".into()),
        }),
    ]);
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
        .plugin(log_builder.build())
        .on_window_event(|window, event| {
            if window.label() != "main" {
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
            #[cfg(feature = "ui-tests")]
            let new_profile = !data_directory.join("giveaway-scout.db").exists();
            let (mut database, database_recovery) =
                Database::open_with_recovery(data_directory.join("giveaway-scout.db"))?;
            #[cfg(feature = "ui-tests")]
            if new_profile { database.update_settings(&ui_tests::seed_settings())?; }
            if let Some(recovery) = database_recovery.as_ref() {
                log::error!(
                    "damaged SQLite database was preserved at {} before creating a clean database: {}",
                    recovery.backup_path,
                    recovery.original_error
                );
            }
            let (legacy_import, migration_error) = match legacy_data_directory() {
                Some(path) => match database.migrate_legacy_json(path) {
                    Ok(report) => (Some(report), None),
                    Err(error) => (None, Some(error.to_string())),
                },
                None => (None, None),
            };

            if let Some(report) = legacy_import {
                log::info!(
                    "legacy data import finished: settings={}, tracked={}, notifications={}, skipped={}",
                    report.settings_imported,
                    report.tracked_imported,
                    report.notifications_imported,
                    report.service_entries_skipped
                );
            }
            if let Some(error) = migration_error.as_ref() {
                log::error!("legacy data import failed: {error}");
            }

            let settings = database.settings()?;
            let initial_sync_choice_required = database.initial_sync_choice_required()?;
            apply_log_level(settings.verbose_logging);

            app.manage(StorageState {
                database: Mutex::new(database),
                database_recovery,
                legacy_import,
                migration_error,
            });
            app.manage(MonitorState {
                fetcher: Mutex::new(HttpPageFetcher::with_policy(
                    settings.http_timeout_seconds,
                    settings.http_retry_count,
                )?),
                data_operation: DataOperationGate::default(),
                status: Mutex::new(MonitorSnapshot::default()),
                schedule_changed: Notify::new(),
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
                && let Ok(database) = storage.database.lock()
                && let Err(error) = sync_attention(app.handle(), &database)
            {
                log::warn!("could not restore unread notification attention: {error}");
            }

            if let Err(error) = sync_autostart(app.handle(), settings.start_at_login) {
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

            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(monitor_loop(app_handle));
            usage_statistics::start(app.handle());
            log::info!("Giveaway Scout backend initialized");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_app_state,
            get_storage_snapshot,
            update_settings,
            clear_giveaway_data,
            get_notification_history,
            mark_all_notifications_read,
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
    application.run(|_, _| {});
}

#[cfg(test)]
mod tests {
    use giveaway_scout_core::{GiveawaySource, MonitoringOutcome, SourceFailure};

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
    fn successful_check_updates_last_successful_timestamp() {
        let mut status = MonitorSnapshot::default();
        status.record_started("2026-10-10T10:00:00Z".to_owned());
        status.record_finished(
            "2026-10-10T10:00:01Z".to_owned(),
            &Ok(MonitoringOutcome {
                check_performed: true,
                ..MonitoringOutcome::default()
            }),
        );

        assert!(!status.is_checking);
        assert_eq!(
            status.last_successful_check_utc.as_deref(),
            Some("2026-10-10T10:00:01Z")
        );
        assert!(status.last_error.is_none());
    }

    #[test]
    fn failed_check_preserves_previous_success_and_records_error() {
        let mut status = MonitorSnapshot {
            last_successful_check_utc: Some("2026-10-10T09:00:00Z".to_owned()),
            ..MonitorSnapshot::default()
        };
        status.record_started("2026-10-10T10:00:00Z".to_owned());
        status.record_finished(
            "2026-10-10T10:00:01Z".to_owned(),
            &Err("network unavailable".to_owned()),
        );

        assert_eq!(
            status.last_successful_check_utc.as_deref(),
            Some("2026-10-10T09:00:00Z")
        );
        assert_eq!(status.last_error.as_deref(), Some("network unavailable"));
    }

    #[test]
    fn partial_source_errors_keep_the_source_name_and_previous_success() {
        let mut status = MonitorSnapshot {
            last_successful_check_utc: Some("2026-10-19T10:00:00Z".to_owned()),
            ..MonitorSnapshot::default()
        };
        status.record_finished(
            "2026-10-19T11:00:00Z".to_owned(),
            &Ok(MonitoringOutcome {
                check_performed: true,
                failures: vec![SourceFailure {
                    source: GiveawaySource::GamesForFarm,
                    message: "request timed out".to_owned(),
                    reason: giveaway_scout_core::SourceFailureReason::Timeout,
                }],
                ..MonitoringOutcome::default()
            }),
        );
        assert_eq!(
            status.last_error.as_deref(),
            Some("Games for Farm: request timed out")
        );
        assert_eq!(status.last_failure_count, 1);
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["sourceFailures"][0]["reason"]["code"], "timeout");
        assert_eq!(json["sourceFailures"][0]["source"], "gamesForFarm");
        assert_eq!(json["sourceFailures"][0]["message"], "request timed out");
        assert_eq!(
            status.last_successful_check_utc.as_deref(),
            Some("2026-10-19T10:00:00Z")
        );
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
