//! Tauri-independent scheduling, ingestion and notification delivery.
mod coordinator;
pub mod logging;
pub mod operations;
mod redirects;
pub mod usage_statistics;

use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use giveaway_scout_core::{
    AppSettings, Database, DeliveryReport, HttpPageFetcher, MonitoringOutcome, NotificationRecord,
    PageFetcher, SourceCheckResult, SourceFailure, SourceFailureReason, SourceStatus,
    apply_source_check, check_source, dispatch_pending_notifications,
};
use serde::Serialize;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio::{
    sync::{Notify, mpsc},
    task::JoinHandle,
};

pub use coordinator::CheckStartResult;
use coordinator::{Coordinator, RunToken};
pub use giveaway_scout_core::GiveawaySource;
use operations::DataOperationGate;
use redirects::RedirectPipeline;
pub use redirects::{RedirectLookup, RedirectResolver, resolver_from_transport};

pub type SharedDatabase = Arc<Mutex<Database>>;
pub type NotificationSink =
    Arc<dyn Fn(&NotificationRecord, &AppSettings) -> Result<(), String> + Send + Sync>;
pub type RuntimeObserver = Arc<dyn Fn(RuntimeEvent) + Send + Sync>;
pub type SourceFuture<'a> = Pin<Box<dyn Future<Output = SourceCheckResult> + Send + 'a>>;

/// Object-safe boundary used by the native fixture harness and runtime tests.
pub trait SourceFetcher: Send + Sync {
    fn check(&self, source: GiveawaySource) -> SourceFuture<'_>;
}

struct PageFetcherAdapter<F>(F);
impl<F: PageFetcher + Send + Sync> SourceFetcher for PageFetcherAdapter<F> {
    fn check(&self, source: GiveawaySource) -> SourceFuture<'_> {
        Box::pin(check_source(&self.0, source))
    }
}

pub fn source_fetcher<F: PageFetcher + Send + Sync + 'static>(
    fetcher: F,
) -> Arc<dyn SourceFetcher> {
    Arc::new(PageFetcherAdapter(fetcher))
}

pub type FetcherFactory =
    Arc<dyn Fn(&AppSettings) -> Result<Arc<dyn SourceFetcher>, String> + Send + Sync>;

pub struct RuntimeDependencies {
    pub fetcher_factory: FetcherFactory,
    pub resolver: Arc<dyn RedirectResolver>,
    pub notification_sink: NotificationSink,
    pub observer: RuntimeObserver,
    /// Console output failures must terminate a pipe. Desktop transports keep
    /// their existing retryable pending-notification behavior.
    pub fail_on_delivery_error: bool,
}

impl RuntimeDependencies {
    pub fn production(notification_sink: NotificationSink, observer: RuntimeObserver) -> Self {
        Self {
            fetcher_factory: Arc::new(|settings| {
                HttpPageFetcher::with_policy(
                    settings.http_timeout_seconds,
                    settings.http_retry_count,
                )
                .map(source_fetcher)
                .map_err(|error| error.to_string())
            }),
            resolver: redirects::production_resolver(),
            notification_sink,
            observer,
            fail_on_delivery_error: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    Once,
    Watch,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceCompletion {
    pub source: GiveawaySource,
    pub outcome: Result<MonitoringOutcome, String>,
    pub unresolved_links: u64,
    pub delivery: DeliveryReport,
}

#[derive(Debug, Clone)]
pub enum RuntimeEvent {
    HistoryChanged,
    MonitorChanged,
    SourceCompleted(SourceCompletion),
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunReport {
    pub completions: Vec<SourceCompletion>,
    pub interrupted: bool,
}

struct CompletedCheck {
    token: RunToken,
    result: SourceCheckResult,
    unresolved_links: u64,
}

/// One profile owner. The application must hold its ProfileGuard until this
/// runtime and all users of `database` have been dropped.
pub struct Runtime {
    database: SharedDatabase,
    dependencies: RuntimeDependencies,
    fetcher: Mutex<Arc<dyn SourceFetcher>>,
    coordinator: Mutex<Coordinator>,
    data_operation: Arc<DataOperationGate>,
    redirects: Arc<RedirectPipeline>,
    completions: mpsc::UnboundedSender<CompletedCheck>,
    completion_receiver: Mutex<Option<mpsc::UnboundedReceiver<CompletedCheck>>>,
    schedule_changed: Notify,
    stop_requested: AtomicBool,
    stopped: AtomicBool,
    running: AtomicBool,
    stop_changed: Notify,
    stopped_changed: Notify,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    // Dropped after the database even if the host's managed-state drop order
    // differs from declaration order. The runtime needs no platform dependency.
    profile_owner: Mutex<Option<Arc<dyn Send + Sync>>>,
}

impl Runtime {
    pub fn new(
        database: SharedDatabase,
        dependencies: RuntimeDependencies,
    ) -> Result<Arc<Self>, String> {
        let (settings, source_states, ready) = {
            let database = database
                .lock()
                .map_err(|_| "database lock is unavailable")?;
            (
                database.settings().map_err(|error| error.to_string())?,
                database
                    .source_statuses()
                    .map_err(|error| error.to_string())?,
                !database
                    .initial_sync_choice_required()
                    .map_err(|error| error.to_string())?,
            )
        };
        let fetcher = (dependencies.fetcher_factory)(&settings)?;
        let redirects = Arc::new(RedirectPipeline::new(
            database.clone(),
            dependencies.resolver.clone(),
            dependencies.observer.clone(),
        ));
        let (completions, completion_receiver) = mpsc::unbounded_channel();
        Ok(Arc::new(Self {
            database,
            dependencies,
            fetcher: Mutex::new(fetcher),
            coordinator: Mutex::new(Coordinator::new(
                &settings,
                source_states,
                ready,
                Instant::now(),
            )),
            data_operation: Arc::new(DataOperationGate::default()),
            redirects,
            completions,
            completion_receiver: Mutex::new(Some(completion_receiver)),
            schedule_changed: Notify::new(),
            stop_requested: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            running: AtomicBool::new(false),
            stop_changed: Notify::new(),
            stopped_changed: Notify::new(),
            tasks: Mutex::new(Vec::new()),
            profile_owner: Mutex::new(None),
        }))
    }

    pub fn database(&self) -> &SharedDatabase {
        &self.database
    }

    pub fn retain_profile_guard<G: Send + Sync + 'static>(&self, guard: Arc<G>) {
        *self
            .profile_owner
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(guard);
    }

    pub fn data_operation(&self) -> Arc<DataOperationGate> {
        self.data_operation.clone()
    }

    pub fn source_states(&self) -> Result<Vec<SourceStatus>, String> {
        Ok(self
            .coordinator
            .lock()
            .map_err(|_| "monitor status lock is unavailable")?
            .states())
    }

    pub fn validate_settings(&self, settings: &AppSettings) -> Result<(), String> {
        (self.dependencies.fetcher_factory)(settings).map(|_| ())
    }

    /// Rebuild the HTTP client only after settings have been validated/committed
    /// by the platform settings service. Existing in-flight checks retain theirs.
    pub fn reconfigure(&self) -> Result<(), String> {
        let (settings, ready) = {
            let database = self
                .database
                .lock()
                .map_err(|_| "database lock is unavailable")?;
            (
                database.settings().map_err(|error| error.to_string())?,
                !database
                    .initial_sync_choice_required()
                    .map_err(|error| error.to_string())?,
            )
        };
        let fetcher = (self.dependencies.fetcher_factory)(&settings)?;
        *self
            .fetcher
            .lock()
            .map_err(|_| "HTTP client lock is unavailable")? = fetcher;
        let mut coordinator = self
            .coordinator
            .lock()
            .map_err(|_| "monitor status lock is unavailable")?;
        if coordinator.reconfigure(&settings, ready, Instant::now()) {
            self.persist_source_states(coordinator.states());
        }
        drop(coordinator);
        self.schedule_changed.notify_one();
        self.emit(RuntimeEvent::MonitorChanged);
        Ok(())
    }

    pub async fn check_now(self: &Arc<Self>) -> Result<CheckStartResult, String> {
        self.start_source_checks(true)
    }

    pub fn clear_giveaway_data(&self) -> Result<(), String> {
        let _guard = self
            .data_operation
            .try_acquire()
            .ok_or("data operation is already running")?;
        self.redirects.invalidate();
        let mut coordinator = self
            .coordinator
            .lock()
            .map_err(|_| "monitor status lock is unavailable")?;
        coordinator.reset(Instant::now());
        let mut database = self
            .database
            .lock()
            .map_err(|_| "database lock is unavailable")?;
        database
            .clear_giveaway_data()
            .map_err(|error| error.to_string())?;
        self.redirects.invalidate();
        for status in coordinator.states() {
            database
                .save_source_status(&status)
                .map_err(|error| error.to_string())?;
        }
        drop(database);
        drop(coordinator);
        self.schedule_changed.notify_one();
        self.emit(RuntimeEvent::HistoryChanged);
        self.emit(RuntimeEvent::MonitorChanged);
        Ok(())
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }

    pub fn request_shutdown(&self) {
        self.stop_requested.store(true, Ordering::Release);
        self.stop_changed.notify_waiters();
        self.schedule_changed.notify_one();
    }

    pub async fn shutdown(&self) {
        self.request_shutdown();
        loop {
            let stopped = self.stopped_changed.notified();
            tokio::pin!(stopped);
            stopped.as_mut().enable();
            if self.is_stopped() {
                return;
            }
            // A not-yet-started runtime has no network tasks to stop. Its future
            // will observe stop_requested before launching any work.
            if !self.running.load(Ordering::Acquire) {
                return;
            }
            stopped.await;
        }
    }

    pub async fn run(self: Arc<Self>, mode: RunMode) -> Result<RunReport, String> {
        if self.running.swap(true, Ordering::AcqRel) {
            return Err("monitor loop is already running".to_owned());
        }
        let receiver = self
            .completion_receiver
            .lock()
            .map_err(|_| "completion queue is unavailable")?
            .take();
        let Some(mut receiver) = receiver else {
            self.running.store(false, Ordering::Release);
            return Err("monitor loop cannot be restarted".to_owned());
        };
        let result = self.run_loop(mode, &mut receiver).await;
        self.stop_requested.store(true, Ordering::Release);
        self.redirects.invalidate();
        if let Ok(mut coordinator) = self.coordinator.lock() {
            coordinator.stop();
            self.persist_source_states(coordinator.states());
        }
        let tasks = self
            .tasks
            .lock()
            .map(|mut tasks| std::mem::take(&mut *tasks))
            .unwrap_or_default();
        for task in &tasks {
            task.abort();
        }
        for task in tasks {
            let _ = task.await;
        }
        self.stopped.store(true, Ordering::Release);
        self.running.store(false, Ordering::Release);
        self.stopped_changed.notify_waiters();
        result
    }

    async fn run_loop(
        self: &Arc<Self>,
        mode: RunMode,
        receiver: &mut mpsc::UnboundedReceiver<CompletedCheck>,
    ) -> Result<RunReport, String> {
        let mut report = RunReport::default();
        if mode == RunMode::Once && !self.stop_requested.load(Ordering::Acquire) {
            self.start_source_checks(true)?;
        }
        let mut enrich_tick = tokio::time::interval(Duration::from_secs(60));
        loop {
            if self.stop_requested.load(Ordering::Acquire) {
                report.interrupted = true;
                return Ok(report);
            }
            if mode == RunMode::Watch {
                self.start_source_checks(false)?;
            } else if !self
                .coordinator
                .lock()
                .map_err(|_| "monitor status lock is unavailable")?
                .has_running()
            {
                return Ok(report);
            }
            let next = self
                .coordinator
                .lock()
                .map_err(|_| "monitor status lock is unavailable")?
                .next_deadline();
            let delay = next.map_or(Duration::from_secs(60), |due| {
                due.saturating_duration_since(Instant::now())
            });
            tokio::select! {
                biased;
                () = self.stop_changed.notified() => {},
                completed = receiver.recv() => {
                    let Some(completed) = completed else { return Ok(report); };
                    let mut ready = vec![completed];
                    while let Ok(completed) = receiver.try_recv() { ready.push(completed); }
                    ready.sort_by_key(|completed| coordinator::priority(completed.token.source));
                    if !self.data_operation.is_available() {
                        for completed in ready { let _ = self.completions.send(completed); }
                        // Updates may hold the short-commit gate during their
                        // download. Do not spin on our own completion queue.
                        let stopped = self.stop_changed.notified();
                        tokio::pin!(stopped);
                        stopped.as_mut().enable();
                        if !self.stop_requested.load(Ordering::Acquire) {
                            tokio::select! {
                                biased;
                                () = stopped => {},
                                () = self.data_operation.wait_available() => {},
                            }
                        }
                        continue;
                    }
                    for completed in ready {
                        if let Some(completed) = self.apply_completed_check(completed)? {
                            report.completions.push(completed);
                        }
                    }
                    // Watch is unbounded; the observer owns streaming output.
                    if mode == RunMode::Watch { report.completions.clear(); }
                },
                () = self.schedule_changed.notified() => {},
                () = tokio::time::sleep(delay), if mode == RunMode::Watch => {},
                _ = enrich_tick.tick(), if mode == RunMode::Watch => {
                    let redirects = self.redirects.clone();
                    self.spawn(async move { redirects.enrich_history().await; });
                },
            }
        }
    }

    fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) {
        let mut tasks = self.tasks.lock().unwrap_or_else(|error| error.into_inner());
        if self.stop_requested.load(Ordering::Acquire) {
            return;
        }
        tasks.retain(|task| !task.is_finished());
        tasks.push(tokio::spawn(task));
    }

    fn start_source_checks(self: &Arc<Self>, manual: bool) -> Result<CheckStartResult, String> {
        if self.stop_requested.load(Ordering::Acquire) {
            return Err("monitor is shutting down".to_owned());
        }
        self.reconcile_monitor()?;
        if manual
            && self
                .database
                .lock()
                .map_err(|_| "database lock is unavailable")?
                .initial_sync_choice_required()
                .map_err(|error| error.to_string())?
        {
            return Err("initial synchronization mode must be selected first".to_owned());
        }
        let fetcher = self
            .fetcher
            .lock()
            .map_err(|_| "HTTP client lock is unavailable")?
            .clone();
        let (report, launches) = {
            let mut coordinator = self
                .coordinator
                .lock()
                .map_err(|_| "monitor status lock is unavailable")?;
            if self.stop_requested.load(Ordering::Acquire) {
                return Err("monitor is shutting down".to_owned());
            }
            let (report, launches) = coordinator.start(manual, Instant::now());
            if !launches.is_empty() {
                self.persist_source_states(coordinator.states());
            }
            (report, launches)
        };
        for launch in launches {
            let fetcher = fetcher.clone();
            let sender = self.completions.clone();
            let redirects = self.redirects.clone();
            self.spawn(async move {
                let _slot = tokio::select! {
                    biased;
                    () = launch.cancel.notified() => return,
                    slot = launch.request_slot.acquire() => match slot { Ok(slot) => slot, Err(_) => return },
                };
                tokio::select! {
                    biased;
                    () = launch.cancel.notified() => {},
                    (result, unresolved_links) = async {
                        let result = fetcher.check(launch.token.source).await;
                        redirects.prepare_source_check(result).await
                    } => { let _ = sender.send(CompletedCheck { token: launch.token, result, unresolved_links }); }
                }
            });
        }
        if !report.started_sources.is_empty() {
            self.emit(RuntimeEvent::MonitorChanged);
            self.schedule_changed.notify_one();
        }
        Ok(report)
    }

    fn reconcile_monitor(&self) -> Result<(), String> {
        let (settings, ready) = {
            let database = self
                .database
                .lock()
                .map_err(|_| "database lock is unavailable")?;
            (
                database.settings().map_err(|error| error.to_string())?,
                !database
                    .initial_sync_choice_required()
                    .map_err(|error| error.to_string())?,
            )
        };
        let mut coordinator = self
            .coordinator
            .lock()
            .map_err(|_| "monitor status lock is unavailable")?;
        if coordinator.reconfigure(&settings, ready, Instant::now()) {
            self.persist_source_states(coordinator.states());
        }
        Ok(())
    }

    fn persist_source_states(&self, states: Vec<SourceStatus>) {
        match self.database.lock() {
            Ok(database) => {
                for state in states {
                    if let Err(error) = database.save_source_status(&state) {
                        log::error!("could not persist source status: {error}");
                    }
                }
            }
            Err(error) => log::error!("could not lock database to persist source status: {error}"),
        }
    }

    fn apply_completed_check(
        &self,
        completed: CompletedCheck,
    ) -> Result<Option<SourceCompletion>, String> {
        let Some(_guard) = self.data_operation.try_acquire() else {
            let _ = self.completions.send(completed);
            return Ok(None);
        };
        let mut coordinator = self
            .coordinator
            .lock()
            .map_err(|_| "monitor status lock is unavailable")?;
        if !coordinator.accepts(completed.token) {
            return Ok(None);
        }
        let result = match self.database.lock() {
            Ok(mut database) => match database.settings() {
                Ok(settings) if !coordinator::enabled(&settings, completed.token.source) => {
                    return Ok(None);
                }
                Err(error) => Err(error.to_string()),
                Ok(_) => apply_source_check(&mut database, completed.result)
                    .map_err(|error| error.to_string()),
            },
            Err(_) => Err("database lock is unavailable".to_owned()),
        };
        let failure = match &result {
            Ok(outcome) => outcome.failures.first().cloned(),
            Err(message) => Some(SourceFailure {
                source: completed.token.source,
                message: message.clone(),
                reason: SourceFailureReason::Unknown,
            }),
        };
        let new_items = result
            .as_ref()
            .map_or(0, |outcome| outcome.new_items.len() as u64);
        coordinator.finish(
            completed.token,
            failure,
            new_items,
            completed.unresolved_links,
            Instant::now(),
        );
        self.persist_source_states(coordinator.states());
        drop(coordinator);
        let mut delivery_error = None;
        let delivery = if result.is_ok() {
            match dispatch_pending_notifications(&self.database, |record, settings| {
                let delivered = (self.dependencies.notification_sink)(record, settings);
                if let Err(error) = &delivered {
                    delivery_error = Some(error.clone());
                }
                delivered
            }) {
                Ok(report) => report,
                Err(error) => {
                    log::error!("pending notification delivery could not complete: {error}");
                    delivery_error = Some(error.to_string());
                    DeliveryReport::default()
                }
            }
        } else {
            DeliveryReport::default()
        };
        for (level, message) in check_result_diagnostics(&result) {
            log::log!(level, "{message}");
        }
        let result = SourceCompletion {
            source: completed.token.source,
            outcome: result,
            unresolved_links: completed.unresolved_links,
            delivery,
        };
        self.emit(RuntimeEvent::HistoryChanged);
        self.emit(RuntimeEvent::MonitorChanged);
        self.emit(RuntimeEvent::SourceCompleted(result.clone()));
        if self.dependencies.fail_on_delivery_error
            && let Some(error) = delivery_error
        {
            return Err(error);
        }
        Ok(Some(result))
    }

    fn emit(&self, event: RuntimeEvent) {
        (self.dependencies.observer)(event);
    }
}

pub fn now_utc() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .expect("RFC 3339 timestamp formatting should succeed")
}

pub fn source_failure_description(failure: &SourceFailure) -> String {
    let source = match failure.source {
        GiveawaySource::GiveeClub => "Givee Club",
        GiveawaySource::GamesForFarm => "Games for Farm",
        GiveawaySource::Zeros => "Zeros",
        GiveawaySource::FreeSteamKeys => "FreeSteamKeys",
    };
    let message = failure.message.replace('\r', "\\r").replace('\n', "\\n");
    format!("{source}: {message}")
}

pub fn check_result_diagnostics(
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
            messages.push((log::Level::Info, format!("giveaway check finished: new_items={}, failures={}, initial_sync={}, baseline_deferred={}", outcome.new_items.len(), outcome.failures.len(), outcome.is_initial_synchronization, outcome.baseline_deferred)));
            messages
        }
        Err(error) => vec![(log::Level::Error, format!("giveaway check failed: {error}"))],
    }
}

#[cfg(test)]
mod tests;
