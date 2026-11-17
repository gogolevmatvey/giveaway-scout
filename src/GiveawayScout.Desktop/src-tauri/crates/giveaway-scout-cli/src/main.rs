mod arguments;
mod history;
mod output;
mod settings;

use std::{
    io::{self, Write},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use arguments::{Arguments, Command, HistoryFilters};
use giveaway_scout_core::{
    AppSettings, Database, GiveawaySource, InitialSyncMode, NotificationRecord, StorageError,
};
use giveaway_scout_platform::{PlatformError, ProfileGuard};
use giveaway_scout_runtime::{RunMode, Runtime, RuntimeDependencies, RuntimeEvent, SharedDatabase};
use serde_json::json;

macro_rules! diagnostic {
    ($($argument:tt)*) => {{ let _ = writeln!(std::io::stderr().lock(), $($argument)*); }};
}

#[derive(Debug)]
struct Failure {
    code: u8,
    message: String,
}
type Result<T> = std::result::Result<T, Failure>;

impl Failure {
    fn execution(message: impl ToString) -> Self {
        Self {
            code: 1,
            message: message.to_string(),
        }
    }
    fn usage(message: impl ToString) -> Self {
        Self {
            code: 2,
            message: message.to_string(),
        }
    }
}
impl From<PlatformError> for Failure {
    fn from(error: PlatformError) -> Self {
        let code = match &error {
            PlatformError::ProfileBusy(_) => 3,
            PlatformError::InvalidSettings(_)
            | PlatformError::Storage(StorageError::InvalidSettings(_)) => 2,
            _ => 1,
        };
        Self {
            code,
            message: error.to_string(),
        }
    }
}

/// Field order drops the database before releasing its exclusive profile lock.
struct Profile {
    database: SharedDatabase,
    _guard: ProfileGuard,
    directory: PathBuf,
}

impl Profile {
    fn open(directory: PathBuf) -> Result<Self> {
        let guard = ProfileGuard::acquire(&directory)?;
        let (database, recovery) =
            Database::open_with_recovery(directory.join("giveaway-scout.db"))
                .map_err(Failure::execution)?;
        if let Some(recovery) = recovery {
            diagnostic!(
                "SQLite recovery: preserved {} ({})",
                recovery.backup_path,
                recovery.original_error
            );
        }
        Ok(Self {
            database: Arc::new(Mutex::new(database)),
            _guard: guard,
            directory,
        })
    }

    fn settings(&self) -> Result<AppSettings> {
        self.database
            .lock()
            .map_err(Failure::execution)?
            .settings()
            .map_err(Failure::execution)
    }
}

fn print_line(value: &str) -> Result<()> {
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "{value}")
        .and_then(|()| stdout.flush())
        .map_err(Failure::execution)
}

fn initialize_logging(profile: &Profile, settings: &AppSettings) -> Result<()> {
    let (state, dispatch) =
        giveaway_scout_runtime::logging::initialize(settings, profile.directory.join("logs"), None);
    let level = if settings.verbose_logging {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };
    fern::Dispatch::new()
        .level(level)
        .format(|out, message, record| out.finish(format_args!("[{}] {}", record.level(), message)))
        .chain(io::stderr())
        .chain(dispatch)
        .apply()
        .map_err(Failure::execution)?;
    if let Some(error) = state.snapshot(settings).last_error {
        diagnostic!("Logging: {error}");
    }
    Ok(())
}

fn matches_history(record: &NotificationRecord, filters: &HistoryFilters) -> bool {
    let source_matches = filters.source.is_none_or(|source| {
        record.origin_source == Some(source)
            || record.detected_sources.contains(&source)
            || record.source.eq_ignore_ascii_case(source.storage_name())
    });
    source_matches
        && filters.store.is_none_or(|store| store == record.store)
        && (!filters.unread || record.read_at_utc.is_none())
        && (filters.search.trim().is_empty()
            || record
                .title
                .to_lowercase()
                .contains(&filters.search.trim().to_lowercase()))
}

async fn monitor(
    profile: Profile,
    json_output: bool,
    initial: Option<InitialSyncMode>,
    mode: RunMode,
    separate_profile: bool,
) -> Result<u8> {
    let mut settings = profile.settings()?;
    if settings.initial_sync_mode.is_none() {
        settings.initial_sync_mode = Some(initial.unwrap_or(InitialSyncMode::Silent));
        profile
            .database
            .lock()
            .map_err(Failure::execution)?
            .update_settings(&settings)
            .map_err(Failure::execution)?;
    } else if initial.is_some_and(|mode| Some(mode) != settings.initial_sync_mode) {
        return Err(Failure::usage(
            "The profile already has an initial synchronization mode; use config set initialSyncMode to change it",
        ));
    }
    if !GiveawaySource::ALL
        .into_iter()
        .any(|source| settings.allows_source(source))
    {
        return Err(Failure::usage(
            "No sources are enabled; use config set <source>Enabled true",
        ));
    }
    initialize_logging(&profile, &settings)?;
    let ru = output::russian(&settings);
    let output_failed = Arc::new(AtomicBool::new(false));
    let sink_failed = output_failed.clone();
    let sink = Arc::new(move |record: &NotificationRecord, _: &AppSettings| {
        let result = output::notification(&mut io::stdout().lock(), record, json_output);
        if result.is_err() {
            sink_failed.store(true, Ordering::Release);
        }
        result
    });
    let observer_failed = output_failed.clone();
    let observer = Arc::new(move |event| {
        if let RuntimeEvent::SourceCompleted(completed) = event {
            if json_output {
                if output::json(&json!({"type":"sourceCompleted","result":completed})).is_err() {
                    observer_failed.store(true, Ordering::Release);
                }
            } else {
                match &completed.outcome {
                    Ok(outcome) => {
                        for failure in &outcome.failures {
                            diagnostic!("{}: {}", failure.source.storage_name(), failure.message);
                        }
                        diagnostic!(
                            "{}: {} {}, {} {}",
                            completed.source.storage_name(),
                            outcome.new_items.len(),
                            output::text(ru, "новых", "new"),
                            outcome.failures.len(),
                            output::text(ru, "ошибок", "failures")
                        );
                    }
                    Err(error) => diagnostic!("{}: {error}", completed.source.storage_name()),
                }
            }
        }
    });
    let mut dependencies = RuntimeDependencies::production(sink, observer);
    dependencies.fail_on_delivery_error = true;
    let runtime =
        Runtime::new(profile.database.clone(), dependencies).map_err(Failure::execution)?;
    giveaway_scout_runtime::usage_statistics::start(
        &runtime,
        env!("CARGO_PKG_VERSION"),
        cfg!(debug_assertions) || separate_profile,
        false,
    );
    let signal_runtime = runtime.clone();
    let signal = tokio::spawn(async move {
        match tokio::signal::ctrl_c().await {
            Ok(()) => {
                signal_runtime.request_shutdown();
                // A downstream pipe can stay open without consuming stdout.
                // Keep Ctrl+C responsive even if its synchronous writer stalls.
                if tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    signal_runtime.shutdown(),
                )
                .await
                .is_err()
                {
                    std::process::exit(130);
                }
            }
            Err(error) => {
                log::error!("Ctrl+C handler is unavailable: {error}");
                signal_runtime.request_shutdown();
            }
        }
    });
    // Output observers cannot return errors. Poll this shared flag so even an
    // error writing a status event stops watch promptly, without another fetch.
    let pipe_runtime = runtime.clone();
    let pipe_failed = output_failed.clone();
    let pipe = tokio::spawn(async move {
        while !pipe_runtime.is_stopped() {
            if pipe_failed.load(Ordering::Acquire) {
                pipe_runtime.request_shutdown();
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    });
    let result = runtime.clone().run(mode).await;
    runtime.shutdown().await;
    signal.abort();
    pipe.abort();
    let _ = signal.await;
    let _ = pipe.await;
    if output_failed.load(Ordering::Acquire) {
        return Err(Failure::execution(
            "Cannot write console output; undelivered notifications remain pending",
        ));
    }
    let report = result.map_err(Failure::execution)?;
    let failed = report.completions.iter().any(|entry| match &entry.outcome {
        Ok(outcome) => !outcome.failures.is_empty(),
        Err(_) => true,
    });
    if json_output {
        output::json(&json!({"type":"summary","report":report,"sources":runtime.source_states().map_err(Failure::execution)?})).map_err(Failure::execution)?;
    } else if mode == RunMode::Once {
        print_line(output::text(ru, "Проверка завершена.", "Check completed."))?;
    }
    Ok(if report.interrupted {
        130
    } else if failed {
        1
    } else {
        0
    })
}

fn profile_directory(arguments: &Arguments) -> Result<PathBuf> {
    match &arguments.data_dir {
        Some(directory) if directory.is_absolute() => Ok(directory.clone()),
        Some(directory) => Ok(std::env::current_dir()
            .map_err(Failure::execution)?
            .join(directory)),
        None => giveaway_scout_platform::default_data_directory(cfg!(debug_assertions))
            .map_err(Into::into),
    }
}

async fn update(arguments: &Arguments, install: bool) -> Result<u8> {
    let config = giveaway_scout_updater::COMPILED_CONFIG.ok_or_else(|| {
        Failure::execution(
            "Updates are not configured in this build; use an installed signed release",
        )
    })?;
    let candidate = giveaway_scout_updater::check(config, env!("CARGO_PKG_VERSION"))
        .await
        .map_err(Failure::execution)?;
    if !install {
        if arguments.json {
            output::json(&json!({"currentVersion":env!("CARGO_PKG_VERSION"),"availableVersion":candidate.as_ref().map(|candidate| &candidate.version),"notes":candidate.as_ref().map(|candidate| &candidate.notes)})).map_err(Failure::execution)?;
        } else if let Some(candidate) = candidate {
            print_line(&format!(
                "Update available: {}\n{}",
                candidate.version,
                output::safe_notes(&candidate.notes)
            ))?;
        } else {
            print_line("Giveaway Scout is up to date.")?;
        }
        return Ok(0);
    }
    if cfg!(debug_assertions) {
        return Err(Failure::execution(
            "Installing updates is disabled in development builds",
        ));
    }
    let executable = std::env::current_exe().map_err(Failure::execution)?;
    if !giveaway_scout_platform::is_current_installation(&executable) {
        return Err(Failure::execution(
            "This executable is not the registered CLI installation; download and run setup manually",
        ));
    }
    let Some(candidate) = candidate else {
        if arguments.json {
            output::json(&json!({"status":"upToDate","currentVersion":env!("CARGO_PKG_VERSION")}))
                .map_err(Failure::execution)?;
        } else {
            print_line("Giveaway Scout is up to date.")?;
        }
        return Ok(0);
    };
    let profile = Profile::open(profile_directory(arguments)?)?;
    diagnostic!("Downloading and verifying {}…", candidate.version);
    let bytes = giveaway_scout_updater::download(config, &candidate)
        .await
        .map_err(Failure::execution)?;
    let package = giveaway_scout_updater::stage_installer(
        config.public_key,
        &candidate.signature,
        &candidate.version,
        &candidate.filename,
        &bytes,
    )
    .map_err(Failure::execution)?;
    if !profile
        .database
        .lock()
        .map_err(Failure::execution)?
        .checkpoint_before_update()
        .map_err(Failure::execution)?
    {
        return Err(Failure::execution("Database checkpoint is busy"));
    }
    drop(profile);
    package
        .launch(std::process::id(), false)
        .map_err(Failure::execution)?;
    if arguments.json {
        output::json(&json!({"status":"installerStarted","version":candidate.version}))
            .map_err(Failure::execution)?;
    } else {
        print_line("Installer started. This CLI is exiting so setup can replace it.")?;
    }
    Ok(0)
}

async fn execute(arguments: Arguments) -> Result<u8> {
    match &arguments.command {
        Command::Help => {
            print_line(arguments::HELP)?;
            return Ok(0);
        }
        Command::Version => {
            print_line(concat!("giveaway-scout-cli ", env!("CARGO_PKG_VERSION")))?;
            return Ok(0);
        }
        Command::MaintenanceCheck => {
            if arguments.json || arguments.data_dir.is_some() {
                return Err(Failure::usage(
                    "Maintenance does not accept global profile/output options",
                ));
            }
            giveaway_scout_platform::check_profile_available()?;
            return Ok(0);
        }
        Command::Maintenance {
            components,
            install_dir,
            remove,
        } => {
            if arguments.json || arguments.data_dir.is_some() {
                return Err(Failure::usage(
                    "Maintenance does not accept global profile/output options",
                ));
            }
            giveaway_scout_platform::maintain_installation(install_dir, *components, *remove)?;
            return Ok(0);
        }
        Command::UpdateCheck => return update(&arguments, false).await,
        Command::UpdateInstall => return update(&arguments, true).await,
        _ => {}
    }
    let profile = Profile::open(profile_directory(&arguments)?)?;
    match arguments.command {
        Command::Check(initial) => {
            monitor(
                profile,
                arguments.json,
                initial,
                RunMode::Once,
                arguments.data_dir.is_some(),
            )
            .await
        }
        Command::Watch(initial) => {
            monitor(
                profile,
                arguments.json,
                initial,
                RunMode::Watch,
                arguments.data_dir.is_some(),
            )
            .await
        }
        Command::History(filters) => {
            let records: Vec<_> = profile
                .database
                .lock()
                .map_err(Failure::execution)?
                .notifications()
                .map_err(Failure::execution)?
                .into_iter()
                .filter(|record| matches_history(record, &filters))
                .collect();
            if arguments.json {
                output::json(&json!({"items":records})).map_err(Failure::execution)?;
            } else {
                let ru = output::russian(&profile.settings()?);
                let mut writer = io::stdout().lock();
                for record in records {
                    output::history_record(&mut writer, &record, ru).map_err(Failure::execution)?;
                }
            }
            Ok(0)
        }
        Command::Read(id) => {
            let database = profile.database.lock().map_err(Failure::execution)?;
            history::read(&database, &id, &mut io::stdout().lock(), arguments.json)
                .map_err(Failure::execution)?;
            Ok(0)
        }
        Command::ReadAll => {
            let ru = output::russian(&profile.settings()?);
            let marked = profile
                .database
                .lock()
                .map_err(Failure::execution)?
                .mark_all_notifications_read()
                .map_err(Failure::execution)?;
            if arguments.json {
                output::json(&json!({"markedRead":marked})).map_err(Failure::execution)?;
            } else {
                print_line(&format!(
                    "{}: {marked}",
                    output::text(ru, "Отмечено прочитанными", "Marked read")
                ))?;
            }
            Ok(0)
        }
        Command::Open(id) => {
            let ru = output::russian(&profile.settings()?);
            let database = profile.database.lock().map_err(Failure::execution)?;
            let url = history::open_with(&database, &id, |url| {
                open::that(url).map_err(|error| error.to_string())
            })
            .map_err(Failure::execution)?;
            if arguments.json {
                output::json(&json!({"id":id,"url":url,"opened":true}))
                    .map_err(Failure::execution)?;
            } else {
                print_line(output::text(
                    ru,
                    "Ссылка передана браузеру, запись отмечена прочитанной.",
                    "URL sent to the browser; record marked read.",
                ))?;
            }
            Ok(0)
        }
        Command::ConfigShow => {
            let settings = profile.settings()?;
            if arguments.json {
                output::json(&settings).map_err(Failure::execution)?;
            } else {
                print_line(&serde_json::to_string_pretty(&settings).map_err(Failure::execution)?)?;
            }
            Ok(0)
        }
        Command::ConfigSet { key, value } => {
            let database = profile.database.lock().map_err(Failure::execution)?;
            let previous = database.settings().map_err(Failure::execution)?;
            let next = settings::changed(&previous, &key, &value).map_err(Failure::usage)?;
            if arguments.data_dir.is_some() && next.start_at_login {
                return Err(Failure::usage(
                    "Windows autostart uses the default profile; configure it without --data-dir",
                ));
            }
            giveaway_scout_platform::update_settings(
                &database,
                &next,
                cfg!(debug_assertions) || arguments.data_dir.is_some(),
            )?;
            if arguments.json {
                output::json(&next).map_err(Failure::execution)?;
            } else {
                print_line(output::text(
                    output::russian(&next),
                    "Настройки сохранены.",
                    "Settings saved.",
                ))?;
            }
            Ok(0)
        }
        _ => unreachable!("handled before opening profile"),
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> std::process::ExitCode {
    let parsed = std::env::args_os()
        .skip(1)
        .map(|argument| {
            argument
                .into_string()
                .map_err(|_| Failure::usage("Arguments must contain valid Unicode"))
        })
        .collect::<Result<Vec<_>>>();
    let result = match parsed {
        Ok(args) => match arguments::parse(args) {
            Ok(arguments) => execute(arguments).await,
            Err(error) => Err(Failure::usage(error)),
        },
        Err(error) => Err(error),
    };
    match result {
        Ok(code) => std::process::ExitCode::from(code),
        Err(error) => {
            diagnostic!("Giveaway Scout: {}", error.message);
            std::process::ExitCode::from(error.code)
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn update_handoff_never_restarts_gui_or_forwards_cli_arguments() {
        assert_eq!(
            giveaway_scout_updater::installer_arguments(42, false),
            ["/P", "/UPDATE", "/WAITPID=42"]
        );
    }
}
