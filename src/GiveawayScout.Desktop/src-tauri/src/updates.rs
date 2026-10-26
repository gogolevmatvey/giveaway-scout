//! Application-owned updater API: no arbitrary URLs/keys or install commands in JS.
#[cfg(target_os = "linux")]
use std::path::Path;
use std::{sync::Mutex, time::Duration};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Serialize;
use tauri::{AppHandle, Manager, utils::config::BundleType};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::operations::{DataOperationGate, DataOperationGuard};

const MAX_PACKAGE_BYTES: usize = 256 * 1024 * 1024;
const MAX_NOTES_CHARS: usize = 20_000;

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
                    downloaded_bytes: 0,
                    total_bytes: None,
                    last_checked_at_utc: None,
                    last_error: None,
                },
                pending: None,
            }),
            operation: DataOperationGate::default(),
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
        })
    }
}

struct Operation<'a> {
    service: &'a Service,
    _guard: DataOperationGuard<'a>,
}
impl Drop for Operation<'_> {
    fn drop(&mut self) {
        self.service.edit(|model| {
            if matches!(
                model.snapshot.phase,
                Phase::Checking | Phase::Downloading | Phase::Installing
            ) {
                model.snapshot.phase = Phase::Failed;
                model.snapshot.last_error = Some("UPDATES_INTERRUPTED".to_owned());
            }
        });
    }
}

fn secure_url(url: &url::Url) -> bool {
    url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
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
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                if let Err(error) = check_for_updates(handle.clone()).await {
                    log::warn!("update check failed: {error}");
                }
                tokio::time::sleep(Duration::from_secs(24 * 60 * 60)).await;
            }
        });
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn get_update_state(service: tauri::State<'_, Service>) -> Snapshot {
    service.view()
}

fn client_policy(client: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    client
        .https_only(true)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if secure_url(attempt.url()) && attempt.previous().len() < 5 {
                attempt.follow()
            } else {
                attempt.error("unsafe or excessive update redirect")
            }
        }))
}

#[tauri::command]
pub(crate) async fn check_for_updates(app: AppHandle) -> Result<Snapshot, String> {
    let service = app
        .try_state::<Service>()
        .ok_or_else(|| "UPDATES_UNAVAILABLE".to_owned())?;
    let _operation = service.begin()?;
    service.edit(|model| {
        model.snapshot.phase = Phase::Checking;
        model.snapshot.candidate = None;
        model.pending = None;
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
    match result {
        Ok(update) => service.edit(|model| {
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
    Ok(service.view())
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
        // install verifies bytes and signed version again before any executable
        // is started. Windows invokes Tauri's normal cleanup-before-exit hook.
        tauri::async_runtime::spawn_blocking(move || {
            update.install(bytes).map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| error.to_string())??;
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
    // Windows normally exits from install(); other platforms need an explicit restart.
    app.restart();
}

#[cfg(test)]
mod tests {
    use super::*;

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
