use std::{cell::RefCell, error::Error as _, future::Future, time::Duration};

use serde::{Deserialize, Serialize};
use thiserror::Error;

tokio::task_local! {
    /// Each send future owns its redirect state, even when a shared client is
    /// polled concurrently or several requests run inside the same Tokio task.
    static REDIRECT_TARGET: RefCell<Option<url::Url>>;
}

use crate::{
    Database, GiveawayItem, GiveawaySource, StorageError, parse_free_steam_keys,
    parse_games_for_farm, parse_givee_club, parse_zeros,
    sources::{
        FREE_STEAM_KEYS_FEED_URL, GAMES_FOR_FARM_FEED_URL, GIVEE_CLUB_FEED_URL, ZEROS_FEED_URL,
        fetch_zeros,
    },
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
                .redirect(tracked_redirect_policy())
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
        self.get_http_text(url).await
    }
}

impl HttpPageFetcher {
    async fn get_http_text(&self, url: &str) -> Result<String, PageFetchError> {
        for attempt in 0..=self.retry_count {
            let response = REDIRECT_TARGET
                .scope(RefCell::new(None), async {
                    self.client.get(url).send().await.map_err(|error| {
                        // reqwest may attach the initial URL to connection and
                        // timeout errors after a redirect. The callback records
                        // the target within this exact send future's scope.
                        if error.is_redirect() {
                            return error;
                        }
                        let target = REDIRECT_TARGET.with(|target| target.borrow().clone());
                        match target {
                            Some(target) => error.with_url(target),
                            None => error,
                        }
                    })
                })
                .await;
            match response {
                Ok(response) => {
                    let status = response.status();
                    if should_retry_status(status) && attempt < self.retry_count {
                        wait_before_retry(attempt).await;
                        continue;
                    }

                    let response = response.error_for_status()?;
                    if is_empty_free_steam_keys_response(url, status) {
                        return Ok("[]".to_owned());
                    }
                    // Reading the body can fail after a redirect. Preserve the
                    // response URL rather than reporting the original feed URL.
                    let response_url = response.url().clone();
                    match response.text().await {
                        Ok(text) => return Ok(text),
                        Err(error) if attempt < self.retry_count => {
                            log::debug!(
                                "HTTP response body failed on attempt {} for {url}: {error}",
                                attempt + 1
                            );
                            wait_before_retry(attempt).await;
                        }
                        Err(error) => return Err(error.with_url(response_url).into()),
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

fn tracked_redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        // The default policy still decides whether to follow or reject the
        // redirect. Rejected redirects retain reqwest's own error URL above.
        let _ = REDIRECT_TARGET.try_with(|target| {
            *target.borrow_mut() = Some(attempt.url().clone());
        });
        reqwest::redirect::Policy::default().redirect(attempt)
    })
}

fn should_retry_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::REQUEST_TIMEOUT
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}

fn is_empty_free_steam_keys_response(url: &str, status: reqwest::StatusCode) -> bool {
    // This API documents 204 as an empty active list. An empty 200 response, or
    // an empty response from any other source, must still fail parsing.
    url == FREE_STEAM_KEYS_FEED_URL && status == reqwest::StatusCode::NO_CONTENT
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
            message: fetch_error_details(source, error),
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

fn fetch_error_details(source: GiveawaySource, error: PageFetchError) -> String {
    let PageFetchError::Request(error) = error else {
        return error.to_string();
    };
    let url = error.url().map(ToString::to_string).or_else(|| {
        // Custom fetchers can return a reqwest error without an attached URL.
        // Zeros uses WebSocket, so its page is not a request endpoint fallback.
        match source {
            GiveawaySource::FreeSteamKeys => Some(FREE_STEAM_KEYS_FEED_URL),
            GiveawaySource::GiveeClub => Some(GIVEE_CLUB_FEED_URL),
            GiveawaySource::GamesForFarm => Some(GAMES_FOR_FARM_FEED_URL),
            GiveawaySource::Zeros => None,
        }
        .map(str::to_owned)
    });
    // reqwest's Display omits its cause. Keep every distinct level, including
    // DNS/TLS/HTTP transport and the final operating-system error if available.
    let error = error.without_url();
    let mut details = vec![error.to_string()];
    let mut cause = error.source();
    while let Some(error) = cause {
        let message = error.to_string();
        if details.last() != Some(&message) {
            details.push(message);
        }
        cause = error.source();
    }
    let details = details.join(": ");
    match url {
        Some(url) => format!("{url}: {details}"),
        None => details,
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

pub async fn check_source(fetcher: &impl PageFetcher, source: GiveawaySource) -> SourceCheckResult {
    type SourceParser = fn(&str) -> Result<Vec<GiveawayItem>, crate::SourceParseError>;
    let (url, parser): (&str, SourceParser) = match source {
        GiveawaySource::FreeSteamKeys => (FREE_STEAM_KEYS_FEED_URL, parse_free_steam_keys),
        GiveawaySource::GiveeClub => (GIVEE_CLUB_FEED_URL, parse_givee_club),
        GiveawaySource::GamesForFarm => (GAMES_FOR_FARM_FEED_URL, parse_games_for_farm),
        GiveawaySource::Zeros => (ZEROS_FEED_URL, parse_zeros),
    };
    let mut result = SourceCheckResult {
        checked_sources: vec![source],
        ..Default::default()
    };
    match fetcher.get_text(url).await {
        Ok(body) => match parser(&body) {
            Ok(items) => result.items = items,
            Err(error) => result
                .failures
                .push(SourceFailure::from_parse(source, error)),
        },
        Err(error) => result
            .failures
            .push(SourceFailure::from_fetch(source, error)),
    }
    result
}

/// Compatibility helper for diagnostics and fixtures. The desktop coordinator
/// calls `check_source` independently and applies each completed result at once.
pub async fn check_sources(
    fetcher: &impl PageFetcher,
    givee_club_enabled: bool,
    games_for_farm_enabled: bool,
    zeros_enabled: bool,
    free_steam_keys_enabled: bool,
) -> SourceCheckResult {
    let mut result = SourceCheckResult::default();
    for source in GiveawaySource::ALL {
        let enabled = match source {
            GiveawaySource::FreeSteamKeys => free_steam_keys_enabled,
            GiveawaySource::GiveeClub => givee_club_enabled,
            GiveawaySource::GamesForFarm => games_for_farm_enabled,
            GiveawaySource::Zeros => zeros_enabled,
        };
        if enabled {
            let checked = check_source(fetcher, source).await;
            result.checked_sources.extend(checked.checked_sources);
            result.items.extend(checked.items);
            result.failures.extend(checked.failures);
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
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::{Arc, mpsc},
        thread::{self, JoinHandle},
        time::{Duration, Instant},
    };

    use super::{
        HttpPageFetcher, is_empty_free_steam_keys_response, should_retry_status,
        tracked_redirect_policy,
    };
    use crate::{
        FREE_STEAM_KEYS_FEED_URL, GIVEE_CLUB_FEED_URL, GiveawaySource, PageFetchError,
        SourceFailure, SourceFailureReason,
    };

    fn local_fetcher(timeout: Duration) -> HttpPageFetcher {
        HttpPageFetcher {
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(timeout)
                .redirect(tracked_redirect_policy())
                .build()
                .unwrap(),
            retry_count: 0,
            timeout,
        }
    }

    fn local_server(serve: impl FnOnce(TcpListener) + Send + 'static) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        (url, thread::spawn(move || serve(listener)))
    }

    fn accept_request(listener: &TcpListener) -> TcpStream {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    // Windows accepted sockets can inherit the listener's mode.
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(1)))
                        .unwrap();
                    let mut request = Vec::new();
                    let mut buffer = [0; 1024];
                    while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                        let count = stream.read(&mut buffer).unwrap();
                        assert_ne!(count, 0, "request ended before its HTTP headers");
                        request.extend_from_slice(&buffer[..count]);
                    }
                    return stream;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "local HTTP request not received");
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("could not accept local HTTP request: {error}"),
            }
        }
    }

    fn last_cause(error: &reqwest::Error) -> String {
        let mut cause: &(dyn std::error::Error + 'static) = error;
        while let Some(source) = cause.source() {
            cause = source;
        }
        cause.to_string()
    }

    #[test]
    fn only_documented_free_steam_keys_no_content_is_an_empty_feed() {
        assert!(is_empty_free_steam_keys_response(
            FREE_STEAM_KEYS_FEED_URL,
            reqwest::StatusCode::NO_CONTENT
        ));
        assert!(!is_empty_free_steam_keys_response(
            FREE_STEAM_KEYS_FEED_URL,
            reqwest::StatusCode::OK
        ));
        assert!(!is_empty_free_steam_keys_response(
            GIVEE_CLUB_FEED_URL,
            reqwest::StatusCode::NO_CONTENT
        ));
    }

    #[test]
    fn retries_only_transient_http_statuses() {
        assert!(should_retry_status(reqwest::StatusCode::REQUEST_TIMEOUT));
        assert!(should_retry_status(reqwest::StatusCode::TOO_MANY_REQUESTS));
        assert!(should_retry_status(reqwest::StatusCode::BAD_GATEWAY));
        assert!(!should_retry_status(reqwest::StatusCode::NOT_FOUND));
        assert!(!should_retry_status(reqwest::StatusCode::UNAUTHORIZED));
    }

    #[tokio::test]
    async fn refused_connection_preserves_the_actual_url_and_operating_system_cause() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/unavailable", listener.local_addr().unwrap());
        drop(listener);
        let error = local_fetcher(Duration::from_secs(10))
            .get_http_text(&url)
            .await
            .unwrap_err();
        let PageFetchError::Request(request_error) = &error else {
            panic!("expected real HTTP request error");
        };
        let cause = last_cause(request_error);
        assert_ne!(cause, request_error.to_string());
        let failure = SourceFailure::from_fetch(GiveawaySource::GamesForFarm, error);
        assert_eq!(failure.reason, SourceFailureReason::Connection);
        assert!(failure.message.starts_with(&url));
        assert!(failure.message.contains(&cause));
    }

    #[tokio::test]
    async fn dns_failure_preserves_the_resolver_cause_without_a_live_dns_request() {
        struct FailingResolver;
        impl reqwest::dns::Resolve for FailingResolver {
            fn resolve(&self, _name: reqwest::dns::Name) -> reqwest::dns::Resolving {
                Box::pin(async {
                    Err(std::io::Error::other("controlled DNS resolver failure").into())
                })
            }
        }
        let timeout = Duration::from_secs(2);
        let fetcher = HttpPageFetcher {
            client: reqwest::Client::builder()
                .no_proxy()
                .dns_resolver(Arc::new(FailingResolver))
                .timeout(timeout)
                .build()
                .unwrap(),
            retry_count: 0,
            timeout,
        };
        let error = fetcher
            .get_http_text("http://controlled-source.invalid/")
            .await
            .unwrap_err();
        let failure = SourceFailure::from_fetch(GiveawaySource::GamesForFarm, error);
        assert_eq!(failure.reason, SourceFailureReason::Connection);
        assert!(
            failure
                .message
                .starts_with("http://controlled-source.invalid/:")
        );
        assert!(failure.message.contains("controlled DNS resolver failure"));
    }

    #[tokio::test]
    async fn tls_handshake_failure_preserves_the_tls_cause() {
        let (url, server) = local_server(|listener| {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(1)))
                            .unwrap();
                        let mut client_hello = [0; 8192];
                        assert_ne!(stream.read(&mut client_hello).unwrap(), 0);
                        stream
                            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                            .unwrap();
                        stream.shutdown(std::net::Shutdown::Write).unwrap();
                        let _ = stream.read(&mut client_hello);
                        break;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "TLS connection not received");
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("could not accept TLS connection: {error}"),
                }
            }
        });
        let url = url.replacen("http:", "https:", 1);
        let error = local_fetcher(Duration::from_secs(2))
            .get_http_text(&url)
            .await
            .unwrap_err();
        server.join().unwrap();
        let PageFetchError::Request(request_error) = &error else {
            panic!("expected real TLS request error");
        };
        let cause = last_cause(request_error);
        assert_ne!(cause, request_error.to_string());
        let failure = SourceFailure::from_fetch(GiveawaySource::GamesForFarm, error);
        assert_eq!(failure.reason, SourceFailureReason::Connection);
        assert!(failure.message.starts_with(&url));
        assert!(failure.message.contains(&cause));
    }

    #[tokio::test]
    async fn connection_closed_without_response_preserves_the_transport_cause() {
        let (url, server) = local_server(|listener| {
            drop(accept_request(&listener));
        });
        let error = local_fetcher(Duration::from_secs(2))
            .get_http_text(&url)
            .await
            .unwrap_err();
        server.join().unwrap();
        let PageFetchError::Request(request_error) = &error else {
            panic!("expected real HTTP request error");
        };
        let cause = last_cause(request_error);
        assert_ne!(cause, request_error.to_string());
        let failure = SourceFailure::from_fetch(GiveawaySource::GamesForFarm, error);
        assert!(failure.message.starts_with(&url));
        assert!(failure.message.contains(&cause));
        assert!(!failure.message.contains("freeanywhere.net"));
    }

    #[tokio::test]
    async fn timeout_retains_the_actual_transport_cause() {
        let (release_server, wait_for_client) = mpsc::channel::<()>();
        let (url, server) = local_server(move |listener| {
            let _stream = accept_request(&listener);
            let _ = wait_for_client.recv_timeout(Duration::from_secs(3));
        });
        let error = local_fetcher(Duration::from_millis(500))
            .get_http_text(&url)
            .await
            .unwrap_err();
        release_server.send(()).unwrap();
        server.join().unwrap();
        let PageFetchError::Request(request_error) = &error else {
            panic!("expected real HTTP request error");
        };
        let cause = last_cause(request_error);
        let failure = SourceFailure::from_fetch(GiveawaySource::GamesForFarm, error);
        assert_eq!(failure.reason, SourceFailureReason::Timeout);
        assert!(failure.message.starts_with(&url));
        assert!(failure.message.contains(&cause));
    }

    #[tokio::test]
    async fn real_http_status_and_failed_redirect_url_are_preserved() {
        for status in [429, 503] {
            let (url, server) = local_server(move |listener| {
                accept_request(&listener)
                    .write_all(b"HTTP/1.1 302 Found\r\nLocation: /failed-target\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .unwrap();
                write!(
                    accept_request(&listener),
                    "HTTP/1.1 {status} Failure\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
            });
            let error = local_fetcher(Duration::from_secs(2))
                .get_http_text(&url)
                .await
                .unwrap_err();
            server.join().unwrap();
            let failure = SourceFailure::from_fetch(GiveawaySource::GamesForFarm, error);
            assert_eq!(failure.reason, SourceFailureReason::Http { status });
            assert!(failure.message.starts_with(&format!("{url}failed-target:")));
            assert!(failure.message.contains(&status.to_string()));
        }
    }

    #[tokio::test]
    async fn redirected_connection_refusal_reports_the_target_instead_of_the_initial_url() {
        let refused_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let target = format!(
            "http://{}/refused-target",
            refused_listener.local_addr().unwrap()
        );
        let redirect_target = target.clone();
        let (url, server) = local_server(move |listener| {
            write!(
                accept_request(&listener),
                "HTTP/1.1 302 Found\r\nLocation: {redirect_target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
        });
        // Keep the refused port reserved until the redirect server has a
        // different port, then close it before starting the actual request.
        drop(refused_listener);
        let error = local_fetcher(Duration::from_secs(10))
            .get_http_text(&url)
            .await
            .unwrap_err();
        server.join().unwrap();
        let PageFetchError::Request(request_error) = &error else {
            panic!("expected real HTTP request error");
        };
        let cause = last_cause(request_error);
        let failure = SourceFailure::from_fetch(GiveawaySource::GamesForFarm, error);
        assert_eq!(failure.reason, SourceFailureReason::Connection);
        assert!(failure.message.starts_with(&format!("{target}:")));
        assert!(failure.message.contains(&cause));
        assert!(!failure.message.contains(&url));
    }

    #[tokio::test]
    async fn redirected_timeout_reports_the_target_instead_of_the_initial_url() {
        let (release_server, wait_for_client) = mpsc::channel::<()>();
        let (url, server) = local_server(move |listener| {
            accept_request(&listener)
                .write_all(b"HTTP/1.1 302 Found\r\nLocation: /slow-target\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap();
            let _stream = accept_request(&listener);
            let _ = wait_for_client.recv_timeout(Duration::from_secs(3));
        });
        let initial_url = format!("{url}initial-timeout");
        let error = local_fetcher(Duration::from_millis(500))
            .get_http_text(&initial_url)
            .await
            .unwrap_err();
        release_server.send(()).unwrap();
        server.join().unwrap();
        let failure = SourceFailure::from_fetch(GiveawaySource::GamesForFarm, error);
        assert_eq!(failure.reason, SourceFailureReason::Timeout);
        assert!(failure.message.starts_with(&format!("{url}slow-target:")));
        assert!(!failure.message.contains(&initial_url));
    }

    #[tokio::test]
    async fn concurrent_redirects_in_the_same_task_keep_their_error_targets_isolated() {
        fn stalling_redirect(
            path: &'static str,
        ) -> (
            String,
            JoinHandle<()>,
            tokio::sync::oneshot::Receiver<()>,
            mpsc::Sender<()>,
        ) {
            let (reached_target, wait_for_target) = tokio::sync::oneshot::channel();
            let (release_target, wait_for_release) = mpsc::channel();
            let (url, server) = local_server(move |listener| {
                write!(
                    accept_request(&listener),
                    "HTTP/1.1 302 Found\r\nLocation: /{path}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
                let _stream = accept_request(&listener);
                reached_target.send(()).unwrap();
                let _ = wait_for_release.recv_timeout(Duration::from_secs(3));
            });
            (url, server, wait_for_target, release_target)
        }

        let (first_url, first_server, first_reached, first_release) =
            stalling_redirect("first-target");
        let (second_url, second_server, second_reached, second_release) =
            stalling_redirect("second-target");
        let fetcher = local_fetcher(Duration::from_secs(2));
        let first_initial = format!("{first_url}initial-first");
        let second_initial = format!("{second_url}initial-second");
        let release_both = async {
            // Both redirect callbacks have run before either connection fails.
            first_reached.await.unwrap();
            second_reached.await.unwrap();
            first_release.send(()).unwrap();
            second_release.send(()).unwrap();
        };
        let (first, second, ()) = tokio::join!(
            fetcher.get_http_text(&first_initial),
            fetcher.get_http_text(&second_initial),
            release_both
        );
        first_server.join().unwrap();
        second_server.join().unwrap();
        let first = SourceFailure::from_fetch(GiveawaySource::GamesForFarm, first.unwrap_err());
        let second = SourceFailure::from_fetch(GiveawaySource::GiveeClub, second.unwrap_err());
        assert!(
            first
                .message
                .starts_with(&format!("{first_url}first-target:"))
        );
        assert!(
            second
                .message
                .starts_with(&format!("{second_url}second-target:"))
        );
        assert!(!first.message.contains("second-target"));
        assert!(!second.message.contains("first-target"));
        assert!(!first.message.contains(&first_initial));
        assert!(!second.message.contains(&second_initial));
    }

    #[tokio::test]
    async fn default_redirect_limit_retains_the_last_requested_url_not_the_rejected_target() {
        let (url, server) = local_server(|listener| {
            for next_hop in 1..=11 {
                write!(
                    accept_request(&listener),
                    "HTTP/1.1 302 Found\r\nLocation: /hop/{next_hop}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
            }
        });
        let error = local_fetcher(Duration::from_secs(2))
            .get_http_text(&url)
            .await
            .unwrap_err();
        server.join().unwrap();
        let PageFetchError::Request(request_error) = &error else {
            panic!("expected reqwest redirect policy error");
        };
        assert!(request_error.is_redirect());
        let failure = SourceFailure::from_fetch(GiveawaySource::GamesForFarm, error);
        assert!(failure.message.starts_with(&format!("{url}hop/10:")));
        assert!(!failure.message.contains("hop/11"));
    }

    #[tokio::test]
    async fn interrupted_redirect_response_body_retains_the_final_url_and_cause() {
        let (url, server) = local_server(|listener| {
            accept_request(&listener)
                .write_all(b"HTTP/1.1 302 Found\r\nLocation: /truncated-body\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap();
            accept_request(&listener)
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\nshort",
                )
                .unwrap();
        });
        let error = local_fetcher(Duration::from_secs(2))
            .get_http_text(&url)
            .await
            .unwrap_err();
        server.join().unwrap();
        let PageFetchError::Request(request_error) = &error else {
            panic!("expected real HTTP request error");
        };
        let cause = last_cause(request_error);
        let failure = SourceFailure::from_fetch(GiveawaySource::GamesForFarm, error);
        assert!(
            failure
                .message
                .starts_with(&format!("{url}truncated-body:"))
        );
        assert!(failure.message.contains(&cause));
    }
}
