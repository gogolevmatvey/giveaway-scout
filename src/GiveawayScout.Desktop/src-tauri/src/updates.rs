//! Application-owned updater API: no arbitrary URLs/keys or install commands in JS.
#[cfg(target_os = "linux")]
use std::path::Path;
use std::{sync::Mutex, time::Duration};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Serialize;
use tauri::{AppHandle, Manager, utils::config::BundleType};
use tauri_plugin_updater::{Update, UpdaterExt};
use tokio::sync::watch;

use crate::notifications::NotificationRequest;
use crate::operations::{DataOperationGate, DataOperationGuard};

const MAX_PACKAGE_BYTES: usize = giveaway_scout_updater::MAX_PACKAGE_BYTES;
const MAX_NOTES_CHARS: usize = giveaway_scout_updater::MAX_NOTES_CHARS;
const AUTOMATIC_CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Mode {
    Enabled,
    Development,
    Manual,
    PackageManager,
    NotConfigured,
    InvalidConfiguration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Phase {
    Idle,
    Checking,
    Available,
    Downloading,
    Installing,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Candidate {
    token: String,
    version: String,
    notes: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    mode: Mode,
    phase: Phase,
    current_version: String,
    candidate: Option<Candidate>,
    banner_dismissed: bool,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    last_checked_at_utc: Option<String>,
    last_error: Option<String>,
}

struct Model {
    snapshot: Snapshot,
    pending: Option<Update>,
}

pub(crate) struct Service {
    model: Mutex<Model>,
    operation: DataOperationGate,
    automatic: watch::Sender<AutomaticPolicy>,
    // Switching the policy waits for an in-progress announcement, so an old
    // automatic check cannot deliver a toast after the setting has been applied.
    automatic_delivery: Mutex<()>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AutomaticPolicy {
    enabled: bool,
    generation: u64,
    stopped: bool,
}

impl Service {
    fn new(mode: Mode, version: String) -> Self {
        Self {
            model: Mutex::new(Model {
                snapshot: Snapshot {
                    mode,
                    phase: Phase::Idle,
                    current_version: version,
                    candidate: None,
                    banner_dismissed: false,
                    downloaded_bytes: 0,
                    total_bytes: None,
                    last_checked_at_utc: None,
                    last_error: None,
                },
                pending: None,
            }),
            operation: DataOperationGate::default(),
            automatic: watch::channel(AutomaticPolicy {
                enabled: false,
                generation: 0,
                stopped: false,
            })
            .0,
            automatic_delivery: Mutex::new(()),
        }
    }

    fn view(&self) -> Snapshot {
        self.model
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .snapshot
            .clone()
    }

    fn edit(&self, edit: impl FnOnce(&mut Model)) {
        edit(&mut self.model.lock().unwrap_or_else(|error| error.into_inner()));
    }

    fn begin(&self) -> Result<Operation<'_>, String> {
        if self.view().mode != Mode::Enabled {
            return Err("UPDATES_UNAVAILABLE".to_owned());
        }
        let guard = self
            .operation
            .try_acquire()
            .ok_or_else(|| "UPDATES_BUSY".to_owned())?;
        Ok(Operation {
            service: self,
            _guard: guard,
            cancel_check_cleanly: false,
        })
    }

    fn automatic_is_current(&self, policy: AutomaticPolicy) -> bool {
        policy.enabled && !policy.stopped && *self.automatic.borrow() == policy
    }

    fn with_automatic_delivery<T>(
        &self,
        policy: Option<AutomaticPolicy>,
        deliver: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let _delivery = policy.map(|_| {
            self.automatic_delivery
                .lock()
                .unwrap_or_else(|error| error.into_inner())
        });
        if policy.is_some_and(|policy| !self.automatic_is_current(policy)) {
            return Err("UPDATES_AUTOMATIC_CANCELLED".to_owned());
        }
        deliver()
    }

    fn set_automatic_checks(&self, enabled: bool) {
        let _delivery = self
            .automatic_delivery
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.automatic.send_if_modified(|policy| {
            if policy.stopped || policy.enabled == enabled {
                return false;
            }
            policy.enabled = enabled;
            policy.generation = policy.generation.wrapping_add(1);
            true
        });
    }

    fn stop_automatic_checks(&self) {
        let _delivery = self
            .automatic_delivery
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.automatic.send_if_modified(|policy| {
            if policy.stopped {
                return false;
            }
            policy.enabled = false;
            policy.stopped = true;
            policy.generation = policy.generation.wrapping_add(1);
            true
        });
    }
}

struct Operation<'a> {
    service: &'a Service,
    _guard: DataOperationGuard<'a>,
    cancel_check_cleanly: bool,
}
impl Drop for Operation<'_> {
    fn drop(&mut self) {
        self.service.edit(|model| {
            if matches!(
                model.snapshot.phase,
                Phase::Checking | Phase::Downloading | Phase::Installing
            ) {
                if self.cancel_check_cleanly && model.snapshot.phase == Phase::Checking {
                    model.snapshot.phase = if model.snapshot.candidate.is_some() {
                        Phase::Available
                    } else {
                        Phase::Idle
                    };
                    model.snapshot.last_error = None;
                } else {
                    model.snapshot.phase = Phase::Failed;
                    model.snapshot.last_error = Some("UPDATES_INTERRUPTED".to_owned());
                }
            }
        });
    }
}

fn secure_url(url: &url::Url) -> bool {
    giveaway_scout_updater::secure_url(url)
}

fn validate_config(value: &serde_json::Value) -> Result<(), String> {
    let config: tauri_plugin_updater::Config = serde_json::from_value(value.clone())
        .map_err(|_| "invalid updater configuration".to_owned())?;
    if config.endpoints.is_empty()
        || config.endpoints.iter().any(|url| !secure_url(url))
        || config.dangerous_insecure_transport_protocol
        || config.dangerous_accept_invalid_certs
        || config.dangerous_accept_invalid_hostnames
        || config.allow_downgrades
        || !config.require_signed_version
    {
        return Err("updater requires HTTPS, signed versions and no unsafe overrides".to_owned());
    }
    let decoded = STANDARD
        .decode(&config.pubkey)
        .map_err(|_| "invalid update public key".to_owned())?;
    let key = std::str::from_utf8(&decoded).map_err(|_| "invalid update public key".to_owned())?;
    minisign_verify::PublicKey::decode(key).map_err(|_| "invalid update public key".to_owned())?;
    Ok(())
}

fn distribution_mode(
    development: bool,
    bundle: Option<BundleType>,
    installed_identity: bool,
) -> Mode {
    if development {
        return Mode::Development;
    }
    match bundle {
        Some(BundleType::Deb | BundleType::Rpm) if installed_identity => Mode::PackageManager,
        Some(BundleType::AppImage) if installed_identity => Mode::Enabled,
        Some(BundleType::App | BundleType::Dmg | BundleType::Nsis | BundleType::Msi)
            if installed_identity =>
        {
            Mode::Enabled
        }
        _ => Mode::Manual,
    }
}

#[cfg(target_os = "windows")]
fn installed_identity(app: &AppHandle) -> bool {
    crate::windows_installation::is_current_installation(app)
}

#[cfg(target_os = "macos")]
fn installed_identity(_app: &AppHandle) -> bool {
    std::env::current_exe().ok().is_some_and(|path| {
        path.ancestors()
            .any(|parent| parent.extension().is_some_and(|ext| ext == "app"))
    })
}

#[cfg(target_os = "linux")]
fn installed_identity(app: &AppHandle) -> bool {
    match tauri::utils::platform::bundle_type() {
        Some(BundleType::AppImage) => app
            .env()
            .appimage
            .as_deref()
            .is_some_and(|path| Path::new(path).is_file()),
        Some(BundleType::Deb | BundleType::Rpm) => std::env::current_exe()
            .and_then(std::fs::canonicalize)
            .ok()
            .zip(std::fs::canonicalize("/usr/bin/giveaway-scout").ok())
            .is_some_and(|(current, installed)| current == installed),
        _ => false,
    }
}

pub(crate) fn initialize(app: &AppHandle) -> Result<(), String> {
    let mut mode = distribution_mode(
        crate::development_profile(&app.config().identifier, tauri::is_dev()),
        tauri::utils::platform::bundle_type(),
        installed_identity(app),
    );
    if mode == Mode::Enabled {
        mode = match app.config().plugins.0.get("updater") {
            None => Mode::NotConfigured,
            Some(value) => match validate_config(value) {
                Ok(()) => Mode::Enabled,
                Err(error) => {
                    log::warn!("automatic updates disabled: {error}");
                    Mode::InvalidConfiguration
                }
            },
        };
    }
    if mode == Mode::Enabled {
        app.plugin(tauri_plugin_updater::Builder::new().build())
            .map_err(|error| error.to_string())?;
    }
    app.manage(Service::new(mode, app.package_info().version.to_string()));
    if mode == Mode::Enabled {
        let enabled = app
            .state::<crate::StorageState>()
            .database
            .lock()
            .map_err(|_| "database lock unavailable".to_owned())?
            .settings()
            .map_err(|error| error.to_string())?
            .automatic_update_checks;
        app.state::<Service>().set_automatic_checks(enabled);
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            run_automatic_checks(handle).await;
        });
    }
    Ok(())
}

/// Apply only after a successful settings write, outside the profile mutex.
pub(crate) fn apply_automatic_checks(app: &AppHandle, enabled: bool) {
    if let Some(service) = app.try_state::<Service>() {
        service.set_automatic_checks(enabled);
    }
}

pub(crate) fn shutdown(app: &AppHandle) {
    if let Some(service) = app.try_state::<Service>() {
        service.stop_automatic_checks();
    }
}

// Policy changes drop only the worker's own future. Manual checks and package
// installation never run through this cancellation boundary.
async fn automatic_step<T>(
    receiver: &mut watch::Receiver<AutomaticPolicy>,
    policy: AutomaticPolicy,
    step: impl std::future::Future<Output = T>,
) -> Option<T> {
    if *receiver.borrow_and_update() != policy {
        return None;
    }
    tokio::select! {
        biased;
        _ = receiver.changed() => None,
        result = step => Some(result),
    }
}

async fn run_automatic_checks(app: AppHandle) {
    let service = app.state::<Service>();
    let mut receiver = service.automatic.subscribe();
    loop {
        let policy = *receiver.borrow_and_update();
        if policy.stopped {
            break;
        }
        if !policy.enabled {
            if receiver.changed().await.is_err() {
                break;
            }
            continue;
        }
        let interval = match automatic_step(
            &mut receiver,
            policy,
            check_updates(app.clone(), Some(policy)),
        )
        .await
        {
            // A busy manual operation did not perform an automatic check. Retry
            // soon after it instead of postponing the newly enabled check a day.
            Some(Err(error)) if error == "UPDATES_BUSY" => Duration::from_secs(60),
            Some(Err(error)) if error != "UPDATES_AUTOMATIC_CANCELLED" => {
                log::warn!("automatic update check failed: {error}");
                AUTOMATIC_CHECK_INTERVAL
            }
            None => continue,
            Some(_) => AUTOMATIC_CHECK_INTERVAL,
        };
        let _ = automatic_step(&mut receiver, policy, tokio::time::sleep(interval)).await;
    }
}

#[tauri::command]
pub(crate) fn get_update_state(app: AppHandle, service: tauri::State<'_, Service>) -> Snapshot {
    // Display-only fixture in the validated, isolated UI-test profile. The real
    // service mode and pending package remain unchanged, so it cannot install.
    #[cfg(feature = "ui-tests")]
    if let Some(snapshot) = banner_fixture(&app, &service) {
        return snapshot;
    }
    #[cfg(not(feature = "ui-tests"))]
    let _ = app;
    service.view()
}

#[cfg(feature = "ui-tests")]
fn banner_fixture(app: &AppHandle, service: &Service) -> Option<Snapshot> {
    if !crate::ui_tests::profile()
        .root
        .join("update-banner-fixture")
        .is_file()
    {
        return None;
    }
    let mut snapshot = service.view();
    snapshot.mode = Mode::Enabled;
    snapshot.phase = Phase::Available;
    snapshot.candidate = Some(Candidate {
        token: "layout-only-no-install".into(),
        version: "2.0.0".into(),
        notes: "Layout fixture".into(),
    });
    snapshot.banner_dismissed = app
        .state::<crate::StorageState>()
        .database
        .lock()
        .ok()
        .and_then(|database| database.update_banner_was_dismissed("2.0.0").ok())
        .unwrap_or(false);
    Some(snapshot)
}

fn client_policy(client: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    giveaway_scout_updater::client_policy(client)
}

#[tauri::command]
pub(crate) async fn check_for_updates(app: AppHandle) -> Result<Snapshot, String> {
    check_updates(app, None).await
}

async fn check_updates(
    app: AppHandle,
    automatic: Option<AutomaticPolicy>,
) -> Result<Snapshot, String> {
    let service = app
        .try_state::<Service>()
        .ok_or_else(|| "UPDATES_UNAVAILABLE".to_owned())?;
    if automatic.is_some_and(|policy| !service.automatic_is_current(policy)) {
        return Err("UPDATES_AUTOMATIC_CANCELLED".to_owned());
    }
    let mut operation = service.begin()?;
    operation.cancel_check_cleanly = automatic.is_some();
    service.edit(|model| {
        model.snapshot.phase = Phase::Checking;
        model.snapshot.last_error = None;
    });
    let result = async {
        app.updater_builder()
            .timeout(Duration::from_secs(20))
            .configure_client(client_policy)
            .build()
            .map_err(|error| error.to_string())?
            .check()
            .await
            .map_err(|error| error.to_string())
    }
    .await;
    let result = result.and_then(|update| {
        if let Some(update) = &update
            && (!secure_url(&update.download_url) || update.signature.trim().is_empty())
        {
            return Err("unsafe update download or missing signature".to_owned());
        }
        Ok(update)
    });
    // Network awaits are complete before either delivery or profile locks are
    // taken. The policy and result now commit together with announcement delivery.
    service.with_automatic_delivery(automatic, || {
        match result {
            Ok(update) => service.edit(|model| {
                // Use the same model -> database lock order as dismissal. A check
                // must not overwrite a dismissal saved concurrently for this version.
                model.snapshot.banner_dismissed = update.as_ref().is_some_and(|update| {
                    match app.state::<crate::StorageState>().database.lock() {
                        Ok(database) => match database.update_banner_was_dismissed(&update.version)
                        {
                            Ok(dismissed) => dismissed,
                            Err(error) => {
                                log::warn!("could not read update banner dismissal: {error}");
                                false
                            }
                        },
                        Err(_) => {
                            log::warn!(
                                "could not read update banner dismissal: database lock unavailable"
                            );
                            false
                        }
                    }
                });
                model.snapshot.last_checked_at_utc = Some(crate::now_utc());
                model.snapshot.candidate = update.as_ref().map(|update| Candidate {
                    token: uuid::Uuid::new_v4().to_string(),
                    version: update.version.clone(),
                    notes: update
                        .body
                        .as_deref()
                        .unwrap_or_default()
                        .chars()
                        .take(MAX_NOTES_CHARS)
                        .collect(),
                });
                model.snapshot.phase = if update.is_some() {
                    Phase::Available
                } else {
                    Phase::Idle
                };
                model.pending = update;
            }),
            Err(error) => {
                log::warn!("could not check for updates: {error}");
                service.edit(|model| {
                    model.snapshot.phase = Phase::Failed;
                    model.snapshot.last_error = Some("UPDATES_CHECK_FAILED".to_owned());
                });
                return Err("UPDATES_CHECK_FAILED".to_owned());
            }
        }
        let snapshot = service.view();
        if let Some(candidate) = &snapshot.candidate
            && let Err(error) =
                notify_available_update(&app, &candidate.version, automatic.is_some())
        {
            // A notification failure must not hide a verified update in the UI.
            log::warn!("could not announce update {}: {error}", candidate.version);
        }
        Ok(snapshot)
    })
}

fn dismiss_candidate(
    model: &mut Model,
    token: &str,
    persist: impl FnOnce(&str) -> Result<(), String>,
) -> Result<Snapshot, String> {
    let candidate = model
        .snapshot
        .candidate
        .as_ref()
        .filter(|candidate| candidate.token == token)
        .ok_or_else(|| "UPDATES_STALE_CANDIDATE".to_owned())?;
    persist(&candidate.version)?;
    model.snapshot.banner_dismissed = true;
    Ok(model.snapshot.clone())
}

#[tauri::command]
pub(crate) fn dismiss_update(app: AppHandle, token: String) -> Result<Snapshot, String> {
    let service = app
        .try_state::<Service>()
        .ok_or_else(|| "UPDATES_UNAVAILABLE".to_owned())?;
    #[cfg(feature = "ui-tests")]
    if let Some(snapshot) = banner_fixture(&app, &service) {
        let storage = app.state::<crate::StorageState>();
        let database = storage
            .database
            .lock()
            .map_err(|_| "database lock unavailable".to_owned())?;
        // The fixture is display-only and isolated; its token can never install
        // a package or replace the real candidate retained by the service.
        return dismiss_candidate(
            &mut Model {
                snapshot,
                pending: None,
            },
            &token,
            |version| {
                database
                    .dismiss_update_banner(version)
                    .map_err(|error| error.to_string())
            },
        );
    }
    // Keep the candidate stable until its version is persisted. A failed write
    // leaves the banner visible, and an obsolete UI token never writes metadata.
    let mut model = service
        .model
        .lock()
        .map_err(|_| "update state unavailable".to_owned())?;
    let storage = app.state::<crate::StorageState>();
    let database = storage
        .database
        .lock()
        .map_err(|_| "database lock unavailable".to_owned())?;
    dismiss_candidate(&mut model, &token, |version| {
        database
            .dismiss_update_banner(version)
            .map_err(|error| error.to_string())
    })
}

fn announce_update_once(
    database: &giveaway_scout_core::Database,
    version: &str,
    show: impl FnOnce() -> Result<(), String>,
) -> Result<bool, String> {
    if database
        .update_was_announced(version)
        .map_err(|error| error.to_string())?
    {
        return Ok(false);
    }
    show()?;
    database
        .mark_update_announced(version)
        .map_err(|error| error.to_string())?;
    Ok(true)
}

fn update_notification_request(
    version: &str,
    language: &str,
    play_sound: bool,
) -> NotificationRequest {
    if language == "ru" {
        NotificationRequest {
            title: format!("Доступно обновление Giveaway Scout {version}"),
            body: "Откройте приложение и нажмите «Обновить», чтобы установить новую версию."
                .to_owned(),
            action_label: "Открыть".to_owned(),
            play_sound,
        }
    } else {
        NotificationRequest {
            title: format!("Giveaway Scout {version} update available"),
            body: "Open the app and click Update to install the new version.".to_owned(),
            action_label: "Open".to_owned(),
            play_sound,
        }
    }
}

fn notify_available_update(app: &AppHandle, version: &str, automatic: bool) -> Result<(), String> {
    let storage = app.state::<crate::StorageState>();
    let database = storage
        .database
        .lock()
        .map_err(|_| "database lock unavailable".to_owned())?;
    let settings = database.settings().map_err(|error| error.to_string())?;
    // The write may have committed just before its worker policy hook runs.
    if automatic && !settings.automatic_update_checks {
        return Ok(());
    }
    let request = update_notification_request(
        version,
        crate::effective_language(&settings),
        settings.notification_sound_enabled,
    );
    let activation_app = app.clone();
    announce_update_once(&database, version, || {
        crate::notifications::show(app, &request, Some("giveaway-scout://show"), move || {
            if let Err(error) = crate::show_main_window(&activation_app) {
                log::warn!("could not open available update: {error}");
            }
        })
    })?;
    Ok(())
}

fn require_consent(
    candidate: Option<&Candidate>,
    token: &str,
    confirmed: bool,
) -> Result<(), String> {
    if !confirmed {
        return Err("UPDATES_CONFIRMATION_REQUIRED".to_owned());
    }
    if !candidate.is_some_and(|candidate| candidate.token == token) {
        return Err("UPDATES_STALE_CANDIDATE".to_owned());
    }
    Ok(())
}

async fn download(update: &Update, service: &Service) -> Result<Vec<u8>, String> {
    let mut response = client_policy(reqwest::Client::builder())
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|error| error.to_string())?
        .get(update.download_url.clone())
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?;
    let total = response.content_length();
    if total.is_some_and(|total| total > MAX_PACKAGE_BYTES as u64) {
        return Err("update package exceeds the size limit".to_owned());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
        if bytes.len().saturating_add(chunk.len()) > MAX_PACKAGE_BYTES {
            return Err("update package exceeds the size limit".to_owned());
        }
        bytes.extend_from_slice(&chunk);
        service.edit(|model| {
            model.snapshot.downloaded_bytes = bytes.len() as u64;
            model.snapshot.total_bytes = total;
        });
    }
    update.verify(&bytes).map_err(|error| error.to_string())?;
    Ok(bytes)
}

#[tauri::command]
pub(crate) async fn install_update(
    app: AppHandle,
    token: String,
    confirmed: bool,
) -> Result<(), String> {
    let service = app
        .try_state::<Service>()
        .ok_or_else(|| "UPDATES_UNAVAILABLE".to_owned())?;
    let _operation = service.begin()?;
    let monitor = app.state::<crate::MonitorState>();
    let _data = monitor
        .data_operation
        .try_acquire()
        .ok_or_else(|| "UPDATES_DATA_BUSY".to_owned())?;
    let update = {
        let model = service
            .model
            .lock()
            .map_err(|_| "update state unavailable".to_owned())?;
        require_consent(model.snapshot.candidate.as_ref(), &token, confirmed)?;
        model
            .pending
            .clone()
            .ok_or_else(|| "UPDATES_STALE_CANDIDATE".to_owned())?
    };
    service.edit(|model| {
        model.snapshot.phase = Phase::Downloading;
        model.snapshot.last_error = None;
        model.snapshot.downloaded_bytes = 0;
        model.snapshot.total_bytes = None;
    });
    let result = async {
        let bytes = download(&update, &service).await?;
        service.edit(|model| model.snapshot.phase = Phase::Installing);
        let storage = app.state::<crate::StorageState>();
        if !storage
            .database
            .lock()
            .map_err(|_| "database lock unavailable".to_owned())?
            .checkpoint_before_update()
            .map_err(|error| error.to_string())?
        {
            return Err("database checkpoint is busy".to_owned());
        }
        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        {
            // Spawn before stopping the runtime: an OS launch error must leave
            // monitoring usable. The setup waits for this exact installed PID;
            // the application's ExitRequested hook drains the shared runtime.
            let public_key = app
                .config()
                .plugins
                .0
                .get("updater")
                .and_then(|value| value.get("pubkey"))
                .and_then(|value| value.as_str())
                .ok_or_else(|| "missing update public key".to_owned())?;
            let filename = format!("Giveaway Scout_{}_x64-setup.exe", update.version);
            let package = giveaway_scout_updater::stage_installer(
                public_key,
                &update.signature,
                &update.version,
                &filename,
                &bytes,
            )?;
            package.launch(std::process::id(), true)?;
            app.exit(0);
        }
        #[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
        {
            // Other platforms retain Tauri's native package installation.
            tauri::async_runtime::spawn_blocking(move || {
                update.install(bytes).map_err(|error| error.to_string())
            })
            .await
            .map_err(|error| error.to_string())??;
        }
        Ok::<(), String>(())
    }
    .await;
    if let Err(error) = result {
        log::warn!("update installation rejected or failed: {error}");
        service.edit(|model| {
            model.snapshot.phase = Phase::Failed;
            model.snapshot.last_error = Some("UPDATES_INSTALL_FAILED".to_owned());
        });
        return Err("UPDATES_INSTALL_FAILED".to_owned());
    }
    #[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
    {
        monitor.runtime.shutdown().await;
        app.restart();
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate_service() -> Service {
        let service = Service::new(Mode::Enabled, "1.1.0".to_owned());
        service.edit(|model| {
            model.snapshot.phase = Phase::Available;
            model.snapshot.candidate = Some(Candidate {
                token: "current-token".to_owned(),
                version: "1.2.0".to_owned(),
                notes: String::new(),
            });
        });
        service
    }

    #[tokio::test]
    async fn disabling_automatic_checks_cancels_the_owned_future_and_keeps_the_candidate() {
        let service = candidate_service();
        service.edit(|model| model.snapshot.banner_dismissed = true);
        service.set_automatic_checks(true);
        let policy = *service.automatic.borrow();
        let mut receiver = service.automatic.subscribe();
        let (started, ready) = tokio::sync::oneshot::channel();
        let check = async {
            let mut operation = service.begin().unwrap();
            operation.cancel_check_cleanly = true;
            service.edit(|model| model.snapshot.phase = Phase::Checking);
            started.send(()).unwrap();
            std::future::pending::<()>().await;
        };
        let (result, ()) = tokio::join!(automatic_step(&mut receiver, policy, check), async {
            ready.await.unwrap();
            service.set_automatic_checks(false);
        });
        assert!(result.is_none());
        let snapshot = service.view();
        assert_eq!(snapshot.phase, Phase::Available);
        assert_eq!(snapshot.candidate.unwrap().token, "current-token");
        assert!(snapshot.banner_dismissed);
        assert!(snapshot.last_error.is_none());
        assert!(service.begin().is_ok());
    }

    #[test]
    fn automatic_policy_changes_do_not_cancel_manual_checks_or_installation() {
        for phase in [Phase::Checking, Phase::Downloading, Phase::Installing] {
            let service = candidate_service();
            service.set_automatic_checks(true);
            let operation = service.begin().unwrap();
            service.edit(|model| model.snapshot.phase = phase);
            service.set_automatic_checks(false);
            assert_eq!(service.view().phase, phase);
            assert!(service.view().last_error.is_none());
            assert!(service.begin().is_err());
            service.edit(|model| model.snapshot.phase = Phase::Available);
            drop(operation);
            assert!(service.begin().is_ok());
        }
    }

    #[test]
    fn a_completed_automatic_response_cannot_commit_or_notify_after_off() {
        let service = candidate_service();
        service.set_automatic_checks(true);
        let old_policy = *service.automatic.borrow();
        service.set_automatic_checks(false);
        assert_eq!(
            service
                .with_automatic_delivery::<()>(Some(old_policy), || {
                    panic!("cancelled response reached commit or notification delivery")
                })
                .unwrap_err(),
            "UPDATES_AUTOMATIC_CANCELLED"
        );
        service.set_automatic_checks(true);
        assert!(
            service
                .with_automatic_delivery::<()>(Some(old_policy), || {
                    panic!("reenabling accepted an older response")
                })
                .is_err()
        );
        assert_eq!(
            service
                .with_automatic_delivery(None, || Ok("manual result"))
                .unwrap(),
            "manual result"
        );
    }

    #[tokio::test]
    async fn disabling_cancels_the_timer_and_reenabling_invalidates_old_results() {
        let service = Service::new(Mode::Enabled, "1.1.0".to_owned());
        service.set_automatic_checks(true);
        let policy = *service.automatic.borrow();
        let mut receiver = service.automatic.subscribe();
        let (timer, ()) = tokio::join!(
            automatic_step(
                &mut receiver,
                policy,
                tokio::time::sleep(AUTOMATIC_CHECK_INTERVAL)
            ),
            async {
                tokio::task::yield_now().await;
                service.set_automatic_checks(false);
            }
        );
        assert!(timer.is_none());
        service.set_automatic_checks(true);
        assert!(!service.automatic_is_current(policy));
        let new_policy = *service.automatic.borrow();
        assert!(service.automatic_is_current(new_policy));
        assert_eq!(
            automatic_step(&mut receiver, new_policy, async { "checked immediately" }).await,
            Some("checked immediately")
        );
        service.stop_automatic_checks();
        service.set_automatic_checks(true);
        assert!(service.automatic.borrow().stopped);
        assert!(!service.automatic_is_current(new_policy));
    }

    #[test]
    fn dismissal_rejects_stale_tokens_before_writing_and_keeps_the_candidate() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("updates.db");
        let database = giveaway_scout_core::Database::open(&path).unwrap();
        let service = candidate_service();
        let mut model = service.model.lock().unwrap();
        assert!(
            dismiss_candidate(&mut model, "obsolete-token", |_| panic!(
                "stale token wrote metadata"
            ))
            .is_err()
        );
        assert!(!model.snapshot.banner_dismissed);
        assert!(!database.update_banner_was_dismissed("1.2.0").unwrap());
        let snapshot = dismiss_candidate(&mut model, "current-token", |version| {
            database
                .dismiss_update_banner(version)
                .map_err(|error| error.to_string())
        })
        .unwrap();
        assert!(snapshot.banner_dismissed);
        assert_eq!(snapshot.candidate.unwrap().token, "current-token");
        assert_eq!(snapshot.phase, Phase::Available);
        drop(database);
        let database = giveaway_scout_core::Database::open(&path).unwrap();
        assert!(database.update_banner_was_dismissed("1.2.0").unwrap());
        assert!(!database.update_banner_was_dismissed("1.3.0").unwrap());
        assert!(!database.update_was_announced("1.2.0").unwrap());
    }

    #[test]
    fn failed_dismissal_persistence_leaves_the_banner_visible() {
        let service = candidate_service();
        let mut model = service.model.lock().unwrap();
        assert_eq!(
            dismiss_candidate(&mut model, "current-token", |_| Err(
                "database unavailable".into()
            ))
            .unwrap_err(),
            "database unavailable"
        );
        assert!(!model.snapshot.banner_dismissed);
        assert_eq!(
            model.snapshot.candidate.as_ref().unwrap().token,
            "current-token"
        );
        assert_eq!(model.snapshot.phase, Phase::Available);
    }

    #[test]
    fn update_announcements_retry_failures_but_do_not_repeat_successful_versions() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("updates.db");
        let database = giveaway_scout_core::Database::open(&path).unwrap();
        assert!(announce_update_once(&database, "1.2.0", || Err("unavailable".into())).is_err());
        assert!(!database.update_was_announced("1.2.0").unwrap());
        assert!(announce_update_once(&database, "1.2.0", || Ok(())).unwrap());
        drop(database);
        let database = giveaway_scout_core::Database::open(&path).unwrap();
        assert!(
            !announce_update_once(&database, "1.2.0", || panic!("repeated notification")).unwrap()
        );
        assert!(announce_update_once(&database, "1.3.0", || Ok(())).unwrap());
        assert!(
            !announce_update_once(&database, "1.2.0", || panic!("old version repeated")).unwrap()
        );
    }

    #[test]
    fn update_notifications_are_localized_and_respect_sound_settings() {
        let russian = update_notification_request("1.2.0", "ru", false);
        assert_eq!(russian.title, "Доступно обновление Giveaway Scout 1.2.0");
        assert!(russian.body.contains("«Обновить»"));
        assert_eq!(russian.action_label, "Открыть");
        assert!(!russian.play_sound);
        let english = update_notification_request("1.2.0", "en", true);
        assert_eq!(english.title, "Giveaway Scout 1.2.0 update available");
        assert!(english.body.contains("Update"));
        assert_eq!(english.action_label, "Open");
        assert!(english.play_sound);
    }

    #[tokio::test]
    async fn real_http_client_rejects_plaintext_before_connecting() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/never-requested", listener.local_addr().unwrap());
        let client = client_policy(reqwest::Client::builder()).build().unwrap();
        assert!(client.get(url).send().await.is_err());
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    fn test_public_key() -> String {
        // Test-only, one-time keys remain in memory. Never configure them for a release.
        let keys = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        STANDARD.encode(keys.pk.to_box().unwrap().into_string())
    }

    #[test]
    fn valid_configuration_cannot_enable_insecure_transport_or_downgrades() {
        let key = test_public_key();
        let config = serde_json::json!({"pubkey":key,"endpoints":["https://example.org/latest.json"],"requireSignedVersion":true});
        validate_config(&config).unwrap();
        for field in [
            "dangerousInsecureTransportProtocol",
            "dangerousAcceptInvalidCerts",
            "dangerousAcceptInvalidHostnames",
            "allowDowngrades",
        ] {
            let mut unsafe_config = config.clone();
            unsafe_config[field] = serde_json::json!(true);
            assert!(validate_config(&unsafe_config).is_err(), "{field}");
        }
        let mut unsigned_version = config;
        unsigned_version["requireSignedVersion"] = serde_json::json!(false);
        assert!(validate_config(&unsigned_version).is_err());
    }

    #[test]
    fn distribution_rules_never_self_install_deb_portable_or_dev() {
        assert_eq!(
            distribution_mode(true, Some(BundleType::Nsis), true),
            Mode::Development
        );
        for bundle in [BundleType::Deb, BundleType::Rpm] {
            assert_eq!(
                distribution_mode(false, Some(bundle), true),
                Mode::PackageManager
            );
        }
        for bundle in [None, Some(BundleType::Nsis), Some(BundleType::App)] {
            assert_eq!(distribution_mode(false, bundle, false), Mode::Manual);
        }
        assert_eq!(
            distribution_mode(false, Some(BundleType::Deb), false),
            Mode::Manual
        );
        for bundle in [BundleType::Nsis, BundleType::App, BundleType::AppImage] {
            assert_eq!(distribution_mode(false, Some(bundle), true), Mode::Enabled);
        }
    }

    #[test]
    fn unsafe_urls_and_configuration_are_rejected_before_network_access() {
        for value in [
            "http://example.org/update",
            "file:///tmp/update",
            "https://user:secret@example.org/update",
            "https://example.org/update#fragment",
        ] {
            assert!(!secure_url(&url::Url::parse(value).unwrap()));
        }
        assert!(secure_url(
            &url::Url::parse("https://example.org/latest.json").unwrap()
        ));
        assert!(validate_config(&serde_json::json!({"endpoints": ["https://example.org/latest.json"], "pubkey":"placeholder", "requireSignedVersion":true})).is_err());
    }

    #[test]
    fn confirmation_is_bound_to_the_current_candidate() {
        let candidate = Candidate {
            token: "current-token".to_owned(),
            version: "1.0.0".to_owned(),
            notes: String::new(),
        };
        assert!(require_consent(Some(&candidate), "current-token", true).is_ok());
        assert!(require_consent(Some(&candidate), "current-token", false).is_err());
        assert!(require_consent(Some(&candidate), "old-token", true).is_err());
        assert!(require_consent(None, "current-token", true).is_err());
    }

    #[test]
    fn cancelled_operation_unlocks_without_leaving_a_fake_busy_state() {
        let service = Service::new(Mode::Enabled, "0.1.0".to_owned());
        let operation = service.begin().unwrap();
        service.edit(|model| model.snapshot.phase = Phase::Checking);
        assert!(service.begin().is_err());
        drop(operation);
        assert_eq!(service.view().phase, Phase::Failed);
        assert!(service.begin().is_ok());
    }
}
