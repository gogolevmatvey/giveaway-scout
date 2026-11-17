use std::sync::atomic::{AtomicUsize, Ordering};

use giveaway_scout_core::{GameStore, GiveawayItem, GiveawayKind, InitialSyncMode, SourcePhase};

use super::*;

struct NoResolver;
impl RedirectResolver for NoResolver {
    fn resolve_batch(&self, urls: Vec<String>) -> RedirectLookup<'_> {
        Box::pin(async move {
            urls.into_iter()
                .map(|url| (url, Err("fixture has no redirect".to_owned())))
                .collect()
        })
    }
}

struct StaticFetcher(Vec<GiveawayItem>);
impl SourceFetcher for StaticFetcher {
    fn check(&self, source: GiveawaySource) -> SourceFuture<'_> {
        Box::pin(async move {
            SourceCheckResult {
                checked_sources: vec![source],
                items: self
                    .0
                    .iter()
                    .filter(|item| item.source == source)
                    .cloned()
                    .collect(),
                failures: vec![],
            }
        })
    }
}

fn item(source: GiveawaySource, id: &str) -> GiveawayItem {
    GiveawayItem {
        source,
        source_id: id.to_owned(),
        title: format!("Fixture {id}"),
        url: format!("https://givee.club/en/event/{id}"),
        kind: GiveawayKind::Giveaway,
        store: GameStore::Steam,
        details: None,
    }
}

fn database(mode: Option<InitialSyncMode>) -> SharedDatabase {
    let database = Database::open_in_memory().unwrap();
    let settings = AppSettings {
        initial_sync_mode: mode,
        givee_club_enabled: true,
        games_for_farm_enabled: false,
        zeros_enabled: false,
        free_steam_keys_enabled: false,
        ..AppSettings::default()
    };
    database.update_settings(&settings).unwrap();
    Arc::new(Mutex::new(database))
}

fn dependencies(
    fetcher: Arc<dyn SourceFetcher>,
    sink: NotificationSink,
    observer: RuntimeObserver,
) -> RuntimeDependencies {
    RuntimeDependencies {
        fetcher_factory: Arc::new(move |_| Ok(fetcher.clone())),
        resolver: Arc::new(NoResolver),
        notification_sink: sink,
        observer,
        fail_on_delivery_error: false,
    }
}

#[tokio::test]
async fn one_shot_shares_ingestion_delivery_and_unread_state() {
    let database = database(Some(InitialSyncMode::NotifyCurrent));
    let delivered = Arc::new(Mutex::new(Vec::new()));
    let sent = delivered.clone();
    let observer_database = database.clone();
    let runtime = Runtime::new(
        database.clone(),
        dependencies(
            Arc::new(StaticFetcher(vec![item(GiveawaySource::GiveeClub, "42")])),
            Arc::new(move |record, _| {
                sent.lock().unwrap().push(record.id.clone());
                Ok(())
            }),
            Arc::new(move |_| {
                assert!(
                    observer_database.try_lock().is_ok(),
                    "observers must not hold the database lock"
                );
            }),
        ),
    )
    .unwrap();
    let report = runtime.clone().run(RunMode::Once).await.unwrap();
    assert!(!report.interrupted);
    assert_eq!(report.completions.len(), 1);
    assert_eq!(report.completions[0].delivery.delivered, 1);
    assert_eq!(delivered.lock().unwrap().len(), 1);
    let database = database.lock().unwrap();
    assert!(database.pending_notifications().unwrap().is_empty());
    assert!(database.notifications().unwrap()[0].read_at_utc.is_none());
    let status = database
        .source_statuses()
        .unwrap()
        .into_iter()
        .find(|status| status.source == GiveawaySource::GiveeClub)
        .unwrap();
    assert_eq!(status.phase, SourcePhase::Ok);
    assert!(status.last_successful_check_utc.is_some());
    assert!(runtime.is_stopped());
}

#[tokio::test]
async fn silent_baseline_survives_a_new_runtime_and_only_later_items_are_delivered() {
    let database = database(Some(InitialSyncMode::Silent));
    let delivered = Arc::new(AtomicUsize::new(0));
    for items in [
        vec![item(GiveawaySource::GiveeClub, "42")],
        vec![
            item(GiveawaySource::GiveeClub, "42"),
            item(GiveawaySource::GiveeClub, "43"),
        ],
    ] {
        let sent = delivered.clone();
        let runtime = Runtime::new(
            database.clone(),
            dependencies(
                Arc::new(StaticFetcher(items)),
                Arc::new(move |_, _| {
                    sent.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }),
                Arc::new(|_| {}),
            ),
        )
        .unwrap();
        runtime.run(RunMode::Once).await.unwrap();
    }
    assert_eq!(delivered.load(Ordering::SeqCst), 1);
    let database = database.lock().unwrap();
    assert_eq!(database.counts().unwrap().tracked_giveaways, 2);
    assert_eq!(database.notifications().unwrap().len(), 1);
    assert_eq!(database.notifications().unwrap()[0].title, "Fixture 43");
}

struct ActiveGuard(Arc<AtomicUsize>);
impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

struct SlowFetcher {
    started: Arc<Notify>,
    release: Arc<Notify>,
    active: Arc<AtomicUsize>,
}
impl SourceFetcher for SlowFetcher {
    fn check(&self, source: GiveawaySource) -> SourceFuture<'_> {
        Box::pin(async move {
            if source == GiveawaySource::GiveeClub {
                self.active.fetch_add(1, Ordering::SeqCst);
                let _guard = ActiveGuard(self.active.clone());
                self.started.notify_one();
                self.release.notified().await;
            }
            SourceCheckResult {
                checked_sources: vec![source],
                items: vec![item(
                    source,
                    if source == GiveawaySource::GiveeClub {
                        "42"
                    } else {
                        "43"
                    },
                )],
                failures: vec![],
            }
        })
    }
}

#[tokio::test]
async fn fast_source_commits_independently_and_shutdown_cancels_slow_http() {
    let database = database(Some(InitialSyncMode::NotifyCurrent));
    {
        let database = database.lock().unwrap();
        let mut settings = database.settings().unwrap();
        settings.free_steam_keys_enabled = true;
        database.update_settings(&settings).unwrap();
    }
    let started = Arc::new(Notify::new());
    let active = Arc::new(AtomicUsize::new(0));
    let fast_completed = Arc::new(Notify::new());
    let observed = fast_completed.clone();
    let runtime = Runtime::new(
        database.clone(),
        dependencies(
            Arc::new(SlowFetcher {
                started: started.clone(),
                release: Arc::new(Notify::new()),
                active: active.clone(),
            }),
            Arc::new(|_, _| Ok(())),
            Arc::new(move |event| {
                if matches!(
                    event,
                    RuntimeEvent::SourceCompleted(SourceCompletion {
                        source: GiveawaySource::FreeSteamKeys,
                        ..
                    })
                ) {
                    observed.notify_one();
                }
            }),
        ),
    )
    .unwrap();
    let monitor = tokio::spawn(runtime.clone().run(RunMode::Watch));
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), fast_completed.notified())
        .await
        .unwrap();
    assert_eq!(active.load(Ordering::SeqCst), 1);
    assert_eq!(
        database.lock().unwrap().counts().unwrap().tracked_giveaways,
        1
    );
    tokio::time::timeout(Duration::from_secs(2), runtime.shutdown())
        .await
        .unwrap();
    let report = monitor.await.unwrap().unwrap();
    assert!(report.interrupted);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    let states = database.lock().unwrap().source_statuses().unwrap();
    let slow = states
        .iter()
        .find(|state| state.source == GiveawaySource::GiveeClub)
        .unwrap();
    assert_eq!(slow.phase, SourcePhase::Waiting);
    assert!(slow.last_successful_check_utc.is_none());
    assert!(
        states
            .iter()
            .all(|state| state.phase != SourcePhase::Checking)
    );
}

#[tokio::test]
async fn disabling_an_inflight_source_prevents_late_ingestion() {
    let database = database(Some(InitialSyncMode::NotifyCurrent));
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let active = Arc::new(AtomicUsize::new(0));
    let runtime = Runtime::new(
        database.clone(),
        dependencies(
            Arc::new(SlowFetcher {
                started: started.clone(),
                release: release.clone(),
                active: active.clone(),
            }),
            Arc::new(|_, _| Ok(())),
            Arc::new(|_| {}),
        ),
    )
    .unwrap();
    let monitor = tokio::spawn(runtime.clone().run(RunMode::Watch));
    started.notified().await;
    {
        let database = database.lock().unwrap();
        let mut settings = database.settings().unwrap();
        settings.givee_club_enabled = false;
        database.update_settings(&settings).unwrap();
    }
    runtime.reconfigure().unwrap();
    release.notify_one();
    runtime.shutdown().await;
    monitor.await.unwrap().unwrap();
    assert_eq!(active.load(Ordering::SeqCst), 0);
    assert_eq!(
        database.lock().unwrap().counts().unwrap().tracked_giveaways,
        0
    );
    assert_eq!(
        runtime
            .source_states()
            .unwrap()
            .iter()
            .find(|state| state.source == GiveawaySource::GiveeClub)
            .unwrap()
            .phase,
        SourcePhase::Disabled
    );
}

#[tokio::test]
async fn failed_console_delivery_stops_without_marking_the_record_delivered_or_read() {
    let database = database(Some(InitialSyncMode::NotifyCurrent));
    let mut dependencies = dependencies(
        Arc::new(StaticFetcher(vec![item(GiveawaySource::GiveeClub, "42")])),
        Arc::new(|_, _| Err("stdout flush failed".to_owned())),
        Arc::new(|_| {}),
    );
    dependencies.fail_on_delivery_error = true;
    let runtime = Runtime::new(database.clone(), dependencies).unwrap();
    assert_eq!(
        runtime.clone().run(RunMode::Once).await.unwrap_err(),
        "stdout flush failed"
    );
    assert!(runtime.is_stopped());
    let database = database.lock().unwrap();
    assert_eq!(database.pending_notifications().unwrap().len(), 1);
    assert!(database.notifications().unwrap()[0].read_at_utc.is_none());
}

#[tokio::test]
async fn one_shot_requires_the_hosts_explicit_initial_sync_policy() {
    let runtime = Runtime::new(
        database(None),
        dependencies(
            Arc::new(StaticFetcher(vec![])),
            Arc::new(|_, _| Ok(())),
            Arc::new(|_| {}),
        ),
    )
    .unwrap();
    assert!(
        runtime
            .clone()
            .run(RunMode::Once)
            .await
            .unwrap_err()
            .contains("initial synchronization")
    );
    assert!(runtime.is_stopped());
}

struct BlockingResolver {
    started: Arc<Notify>,
    release: Arc<Notify>,
}

struct SignallingFetcher(Arc<Notify>);
impl SourceFetcher for SignallingFetcher {
    fn check(&self, source: GiveawaySource) -> SourceFuture<'_> {
        Box::pin(async move {
            self.0.notify_one();
            SourceCheckResult {
                checked_sources: vec![source],
                items: vec![item(source, "42")],
                failures: vec![],
            }
        })
    }
}

#[tokio::test]
async fn shutdown_does_not_wait_for_an_updater_holding_the_commit_gate() {
    let database = database(Some(InitialSyncMode::NotifyCurrent));
    let fetched = Arc::new(Notify::new());
    let runtime = Runtime::new(
        database.clone(),
        dependencies(
            Arc::new(SignallingFetcher(fetched.clone())),
            Arc::new(|_, _| Ok(())),
            Arc::new(|_| {}),
        ),
    )
    .unwrap();
    let gate = runtime.data_operation();
    let _updater = gate.try_acquire().unwrap();
    let monitor = tokio::spawn(runtime.clone().run(RunMode::Once));
    fetched.notified().await;
    tokio::task::yield_now().await;
    tokio::time::timeout(Duration::from_secs(2), runtime.shutdown())
        .await
        .unwrap();
    assert!(monitor.await.unwrap().unwrap().interrupted);
    assert_eq!(
        database.lock().unwrap().counts().unwrap().tracked_giveaways,
        0
    );
}
impl RedirectResolver for BlockingResolver {
    fn resolve_batch(&self, urls: Vec<String>) -> RedirectLookup<'_> {
        Box::pin(async move {
            self.started.notify_one();
            self.release.notified().await;
            urls.into_iter()
                .map(|url| (url, Ok("https://givee.club/en/event/42".to_owned())))
                .collect()
        })
    }
}

#[tokio::test]
async fn redirect_results_read_before_cleanup_cannot_recreate_cleared_cache() {
    let database = database(Some(InitialSyncMode::NotifyCurrent));
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let pipeline = Arc::new(RedirectPipeline::new(
        database.clone(),
        Arc::new(BlockingResolver {
            started: started.clone(),
            release: release.clone(),
        }),
        Arc::new(|_| {}),
    ));
    let mut giveaway = item(GiveawaySource::FreeSteamKeys, "42");
    giveaway.url = "https://www.freesteamkeys.com/open/42/".to_owned();
    let url = giveaway.url.clone();
    let active_pipeline = pipeline.clone();
    let check = tokio::spawn(async move {
        active_pipeline
            .prepare_source_check(SourceCheckResult {
                checked_sources: vec![giveaway.source],
                items: vec![giveaway],
                failures: vec![],
            })
            .await
    });
    started.notified().await;
    pipeline.invalidate();
    database.lock().unwrap().clear_giveaway_data().unwrap();
    pipeline.invalidate();
    release.notify_one();
    let (_, unresolved) = check.await.unwrap();
    assert_eq!(unresolved, 1);
    assert!(
        database
            .lock()
            .unwrap()
            .cached_redirect(&url)
            .unwrap()
            .is_none()
    );
}
