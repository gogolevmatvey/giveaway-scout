use std::{
    collections::HashSet,
    future::Future,
    pin::Pin,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
};

use giveaway_scout_core::{
    OriginResolver, RedirectTransport, SourceCheckResult, is_free_steam_keys_redirect,
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio::sync::Notify;

use crate::{RuntimeEvent, RuntimeObserver, SharedDatabase};

pub type RedirectLookup<'a> =
    Pin<Box<dyn Future<Output = Vec<(String, Result<String, String>)>> + Send + 'a>>;

pub trait RedirectResolver: Send + Sync {
    fn resolve_batch(&self, urls: Vec<String>) -> RedirectLookup<'_>;
}

struct ResolverAdapter<T>(OriginResolver<T>);

impl<T: RedirectTransport> RedirectResolver for ResolverAdapter<T> {
    fn resolve_batch(&self, urls: Vec<String>) -> RedirectLookup<'_> {
        Box::pin(async move {
            self.0
                .resolve_batch(urls)
                .await
                .into_iter()
                .map(|(url, result)| (url, result.map_err(|error| error.to_string())))
                .collect()
        })
    }
}

pub fn resolver_from_transport<T: RedirectTransport + 'static>(
    transport: T,
) -> Arc<dyn RedirectResolver> {
    Arc::new(ResolverAdapter(OriginResolver::with_transport(transport)))
}

struct ProductionResolver(
    OnceLock<Result<OriginResolver, giveaway_scout_core::OriginResolveError>>,
);

impl RedirectResolver for ProductionResolver {
    fn resolve_batch(&self, urls: Vec<String>) -> RedirectLookup<'_> {
        Box::pin(async move {
            match self.0.get_or_init(OriginResolver::new) {
                Ok(resolver) => resolver
                    .resolve_batch(urls)
                    .await
                    .into_iter()
                    .map(|(url, result)| (url, result.map_err(|error| error.to_string())))
                    .collect(),
                Err(error) => urls
                    .into_iter()
                    .map(|url| (url, Err(error.to_string())))
                    .collect(),
            }
        })
    }
}

pub(crate) fn production_resolver() -> Arc<dyn RedirectResolver> {
    Arc::new(ProductionResolver(OnceLock::new()))
}

/// All counters and cancellation generations belong to one profile runtime.
/// A fixture or another profile never shares the production resolver permits.
pub(crate) struct RedirectPipeline {
    database: SharedDatabase,
    resolver: Arc<dyn RedirectResolver>,
    observer: RuntimeObserver,
    generation: AtomicU64,
    background_running: AtomicBool,
    active_feeds: AtomicUsize,
    feed_started: Notify,
}

struct FeedGuard<'a>(&'a AtomicUsize);
impl Drop for FeedGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

struct BackgroundGuard<'a>(&'a AtomicBool);
impl Drop for BackgroundGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl RedirectPipeline {
    pub(crate) fn new(
        database: SharedDatabase,
        resolver: Arc<dyn RedirectResolver>,
        observer: RuntimeObserver,
    ) -> Self {
        Self {
            database,
            resolver,
            observer,
            generation: AtomicU64::new(0),
            background_running: AtomicBool::new(false),
            active_feeds: AtomicUsize::new(0),
            feed_started: Notify::new(),
        }
    }

    pub(crate) fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) async fn prepare_source_check(
        &self,
        result: SourceCheckResult,
    ) -> (SourceCheckResult, u64) {
        if !result.failures.is_empty() {
            return (result, 0);
        }
        let mut seen = HashSet::new();
        let urls: Vec<_> = result
            .items
            .iter()
            .map(|item| item.url.clone())
            .filter(|url| is_free_steam_keys_redirect(url))
            .filter(|url| seen.insert(url.clone()))
            .collect();
        if urls.is_empty() {
            return (result, 0);
        }
        self.active_feeds.fetch_add(1, Ordering::AcqRel);
        self.feed_started.notify_waiters();
        let _active = FeedGuard(&self.active_feeds);
        let generation = self.generation.load(Ordering::Acquire);
        let due: Vec<String> = match self.database.lock() {
            Ok(database) => urls
                .iter()
                .filter(|url| match database.cached_redirect(url) {
                    Ok(Some(entry)) if entry.url.is_some() => false,
                    Ok(Some(entry)) => retry_due(entry.retry_after_utc.as_deref()),
                    _ => true,
                })
                .cloned()
                .collect(),
            Err(_) => return (result, urls.len() as u64),
        };
        if !due.is_empty() {
            let resolved = self.resolver.resolve_batch(due).await;
            self.store_results(generation, resolved);
        }
        let unresolved = match self.database.lock() {
            Ok(database) => urls.iter().filter(|url| !matches!(database.cached_redirect(url), Ok(Some(entry)) if entry.url.is_some())).count() as u64,
            Err(_) => urls.len() as u64,
        };
        (result, unresolved)
    }

    pub(crate) async fn enrich_history(&self) {
        if self.active_feeds.load(Ordering::Acquire) > 0
            || self
                .background_running
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return;
        }
        let _background = BackgroundGuard(&self.background_running);
        let feed_started = self.feed_started.notified();
        tokio::pin!(feed_started);
        feed_started.as_mut().enable();
        let generation = self.generation.load(Ordering::Acquire);
        let urls = {
            let Ok(database) = self.database.lock() else {
                return;
            };
            match database.pending_redirects(2) {
                Ok(urls) => urls
                    .into_iter()
                    .filter(|url| is_free_steam_keys_redirect(url))
                    .collect::<Vec<_>>(),
                Err(error) => {
                    log::warn!("could not load pending origin lookups: {error}");
                    return;
                }
            }
        };
        if urls.is_empty() || self.active_feeds.load(Ordering::Acquire) > 0 {
            return;
        }
        let resolved = tokio::select! {
            biased;
            () = &mut feed_started => return,
            resolved = self.resolver.resolve_batch(urls) => resolved,
        };
        self.store_results(generation, resolved);
    }

    fn store_results(&self, generation: u64, resolved: Vec<(String, Result<String, String>)>) {
        let Ok(mut database) = self.database.lock() else {
            return;
        };
        // Cleanup invalidates before taking this same DB mutex, and again before
        // releasing it. A source commit only makes us wait; it cannot drop URLs.
        if self.generation.load(Ordering::Acquire) != generation {
            return;
        }
        let retry_after = (OffsetDateTime::now_utc() + time::Duration::minutes(15))
            .format(&Rfc3339)
            .ok();
        let mut changed = false;
        for (url, result) in resolved {
            let outcome = match result {
                Ok(destination) => {
                    changed = true;
                    database.apply_resolved_url(&url, &destination)
                }
                Err(error) => {
                    log::debug!("origin lookup deferred for {url}: {error}");
                    database.cache_redirect(&url, None, retry_after.as_deref())
                }
            };
            if let Err(error) = outcome {
                log::warn!("could not save origin lookup: {error}");
            }
        }
        drop(database);
        if changed {
            (self.observer)(RuntimeEvent::HistoryChanged);
        }
    }
}

fn retry_due(value: Option<&str>) -> bool {
    value
        .and_then(|value| OffsetDateTime::parse(value, &Rfc3339).ok())
        .is_none_or(|deadline| deadline <= OffsetDateTime::now_utc())
}
