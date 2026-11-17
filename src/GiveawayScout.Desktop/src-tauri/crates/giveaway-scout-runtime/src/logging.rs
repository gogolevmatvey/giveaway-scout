use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use giveaway_scout_core::AppSettings;
use serde::Serialize;

const LOG_NAME: &str = "Giveaway Scout.log";
static NEXT_ROTATION: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogConfiguration {
    pub max_file_size_mb: u32,
    pub file_count: u32,
    pub directory: Option<String>,
}

impl From<&AppSettings> for LogConfiguration {
    fn from(settings: &AppSettings) -> Self {
        Self {
            max_file_size_mb: settings.log_max_file_size_mb,
            file_count: settings.log_file_count,
            directory: settings.log_directory.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoggingSnapshot {
    pub requested_at_startup: LogConfiguration,
    pub effective_directory: Option<String>,
    pub active_file_path: Option<String>,
    pub max_file_size_mb: u32,
    pub file_count: u32,
    pub restart_required: bool,
    pub last_error: Option<String>,
    pub using_fallback: bool,
}

#[derive(Clone)]
pub struct LoggingState(Arc<Mutex<LoggingInner>>);

struct LoggingInner {
    requested_at_startup: LogConfiguration,
    writer: Option<ManagedFileWriter>,
    max_file_size_mb: u32,
    file_count: u32,
    using_fallback: bool,
    last_error: Option<String>,
}

impl LoggingState {
    pub fn snapshot(&self, saved_settings: &AppSettings) -> LoggingSnapshot {
        // Recover the diagnostic state even if a previous logging callback panicked.
        let inner = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        LoggingSnapshot {
            requested_at_startup: inner.requested_at_startup.clone(),
            effective_directory: inner
                .writer
                .as_ref()
                .map(|writer| display_path(&writer.directory)),
            active_file_path: inner
                .writer
                .as_ref()
                .filter(|writer| writer.file.is_some())
                .map(|writer| display_path(&writer.active_path())),
            max_file_size_mb: inner.max_file_size_mb,
            file_count: inner.file_count,
            restart_required: inner.requested_at_startup != LogConfiguration::from(saved_settings),
            last_error: inner.last_error.clone(),
            using_fallback: inner.using_fallback,
        }
    }

    fn write_message(&self, message: &str) {
        let mut inner = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(writer) = &mut inner.writer
            && let Err(error) = writer.write_message(message)
        {
            // Never log from inside this lock: that would recursively call this writer.
            inner.last_error = Some(error.to_string());
        }
    }
}

/// The forced directory is used by isolated native UI tests. It deliberately has
/// no fallback to the normal profile, even when saved settings name another path.
pub fn initialize(
    settings: &AppSettings,
    default_directory: PathBuf,
    forced_directory: Option<PathBuf>,
) -> (LoggingState, fern::Dispatch) {
    let requested_at_startup = LogConfiguration::from(settings);
    let max_file_size_mb = settings.log_max_file_size_mb.clamp(1, 100);
    let file_count = settings.log_file_count.clamp(1, 10);
    let requested_directory = forced_directory
        .clone()
        .or_else(|| settings.log_directory.as_ref().map(PathBuf::from))
        .unwrap_or_else(|| default_directory.clone());
    let mut using_fallback = false;
    let last_error;
    let writer = match ManagedFileWriter::open(&requested_directory, max_file_size_mb, file_count) {
        Ok((writer, warning)) => {
            last_error = warning;
            Some(writer)
        }
        Err(error) => {
            let requested_error = format!("{}: {error}", requested_directory.display());
            if forced_directory.is_none() && requested_directory != default_directory {
                match ManagedFileWriter::open(&default_directory, max_file_size_mb, file_count) {
                    Ok((writer, warning)) => {
                        using_fallback = true;
                        last_error = Some(format!(
                            "Cannot use the selected log folder ({requested_error}). Using {}.{}",
                            default_directory.display(),
                            warning
                                .map(|warning| format!(" {warning}"))
                                .unwrap_or_default()
                        ));
                        Some(writer)
                    }
                    Err(fallback_error) => {
                        last_error = Some(format!(
                            "File logging is unavailable. Selected folder: {requested_error}. Standard folder {}: {fallback_error}",
                            default_directory.display()
                        ));
                        None
                    }
                }
            } else {
                last_error = Some(format!("File logging is unavailable ({requested_error})."));
                None
            }
        }
    };
    let state = LoggingState(Arc::new(Mutex::new(LoggingInner {
        requested_at_startup,
        writer,
        max_file_size_mb,
        file_count,
        using_fallback,
        last_error,
    })));
    let output = state.clone();
    let dispatch = fern::Dispatch::new().chain(fern::Output::call(move |record| {
        // Output::call receives the complete upstream-formatted message. Unlike
        // io::Write callbacks this does not split a record into formatting chunks.
        output.write_message(&format!("{}\n", record.args()));
    }));
    (state, dispatch)
}

struct ManagedFileWriter {
    directory: PathBuf,
    file: Option<File>,
    size: u64,
    max_size: u64,
    file_count: u32,
}

struct StagedLog {
    index: u64,
    original: PathBuf,
    staged: PathBuf,
}

impl ManagedFileWriter {
    fn open(
        directory: &Path,
        max_size_mb: u32,
        file_count: u32,
    ) -> io::Result<(Self, Option<String>)> {
        if !directory.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the log folder must be an absolute path",
            ));
        }
        fs::create_dir_all(directory)?;
        let active_path = directory.join(LOG_NAME);
        let file = open_active(&active_path)?;
        let size = file.metadata()?.len();
        let mut writer = Self {
            directory: directory.to_owned(),
            file: Some(file),
            size,
            max_size: u64::from(max_size_mb) * 1_000_000,
            file_count,
        };
        // Only prune after the active file was opened successfully.
        let warning = writer.prune_archives().err().map(|error| error.to_string());
        Ok((writer, warning))
    }

    fn active_path(&self) -> PathBuf {
        self.directory.join(LOG_NAME)
    }

    fn archive_path(&self, index: u64) -> PathBuf {
        self.directory.join(format!("Giveaway Scout.{index}.log"))
    }

    fn prune_archives(&mut self) -> io::Result<()> {
        let mut errors = Vec::new();
        for entry in fs::read_dir(&self.directory)? {
            let entry = entry?;
            let Some(index) = archive_index(&entry.file_name().to_string_lossy()) else {
                continue;
            };
            if index < u64::from(self.file_count) {
                continue;
            }
            match entry.file_type() {
                Ok(kind) if kind.is_file() => {
                    if let Err(error) = fs::remove_file(entry.path()) {
                        errors.push(format!("{}: {error}", entry.path().display()));
                    }
                }
                Ok(_) => errors.push(format!(
                    "Refusing to remove non-regular log archive {}",
                    entry.path().display()
                )),
                Err(error) => errors.push(format!("{}: {error}", entry.path().display())),
            }
        }
        errors_result(errors)
    }

    fn write_message(&mut self, message: &str) -> io::Result<()> {
        let mut rotation_error = None;
        if self.size > 0 && self.size.saturating_add(message.len() as u64) > self.max_size {
            rotation_error = self.rotate().err();
        }
        // If rotation failed, retain the new message in the existing file when
        // possible rather than dropping it to enforce the configured size.
        let Some(file) = &mut self.file else {
            return Err(rotation_error
                .unwrap_or_else(|| io::Error::other("the active log file is unavailable")));
        };
        if let Err(error) = file.write_all(message.as_bytes()) {
            self.size = file
                .metadata()
                .map(|metadata| metadata.len())
                .unwrap_or(self.size);
            return Err(error);
        }
        self.size = self.size.saturating_add(message.len() as u64);
        if let Some(error) = rotation_error {
            return Err(error);
        }
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        // Validate every destination before touching the existing files. In
        // particular, never follow a symlink masquerading as one of our archives.
        let mut originals = vec![(0, self.active_path())];
        for entry in fs::read_dir(&self.directory)? {
            let entry = entry?;
            if let Some(index) = archive_index(&entry.file_name().to_string_lossy()) {
                require_regular_file(&entry.path())?;
                originals.push((index, entry.path()));
            }
        }
        require_regular_file(&self.active_path())?;
        originals.sort_by_key(|(index, _)| *index);

        let transaction = create_rotation_directory(&self.directory)?;
        let new_path = transaction.join("new.log");
        let new_file = match OpenOptions::new()
            .append(true)
            .create_new(true)
            .open(&new_path)
        {
            Ok(file) => file,
            Err(error) => {
                let _ = fs::remove_dir(&transaction);
                return Err(error);
            }
        };
        if let Some(file) = &mut self.file
            && let Err(error) = file.flush()
        {
            drop(new_file);
            let _ = fs::remove_file(&new_path);
            let _ = fs::remove_dir(&transaction);
            return Err(error);
        }
        self.file.take();

        let mut staged = Vec::new();
        for (index, original) in originals {
            let staged_path = transaction.join(format!("old-{index}.log"));
            if let Err(error) = fs::rename(&original, &staged_path) {
                drop(new_file);
                return Err(self.rollback_rotation(&transaction, &new_path, &staged, error));
            }
            staged.push(StagedLog {
                index,
                original,
                staged: staged_path,
            });
        }
        if let Err(error) = fs::rename(&new_path, self.active_path()) {
            drop(new_file);
            return Err(self.rollback_rotation(&transaction, &new_path, &staged, error));
        }
        // No old file is discarded before the newly opened active file exists.
        self.file = Some(new_file);
        self.size = 0;

        let mut errors = Vec::new();
        for old in staged {
            if old.index < u64::from(self.file_count - 1) {
                let destination = self.archive_path(old.index + 1);
                let result = match fs::symlink_metadata(&destination) {
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        fs::rename(&old.staged, &destination)
                    }
                    Err(error) => Err(error),
                    Ok(_) => Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "archive destination already exists",
                    )),
                };
                if let Err(error) = result {
                    errors.push(format!(
                        "Preserved log at {} because {} could not be created: {error}",
                        old.staged.display(),
                        destination.display()
                    ));
                }
            } else if let Err(error) = fs::remove_file(&old.staged) {
                errors.push(format!(
                    "Could not remove old log {}: {error}",
                    old.staged.display()
                ));
            }
        }
        if let Err(error) = fs::remove_dir(&transaction) {
            errors.push(format!(
                "Log rotation recovery folder {}: {error}",
                transaction.display()
            ));
        }
        errors_result(errors)
    }

    fn rollback_rotation(
        &mut self,
        transaction: &Path,
        new_path: &Path,
        staged: &[StagedLog],
        cause: io::Error,
    ) -> io::Error {
        let mut errors = vec![format!("Log rotation failed: {cause}")];
        for old in staged.iter().rev() {
            if let Err(error) = fs::rename(&old.staged, &old.original) {
                errors.push(format!(
                    "Preserved log at {}; could not restore {}: {error}",
                    old.staged.display(),
                    old.original.display()
                ));
            }
        }
        let _ = fs::remove_file(new_path);
        let _ = fs::remove_dir(transaction);
        match open_active(&self.active_path()) {
            Ok(file) => {
                self.size = file
                    .metadata()
                    .map(|metadata| metadata.len())
                    .unwrap_or(self.size);
                self.file = Some(file);
            }
            Err(error) => errors.push(format!(
                "Could not reopen {}: {error}",
                self.active_path().display()
            )),
        }
        io::Error::other(errors.join(". "))
    }
}

fn open_active(path: &Path) -> io::Result<File> {
    match fs::symlink_metadata(path) {
        Ok(_) => require_regular_file(path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    OpenOptions::new().create(true).append(true).open(path)
}

fn require_regular_file(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.file_type().is_file() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "Refusing to use non-regular log file {}",
            path.display()
        )))
    }
}

fn archive_index(name: &str) -> Option<u64> {
    let number = name.strip_prefix("Giveaway Scout.")?.strip_suffix(".log")?;
    let index = number.parse::<u64>().ok()?;
    (index > 0 && number == index.to_string()).then_some(index)
}

fn create_rotation_directory(directory: &Path) -> io::Result<PathBuf> {
    loop {
        let sequence = NEXT_ROTATION.fetch_add(1, Ordering::Relaxed);
        let candidate = directory.join(format!(
            ".giveaway-scout-log-rotation-{}-{sequence}",
            std::process::id()
        ));
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}

fn errors_result(errors: Vec<String>) -> io::Result<()> {
    if errors.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(errors.join(". ")))
    }
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn writer(directory: &Path, max_size: u64, count: u32) -> ManagedFileWriter {
        let (mut writer, warning) = ManagedFileWriter::open(directory, 2, count).unwrap();
        assert!(warning.is_none());
        writer.max_size = max_size;
        writer
    }

    fn read(directory: &Path, name: &str) -> String {
        fs::read_to_string(directory.join(name)).unwrap()
    }

    #[test]
    fn single_file_replaces_old_content_only_when_next_whole_record_crosses_limit() {
        let directory = tempdir().unwrap();
        let mut writer = writer(directory.path(), 8, 1);
        writer.write_message("one\n").unwrap();
        writer.write_message("two\n").unwrap();
        assert_eq!(read(directory.path(), LOG_NAME), "one\ntwo\n");
        writer.write_message("three\n").unwrap();
        assert_eq!(read(directory.path(), LOG_NAME), "three\n");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn rapid_rotation_keeps_exact_file_count_and_newest_archives_first() {
        let directory = tempdir().unwrap();
        let mut writer = writer(directory.path(), 2, 3);
        for record in ["1\n", "2\n", "3\n", "4\n", "5\n"] {
            writer.write_message(record).unwrap();
        }
        assert_eq!(read(directory.path(), LOG_NAME), "5\n");
        assert_eq!(read(directory.path(), "Giveaway Scout.1.log"), "4\n");
        assert_eq!(read(directory.path(), "Giveaway Scout.2.log"), "3\n");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 3);
    }

    #[test]
    fn oversized_utf8_record_stays_whole_and_size_counts_bytes() {
        let directory = tempdir().unwrap();
        let mut writer = writer(directory.path(), 5, 3);
        writer.write_message("я\n").unwrap();
        writer.write_message("ю\n").unwrap();
        assert_eq!(read(directory.path(), "Giveaway Scout.1.log"), "я\n");
        assert_eq!(read(directory.path(), LOG_NAME), "ю\n");
        writer.write_message("длинное сообщение\n").unwrap();
        assert_eq!(read(directory.path(), LOG_NAME), "длинное сообщение\n");
        assert_eq!(writer.size, "длинное сообщение\n".len() as u64);
        writer.write_message("x\n").unwrap();
        assert_eq!(
            read(directory.path(), "Giveaway Scout.1.log"),
            "длинное сообщение\n"
        );
    }

    #[test]
    fn dispatch_preserves_complete_upstream_formatted_multiline_message() {
        let directory = tempdir().unwrap();
        let settings = AppSettings::default();
        let (state, output) = initialize(&settings, directory.path().to_owned(), None);
        state.0.lock().unwrap().writer.as_mut().unwrap().max_size = 5;
        let (_, logger) = fern::Dispatch::new()
            .format(|output, message, record| {
                output.finish(format_args!("[{}] {message}", record.level()));
            })
            .chain(output)
            .into_log();
        logger.log(
            &log::Record::builder()
                .args(format_args!("one\ntwo"))
                .level(log::Level::Info)
                .build(),
        );
        assert_eq!(read(directory.path(), LOG_NAME), "[INFO] one\ntwo\n");
        logger.log(
            &log::Record::builder()
                .args(format_args!("three"))
                .level(log::Level::Info)
                .build(),
        );
        assert_eq!(read(directory.path(), LOG_NAME), "[INFO] three\n");
        // Default count=1 discards the previous whole record, without preserving
        // only a fragment as would happen with a chunk-oriented Write callback.
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn default_threshold_is_two_million_bytes_and_existing_full_file_rotates_on_next_message() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join(LOG_NAME), "a".repeat(2_000_000)).unwrap();
        let settings = AppSettings {
            log_file_count: 2,
            ..AppSettings::default()
        };
        let (state, _) = initialize(&settings, directory.path().to_owned(), None);
        assert_eq!(
            state.0.lock().unwrap().writer.as_ref().unwrap().max_size,
            2_000_000
        );
        state.write_message("next\n");
        assert_eq!(read(directory.path(), LOG_NAME), "next\n");
        assert_eq!(
            fs::metadata(directory.path().join("Giveaway Scout.1.log"))
                .unwrap()
                .len(),
            2_000_000
        );
    }

    #[test]
    fn startup_reduction_removes_only_exact_own_archive_names() {
        let directory = tempdir().unwrap();
        for name in [
            "Giveaway Scout.1.log",
            "Giveaway Scout.2.log",
            "Giveaway Scout.10.log",
            "Giveaway Scout.01.log",
            "Giveaway Scout.0.log",
            "Giveaway Scout.other.log",
            "Other.9.log",
        ] {
            fs::write(directory.path().join(name), name).unwrap();
        }
        let _writer = writer(directory.path(), 4, 2);
        assert!(directory.path().join("Giveaway Scout.1.log").exists());
        assert!(!directory.path().join("Giveaway Scout.2.log").exists());
        assert!(!directory.path().join("Giveaway Scout.10.log").exists());
        for name in [
            "Giveaway Scout.01.log",
            "Giveaway Scout.0.log",
            "Giveaway Scout.other.log",
            "Other.9.log",
        ] {
            assert!(directory.path().join(name).exists());
        }
    }

    #[test]
    fn custom_folder_falls_back_without_changing_restart_baseline_or_old_logs() {
        let root = tempdir().unwrap();
        let blocked = root.path().join("blocked");
        fs::write(&blocked, "not a directory").unwrap();
        let standard = root.path().join("standard");
        let settings = AppSettings {
            log_directory: Some(display_path(&blocked)),
            ..AppSettings::default()
        };
        let (state, _) = initialize(&settings, standard.clone(), None);
        let snapshot = state.snapshot(&settings);
        assert!(snapshot.using_fallback);
        assert!(!snapshot.restart_required);
        assert!(snapshot.last_error.is_some());
        assert_eq!(snapshot.effective_directory, Some(display_path(&standard)));
        let saved = AppSettings {
            log_directory: None,
            ..settings.clone()
        };
        assert!(state.snapshot(&saved).restart_required);
        assert_eq!(fs::read_to_string(blocked).unwrap(), "not a directory");
    }

    #[test]
    fn failure_of_both_directories_disables_only_file_logging() {
        let root = tempdir().unwrap();
        let custom = root.path().join("custom");
        let standard = root.path().join("standard");
        fs::write(&custom, "custom").unwrap();
        fs::write(&standard, "standard").unwrap();
        let settings = AppSettings {
            log_directory: Some(display_path(&custom)),
            ..AppSettings::default()
        };
        let (state, _) = initialize(&settings, standard, None);
        state.write_message("still safe\n");
        let snapshot = state.snapshot(&settings);
        assert!(snapshot.active_file_path.is_none());
        assert!(snapshot.effective_directory.is_none());
        assert!(!snapshot.using_fallback);
        assert!(snapshot.last_error.unwrap().contains("Standard folder"));
        assert!(!snapshot.restart_required);
    }

    #[test]
    fn isolated_profile_ignores_custom_folder_and_never_falls_back_to_normal_profile() {
        let root = tempdir().unwrap();
        let custom = root.path().join("custom");
        let standard = root.path().join("standard");
        let isolated = root.path().join("isolated");
        let settings = AppSettings {
            log_directory: Some(display_path(&custom)),
            ..AppSettings::default()
        };
        let (state, _) = initialize(&settings, standard.clone(), Some(isolated.clone()));
        state.write_message("isolated\n");
        assert_eq!(read(&isolated, LOG_NAME), "isolated\n");
        assert!(!custom.exists());
        assert!(!standard.exists());
        drop(state);
        let blocked = root.path().join("blocked");
        fs::write(&blocked, "file").unwrap();
        let (state, _) = initialize(&settings, standard.clone(), Some(blocked));
        assert!(state.snapshot(&settings).active_file_path.is_none());
        assert!(!standard.exists());
    }

    #[test]
    fn changing_folder_keeps_previous_logs_and_current_path_until_restart() {
        let root = tempdir().unwrap();
        let first = root.path().join("first");
        let second = root.path().join("second");
        let settings = AppSettings {
            log_directory: Some(display_path(&first)),
            ..AppSettings::default()
        };
        let (state, _) = initialize(&settings, root.path().join("standard"), None);
        state.write_message("old folder\n");
        let saved = AppSettings {
            log_directory: Some(display_path(&second)),
            ..settings
        };
        assert_eq!(
            state.snapshot(&saved).active_file_path,
            Some(display_path(&first.join(LOG_NAME)))
        );
        assert!(state.snapshot(&saved).restart_required);
        drop(state);
        let (next, _) = initialize(&saved, root.path().join("standard"), None);
        next.write_message("new folder\n");
        assert_eq!(read(&first, LOG_NAME), "old folder\n");
        assert_eq!(read(&second, LOG_NAME), "new folder\n");
        assert!(!next.snapshot(&saved).restart_required);
    }

    #[test]
    fn rotation_error_preserves_old_file_and_incoming_message() {
        let directory = tempdir().unwrap();
        let mut writer = writer(directory.path(), 4, 2);
        writer.write_message("old\n").unwrap();
        fs::create_dir(directory.path().join("Giveaway Scout.1.log")).unwrap();
        assert!(writer.write_message("new\n").is_err());
        assert_eq!(read(directory.path(), LOG_NAME), "old\nnew\n");
        assert!(directory.path().join("Giveaway Scout.1.log").is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn archive_symlinks_are_never_followed_or_removed() {
        use std::os::unix::fs::symlink;
        let root = tempdir().unwrap();
        let outside = root.path().join("outside.log");
        fs::write(&outside, "keep me").unwrap();
        let logs = root.path().join("logs");
        fs::create_dir(&logs).unwrap();
        symlink(&outside, logs.join("Giveaway Scout.1.log")).unwrap();
        let (mut writer, warning) = ManagedFileWriter::open(&logs, 2, 1).unwrap();
        assert!(warning.is_some());
        writer.max_size = 1;
        writer.write_message("one\n").unwrap();
        assert!(writer.write_message("two\n").is_err());
        assert_eq!(fs::read_to_string(outside).unwrap(), "keep me");
        assert!(
            fs::symlink_metadata(logs.join("Giveaway Scout.1.log"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}
