use std::{future::Future, time::Duration};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    Database, GiveawayItem, GiveawaySource, StorageError, parse_games_for_farm, parse_givee_club,
    parse_zeros,
    sources::{GAMES_FOR_FARM_FEED_URL, GIVEE_CLUB_FEED_URL, ZEROS_FEED_URL, fetch_zeros},
};

#[derive(Debug, Error)]
pub enum PageFetchError {
    #[error("{0}")]
    Request(#[from] reqwest::Error),
    #[error("{0}")]
    Message(String),
    #[error("{0}")]
    Timeout(String),
    #[error("{0}")]
    Connection(String),
    #[error("HTTP {status}: {message}")]
    Http { status: u16, message: String },
    #[error("Zeros permits only one connection per IP; close the site's tab and retry")]
    ZerosConnectionLimit,
}

pub trait PageFetcher: Sync {
    fn get_text(
        &self,
        url: &'static str,
    ) -> impl Future<Output = Result<String, PageFetchError>> + Send;
}

#[derive(Clone)]
pub struct HttpPageFetcher {
    client: reqwest::Client,
    retry_count: u32,
    timeout: Duration,
}

impl HttpPageFetcher {
    pub fn new() -> Result<Self, PageFetchError> {
        Self::with_policy(30, 2)
    }

    pub fn with_policy(timeout_seconds: u32, retry_count: u32) -> Result<Self, PageFetchError> {
        let timeout = Duration::from_secs(u64::from(timeout_seconds));
        Ok(Self {
            client: reqwest::Client::builder()
                .connect_timeout(timeout.min(Duration::from_secs(10)))
                .timeout(timeout)
                .user_agent(concat!("GiveawayScout/", env!("CARGO_PKG_VERSION")))
                .build()?,
            retry_count,
            timeout,
        })
    }
}

impl PageFetcher for HttpPageFetcher {
    async fn get_text(&self, url: &'static str) -> Result<String, PageFetchError> {
        if url == ZEROS_FEED_URL {
            // No immediate retry: Zeros permits only one connection per public IP.
            return fetch_zeros(self.timeout).await;
        }
        for attempt in 0..=self.retry_count {
            match self.client.get(url).send().await {
                Ok(response) => {
                    let status = response.status();
                    if should_retry_status(status) && attempt < self.retry_count {
                        wait_before_retry(attempt).await;
                        continue;
                    }

                    let response = response.error_for_status()?;
                    match response.text().await {
                        Ok(text) => return Ok(text),
                        Err(error) if attempt < self.retry_count => {
                            log::debug!(
                                "HTTP response body failed on attempt {} for {url}: {error}",
                                attempt + 1
                            );
                            wait_before_retry(attempt).await;
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
                Err(error) if should_retry_error(&error) && attempt < self.retry_count => {
                    log::debug!(
                        "HTTP request failed on attempt {} for {url}: {error}",
                        attempt + 1
                    );
                    wait_before_retry(attempt).await;
                }
                Err(error) => return Err(error.into()),
            }
        }

        Err(PageFetchError::Message(
            "HTTP retry loop ended unexpectedly".to_owned(),
        ))
    }
}

fn should_retry_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::REQUEST_TIMEOUT
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}

fn should_retry_error(error: &reqwest::Error) -> bool {
    error.is_timeout() || error.is_connect() || error.is_body()
}

async fn wait_before_retry(attempt: u32) {
    let delay_milliseconds = 500_u64.saturating_mul(1_u64 << attempt.min(3));
    tokio::time::sleep(Duration::from_millis(delay_milliseconds)).await;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceFailure {
    pub source: GiveawaySource,
    pub message: String,
    #[serde(default)]
    pub reason: SourceFailureReason,
}

/// Language-neutral contract; `message` remains technical diagnostic text.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "camelCase")]
pub enum SourceFailureReason {
    #[default]
    Unknown,
    Timeout,
    Connection,
    Http {
        status: u16,
    },
    InvalidFeed,
    ZerosConnectionLimit,
}

impl SourceFailure {
    pub fn from_fetch(source: GiveawaySource, error: PageFetchError) -> Self {
        let reason = match &error {
            PageFetchError::Timeout(_) => SourceFailureReason::Timeout,
            PageFetchError::Connection(_) => SourceFailureReason::Connection,
            PageFetchError::Http { status, .. } => SourceFailureReason::Http { status: *status },
            PageFetchError::ZerosConnectionLimit => SourceFailureReason::ZerosConnectionLimit,
            PageFetchError::Request(error) if error.is_timeout() => SourceFailureReason::Timeout,
            PageFetchError::Request(error) if error.status().is_some() => {
                SourceFailureReason::Http {
                    status: error.status().expect("checked HTTP status").as_u16(),
                }
            }
            PageFetchError::Request(error) if error.is_connect() || error.is_body() => {
                SourceFailureReason::Connection
            }
            _ => SourceFailureReason::Unknown,
        };
        Self {
            source,
            message: error.to_string(),
            reason,
        }
    }

    fn from_parse(source: GiveawaySource, error: crate::SourceParseError) -> Self {
        Self {
            source,
            message: error.to_string(),
            reason: SourceFailureReason::InvalidFeed,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceCheckResult {
    pub checked_sources: Vec<GiveawaySource>,
    pub items: Vec<GiveawayItem>,
    pub failures: Vec<SourceFailure>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitoringOutcome {
    pub check_performed: bool,
    pub is_initial_synchronization: bool,
    pub baseline_deferred: bool,
    pub new_items: Vec<GiveawayItem>,
    pub failures: Vec<SourceFailure>,
}

pub async fn check_sources(
    fetcher: &impl PageFetcher,
    givee_club_enabled: bool,
    games_for_farm_enabled: bool,
    zeros_enabled: bool,
) -> SourceCheckResult {
    let mut result = SourceCheckResult::default();

    if givee_club_enabled {
        result.checked_sources.push(GiveawaySource::GiveeClub);
        match fetcher.get_text(GIVEE_CLUB_FEED_URL).await {
            Ok(html) => match parse_givee_club(&html) {
                Ok(items) => result.items.extend(items),
                Err(error) => result
                    .failures
                    .push(SourceFailure::from_parse(GiveawaySource::GiveeClub, error)),
            },
            Err(error) => result
                .failures
                .push(SourceFailure::from_fetch(GiveawaySource::GiveeClub, error)),
        }
    }

    if games_for_farm_enabled {
        result.checked_sources.push(GiveawaySource::GamesForFarm);
        match fetcher.get_text(GAMES_FOR_FARM_FEED_URL).await {
            Ok(html) => match parse_games_for_farm(&html) {
                Ok(items) => result.items.extend(items),
                Err(error) => result.failures.push(SourceFailure::from_parse(
                    GiveawaySource::GamesForFarm,
                    error,
                )),
            },
            Err(error) => result.failures.push(SourceFailure::from_fetch(
                GiveawaySource::GamesForFarm,
                error,
            )),
        }
    }

    if zeros_enabled {
        result.checked_sources.push(GiveawaySource::Zeros);
        match fetcher.get_text(ZEROS_FEED_URL).await {
            Ok(json) => match parse_zeros(&json) {
                Ok(items) => result.items.extend(items),
                Err(error) => result
                    .failures
                    .push(SourceFailure::from_parse(GiveawaySource::Zeros, error)),
            },
            Err(error) => result
                .failures
                .push(SourceFailure::from_fetch(GiveawaySource::Zeros, error)),
        }
    }

    result
}

pub fn apply_source_check(
    database: &mut Database,
    source_result: SourceCheckResult,
) -> Result<MonitoringOutcome, StorageError> {
    if source_result.checked_sources.is_empty() {
        return Ok(MonitoringOutcome::default());
    }

    if !database.tracking_initialized()? && !source_result.failures.is_empty() {
        return Ok(MonitoringOutcome {
            check_performed: true,
            is_initial_synchronization: true,
            baseline_deferred: true,
            failures: source_result.failures,
            ..MonitoringOutcome::default()
        });
    }

    let successful_sources: Vec<_> = source_result
        .checked_sources
        .iter()
        .copied()
        .filter(|source| {
            !source_result
                .failures
                .iter()
                .any(|failure| failure.source == *source)
        })
        .collect();
    let discovery = database.process_discovered_items(&source_result.items, &successful_sources)?;
    Ok(MonitoringOutcome {
        check_performed: true,
        is_initial_synchronization: discovery.is_initial_synchronization,
        baseline_deferred: false,
        new_items: discovery.new_items,
        failures: source_result.failures,
    })
}

#[cfg(test)]
mod tests {
    use super::should_retry_status;

    #[test]
    fn retries_only_transient_http_statuses() {
        assert!(should_retry_status(reqwest::StatusCode::REQUEST_TIMEOUT));
        assert!(should_retry_status(reqwest::StatusCode::TOO_MANY_REQUESTS));
        assert!(should_retry_status(reqwest::StatusCode::BAD_GATEWAY));
        assert!(!should_retry_status(reqwest::StatusCode::NOT_FOUND));
        assert!(!should_retry_status(reqwest::StatusCode::UNAUTHORIZED));
    }
}
