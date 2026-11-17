//! Source scheduling has no network or database locks. Each request carries a
//! generation token so disabling a source or clearing history cancels old work.
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use giveaway_scout_core::{AppSettings, GiveawaySource, SourceFailure, SourcePhase, SourceStatus};
use serde::Serialize;
use tokio::sync::{Notify, Semaphore};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckStartResult {
    pub started_sources: Vec<GiveawaySource>,
    pub already_running_sources: Vec<GiveawaySource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RunToken {
    pub source: GiveawaySource,
    epoch: u64,
    revision: u64,
    id: u64,
}

pub(crate) struct Launch {
    pub token: RunToken,
    pub cancel: Arc<Notify>,
    pub request_slot: Arc<Semaphore>,
}

struct Entry {
    status: SourceStatus,
    enabled: bool,
    interval: Duration,
    revision: u64,
    running: Option<RunToken>,
    cancel: Option<Arc<Notify>>,
    due: Option<Instant>,
    completed: Option<Instant>,
    request_slot: Arc<Semaphore>,
}

pub(crate) struct Coordinator {
    entries: Vec<Entry>,
    ready: bool,
    epoch: u64,
    next_id: u64,
}

pub(crate) fn enabled(settings: &AppSettings, source: GiveawaySource) -> bool {
    match source {
        GiveawaySource::FreeSteamKeys => settings.free_steam_keys_enabled,
        GiveawaySource::GiveeClub => settings.givee_club_enabled,
        GiveawaySource::GamesForFarm => settings.games_for_farm_enabled,
        GiveawaySource::Zeros => settings.zeros_enabled,
    }
}

pub(crate) fn priority(source: GiveawaySource) -> usize {
    match source {
        GiveawaySource::FreeSteamKeys => 0,
        GiveawaySource::GiveeClub => 1,
        GiveawaySource::GamesForFarm => 2,
        GiveawaySource::Zeros => 3,
    }
}

impl Coordinator {
    pub(crate) fn has_running(&self) -> bool {
        self.entries.iter().any(|entry| entry.running.is_some())
    }

    pub(crate) fn stop(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        for entry in &mut self.entries {
            Self::cancel(entry);
            entry.due = None;
            entry.status.next_check_at_utc = None;
            entry.status.phase = if !entry.enabled {
                SourcePhase::Disabled
            } else if entry.status.last_error.is_some() {
                SourcePhase::Error
            } else if entry.status.last_successful_check_utc.is_some() {
                SourcePhase::Ok
            } else {
                SourcePhase::Waiting
            };
        }
    }

    pub(crate) fn new(
        settings: &AppSettings,
        saved: Vec<SourceStatus>,
        ready: bool,
        now: Instant,
    ) -> Self {
        let entries = GiveawaySource::ALL
            .into_iter()
            .map(|source| {
                let mut status = saved
                    .iter()
                    .find(|entry| entry.source == source)
                    .cloned()
                    .unwrap_or_else(|| SourceStatus::new(source));
                let source_enabled = enabled(settings, source);
                // Persisted Checking describes a previous process, never this one.
                status.phase = if !source_enabled {
                    SourcePhase::Disabled
                } else if status.last_error.is_some() {
                    SourcePhase::Error
                } else if status.last_successful_check_utc.is_some() {
                    SourcePhase::Ok
                } else {
                    SourcePhase::Waiting
                };
                status.next_check_at_utc = (source_enabled && ready).then(super::now_utc);
                Entry {
                    status,
                    enabled: source_enabled,
                    interval: Self::interval(settings, source),
                    revision: 0,
                    running: None,
                    cancel: None,
                    due: (source_enabled && ready).then_some(now),
                    completed: None,
                    request_slot: Arc::new(Semaphore::new(1)),
                }
            })
            .collect();
        let mut result = Self {
            entries,
            ready,
            epoch: 0,
            next_id: 0,
        };
        result
            .entries
            .sort_by_key(|entry| priority(entry.status.source));
        result
    }

    fn interval(settings: &AppSettings, source: GiveawaySource) -> Duration {
        Duration::from_secs(u64::from(settings.check_interval_for(source).max(5)) * 60)
    }

    pub(crate) fn reconfigure(
        &mut self,
        settings: &AppSettings,
        ready: bool,
        now: Instant,
    ) -> bool {
        let became_ready = ready && !self.ready;
        let mut changed = false;
        for entry in &mut self.entries {
            let interval = Self::interval(settings, entry.status.source);
            let interval_changed = interval != entry.interval;
            let source_enabled = enabled(settings, entry.status.source);
            if !source_enabled && entry.enabled {
                Self::cancel(entry);
                entry.status.phase = SourcePhase::Disabled;
                entry.due = None;
                entry.status.next_check_at_utc = None;
                changed = true;
            } else if source_enabled && !entry.enabled {
                entry.status.phase = SourcePhase::Waiting;
                entry.due = ready.then_some(now);
                // Re-enabling always requests a fresh immediate check, even if
                // an interval edit arrives before the scheduler can start it.
                entry.completed = None;
                entry.status.next_check_at_utc = ready.then(super::now_utc);
                changed = true;
            } else if source_enabled
                && entry.running.is_none()
                && (became_ready || (interval_changed && entry.completed.is_some()))
            {
                let due = if became_ready {
                    now
                } else {
                    entry.completed.unwrap_or(now) + interval
                };
                entry.due = ready.then_some(due);
                entry.status.next_check_at_utc =
                    ready.then(|| timestamp_after(due.saturating_duration_since(now)));
                changed = true;
            }
            entry.enabled = source_enabled;
            entry.interval = interval;
        }
        self.ready = ready;
        changed
    }

    fn cancel(entry: &mut Entry) {
        entry.revision = entry.revision.wrapping_add(1);
        entry.running = None;
        if let Some(cancel) = entry.cancel.take() {
            cancel.notify_one();
        }
    }

    pub(crate) fn start(&mut self, manual: bool, now: Instant) -> (CheckStartResult, Vec<Launch>) {
        let mut report = CheckStartResult::default();
        let mut launches = Vec::new();
        if !self.ready {
            return (report, launches);
        }
        for entry in &mut self.entries {
            if !entry.enabled {
                continue;
            }
            if entry.running.is_some() {
                if manual {
                    report.already_running_sources.push(entry.status.source);
                }
                continue;
            }
            if !manual && !entry.due.is_some_and(|due| due <= now) {
                continue;
            }
            self.next_id = self.next_id.wrapping_add(1);
            let token = RunToken {
                source: entry.status.source,
                epoch: self.epoch,
                revision: entry.revision,
                id: self.next_id,
            };
            let cancel = Arc::new(Notify::new());
            entry.running = Some(token);
            entry.cancel = Some(cancel.clone());
            entry.due = None;
            entry.status.phase = SourcePhase::Checking;
            entry.status.last_started_at_utc = Some(super::now_utc());
            entry.status.next_check_at_utc = None;
            report.started_sources.push(entry.status.source);
            launches.push(Launch {
                token,
                cancel,
                request_slot: entry.request_slot.clone(),
            });
        }
        (report, launches)
    }

    pub(crate) fn accepts(&self, token: RunToken) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.enabled && entry.running == Some(token) && self.epoch == token.epoch)
    }

    pub(crate) fn finish(
        &mut self,
        token: RunToken,
        failure: Option<SourceFailure>,
        new_items: u64,
        unresolved_links: u64,
        now: Instant,
    ) -> bool {
        if !self.accepts(token) {
            return false;
        }
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.status.source == token.source)
            .unwrap();
        let interval = entry.interval;
        entry.running = None;
        entry.cancel = None;
        entry.completed = Some(now);
        entry.due = Some(now + interval);
        entry.status.last_completed_at_utc = Some(super::now_utc());
        entry.status.next_check_at_utc = Some(timestamp_after(interval));
        entry.status.last_new_items = new_items;
        entry.status.unresolved_links = unresolved_links;
        entry.status.last_error = failure;
        if entry.status.last_error.is_some() {
            entry.status.phase = SourcePhase::Error;
        } else {
            entry.status.phase = SourcePhase::Ok;
            entry.status.last_successful_check_utc = entry.status.last_completed_at_utc.clone();
        }
        true
    }

    pub(crate) fn reset(&mut self, now: Instant) {
        self.epoch = self.epoch.wrapping_add(1);
        for entry in &mut self.entries {
            Self::cancel(entry);
            entry.status = SourceStatus::new(entry.status.source);
            entry.status.phase = if entry.enabled {
                SourcePhase::Waiting
            } else {
                SourcePhase::Disabled
            };
            entry.due = (entry.enabled && self.ready).then_some(now);
            entry.completed = None;
            entry.status.next_check_at_utc = entry.due.map(|_| super::now_utc());
        }
    }

    pub(crate) fn states(&self) -> Vec<SourceStatus> {
        self.entries
            .iter()
            .map(|entry| entry.status.clone())
            .collect()
    }

    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.entries.iter().filter_map(|entry| entry.due).min()
    }
}

fn timestamp_after(duration: Duration) -> String {
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};
    (OffsetDateTime::now_utc()
        + time::Duration::seconds(duration.as_secs().min(i64::MAX as u64) as i64))
    .format(&Rfc3339)
    .expect("RFC 3339 timestamp formatting should succeed")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn settings() -> AppSettings {
        AppSettings {
            givee_club_enabled: true,
            games_for_farm_enabled: true,
            zeros_enabled: true,
            free_steam_keys_enabled: true,
            ..AppSettings::default()
        }
    }
    #[test]
    fn fast_source_schedules_again_while_slow_source_is_still_checking() {
        let now = Instant::now();
        let mut coordinator = Coordinator::new(&settings(), vec![], true, now);
        let (report, launches) = coordinator.start(false, now);
        assert_eq!(report.started_sources.len(), 4);
        assert_eq!(report.started_sources[0], GiveawaySource::FreeSteamKeys);
        let fast = launches[0].token;
        assert!(coordinator.finish(fast, None, 1, 0, now));
        let (report, _) = coordinator.start(false, now + Duration::from_secs(60 * 60));
        assert_eq!(report.started_sources, vec![GiveawaySource::FreeSteamKeys]);
        assert!(
            coordinator
                .states()
                .iter()
                .find(|s| s.source == GiveawaySource::GiveeClub)
                .is_some_and(|s| s.phase == SourcePhase::Checking)
        );
    }
    #[test]
    fn repeated_manual_requests_share_running_source_tasks() {
        let now = Instant::now();
        let mut coordinator = Coordinator::new(&settings(), vec![], true, now);
        coordinator.start(true, now);
        let (report, launches) = coordinator.start(true, now);
        assert!(launches.is_empty());
        assert_eq!(report.already_running_sources.len(), 4);
    }
    #[test]
    fn disabled_reenabled_and_cleared_results_are_rejected() {
        let now = Instant::now();
        let mut settings = settings();
        let mut coordinator = Coordinator::new(&settings, vec![], true, now);
        let (_, launches) = coordinator.start(true, now);
        let old = launches[0].token;
        settings.free_steam_keys_enabled = false;
        coordinator.reconfigure(&settings, true, now);
        assert!(!coordinator.accepts(old));
        settings.free_steam_keys_enabled = true;
        coordinator.reconfigure(&settings, true, now);
        let (_, launches) = coordinator.start(false, now);
        let current = launches[0].token;
        assert_ne!(old, current);
        assert!(!coordinator.finish(old, None, 1, 0, now));
        coordinator.reset(now);
        assert!(!coordinator.finish(current, None, 1, 0, now));
    }
    #[test]
    fn sources_use_their_own_intervals_after_completion() {
        let now = Instant::now();
        let mut settings = settings();
        settings.source_check_intervals.free_steam_keys = 5;
        settings.source_check_intervals.givee_club = 90;
        let mut coordinator = Coordinator::new(&settings, vec![], true, now);
        let (_, launches) = coordinator.start(false, now);
        for launch in launches {
            assert!(coordinator.finish(launch.token, None, 0, 0, now));
        }
        assert_eq!(
            coordinator.next_deadline(),
            Some(now + Duration::from_secs(5 * 60))
        );
        let (report, _) = coordinator.start(false, now + Duration::from_secs(5 * 60));
        assert_eq!(report.started_sources, vec![GiveawaySource::FreeSteamKeys]);
        let (report, _) = coordinator.start(false, now + Duration::from_secs(60 * 60));
        assert_eq!(
            report.started_sources,
            vec![GiveawaySource::GamesForFarm, GiveawaySource::Zeros]
        );
        let (report, _) = coordinator.start(false, now + Duration::from_secs(90 * 60));
        assert_eq!(report.started_sources, vec![GiveawaySource::GiveeClub]);
    }
    #[test]
    fn changing_one_interval_preserves_other_source_deadlines() {
        let now = Instant::now();
        let mut settings = settings();
        let mut coordinator = Coordinator::new(&settings, vec![], true, now);
        let (_, launches) = coordinator.start(true, now);
        for launch in launches {
            assert!(coordinator.finish(launch.token, None, 0, 0, now));
        }
        let previous_deadlines: Vec<_> = coordinator
            .entries
            .iter()
            .skip(1)
            .map(|entry| (entry.due, entry.status.next_check_at_utc.clone()))
            .collect();
        settings.source_check_intervals.free_steam_keys = 5;
        assert!(coordinator.reconfigure(&settings, true, now + Duration::from_secs(10)));
        let other_deadlines: Vec<_> = coordinator
            .entries
            .iter()
            .skip(1)
            .map(|entry| (entry.due, entry.status.next_check_at_utc.clone()))
            .collect();
        assert_eq!(previous_deadlines, other_deadlines);
        let (report, _) = coordinator.start(false, now + Duration::from_secs(300));
        assert_eq!(report.started_sources, vec![GiveawaySource::FreeSteamKeys]);
    }
    #[test]
    fn changing_a_running_source_interval_applies_after_its_completion() {
        let now = Instant::now();
        let mut settings = settings();
        let mut coordinator = Coordinator::new(&settings, vec![], true, now);
        let (_, launches) = coordinator.start(true, now);
        let token = launches[0].token;
        settings.source_check_intervals.free_steam_keys = 5;
        coordinator.reconfigure(&settings, true, now + Duration::from_secs(10));
        assert!(coordinator.accepts(token));
        let (report, launches) = coordinator.start(true, now + Duration::from_secs(20));
        assert!(launches.is_empty());
        assert_eq!(report.already_running_sources.len(), 4);
        assert!(coordinator.finish(token, None, 0, 0, now + Duration::from_secs(30)));
        let (report, _) = coordinator.start(false, now + Duration::from_secs(329));
        assert!(report.started_sources.is_empty());
        let (report, _) = coordinator.start(false, now + Duration::from_secs(330));
        assert_eq!(report.started_sources, vec![GiveawaySource::FreeSteamKeys]);
    }
    #[test]
    fn interval_edits_preserve_the_first_check_after_enabling_a_source() {
        let now = Instant::now();
        let mut settings = settings();
        settings.free_steam_keys_enabled = false;
        let mut coordinator = Coordinator::new(&settings, vec![], true, now);
        coordinator.start(false, now);
        settings.free_steam_keys_enabled = true;
        coordinator.reconfigure(&settings, true, now + Duration::from_secs(10));
        settings.source_check_intervals.free_steam_keys = 120;
        coordinator.reconfigure(&settings, true, now + Duration::from_secs(20));
        assert_eq!(
            coordinator.next_deadline(),
            Some(now + Duration::from_secs(10))
        );
        let (report, _) = coordinator.start(false, now + Duration::from_secs(20));
        assert_eq!(report.started_sources, vec![GiveawaySource::FreeSteamKeys]);
    }
    #[test]
    fn interval_edits_preserve_the_immediate_check_when_reenabling_a_source() {
        let now = Instant::now();
        let mut settings = settings();
        let mut coordinator = Coordinator::new(&settings, vec![], true, now);
        let (_, launches) = coordinator.start(false, now);
        coordinator.finish(launches[0].token, None, 0, 0, now + Duration::from_secs(5));
        let previous_status = coordinator.states()[0].clone();
        settings.free_steam_keys_enabled = false;
        coordinator.reconfigure(&settings, true, now + Duration::from_secs(10));
        settings.free_steam_keys_enabled = true;
        coordinator.reconfigure(&settings, true, now + Duration::from_secs(20));
        settings.source_check_intervals.free_steam_keys = 120;
        coordinator.reconfigure(&settings, true, now + Duration::from_secs(30));
        assert_eq!(
            coordinator.next_deadline(),
            Some(now + Duration::from_secs(20))
        );
        let status = &coordinator.states()[0];
        assert_eq!(
            status.last_successful_check_utc,
            previous_status.last_successful_check_utc
        );
        assert_eq!(
            status.last_completed_at_utc,
            previous_status.last_completed_at_utc
        );
        let (report, _) = coordinator.start(false, now + Duration::from_secs(30));
        assert_eq!(report.started_sources, vec![GiveawaySource::FreeSteamKeys]);
    }
    #[test]
    fn interval_edits_preserve_pending_startup_and_clear_checks() {
        let now = Instant::now();
        let mut settings = settings();
        let mut coordinator = Coordinator::new(&settings, vec![], true, now);
        settings.source_check_intervals.free_steam_keys = 120;
        coordinator.reconfigure(&settings, true, now + Duration::from_secs(10));
        assert!(
            coordinator
                .entries
                .iter()
                .all(|entry| entry.due == Some(now))
        );
        assert_eq!(
            coordinator
                .start(false, now + Duration::from_secs(10))
                .1
                .len(),
            4
        );
        let cleared_at = now + Duration::from_secs(20);
        coordinator.reset(cleared_at);
        settings.source_check_intervals.free_steam_keys = 180;
        coordinator.reconfigure(&settings, true, now + Duration::from_secs(30));
        assert!(
            coordinator
                .entries
                .iter()
                .all(|entry| entry.due == Some(cleared_at))
        );
        assert_eq!(
            coordinator
                .start(false, now + Duration::from_secs(30))
                .1
                .len(),
            4
        );
    }
    #[test]
    fn persisted_checking_is_not_restored_as_a_live_task() {
        let now = Instant::now();
        let mut status = SourceStatus::new(GiveawaySource::FreeSteamKeys);
        status.phase = SourcePhase::Checking;
        status.last_successful_check_utc = Some("2026-11-07T10:00:00Z".into());
        let mut coordinator = Coordinator::new(&settings(), vec![status], false, now);
        assert!(coordinator.start(true, now).1.is_empty());
        assert_eq!(coordinator.states()[0].phase, SourcePhase::Ok);
        coordinator.reconfigure(&settings(), true, now);
        assert_eq!(coordinator.start(false, now).1.len(), 4);
    }
    #[test]
    fn an_empty_success_clears_error_and_ignores_disabled_failures_in_aggregate() {
        let now = Instant::now();
        let mut coordinator = Coordinator::new(&settings(), vec![], true, now);
        let (_, launches) = coordinator.start(false, now);
        let token = launches[0].token;
        coordinator.finish(
            token,
            Some(SourceFailure {
                source: token.source,
                message: "timeout".into(),
                reason: giveaway_scout_core::SourceFailureReason::Timeout,
            }),
            0,
            0,
            now,
        );
        let (_, launches) = coordinator.start(true, now);
        coordinator.finish(launches[0].token, None, 0, 0, now);
        assert_eq!(coordinator.states()[0].phase, SourcePhase::Ok);
        assert!(coordinator.states()[0].last_error.is_none());
    }
}
