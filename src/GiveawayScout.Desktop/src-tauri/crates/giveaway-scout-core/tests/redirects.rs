use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use giveaway_scout_core::{
    OriginResolveError, OriginResolver, RedirectResponse, RedirectTransport,
};
use url::Url;

#[derive(Default)]
struct Observed {
    calls: Mutex<Vec<String>>,
    active: AtomicUsize,
    peak: AtomicUsize,
}

struct ActiveRequest(Arc<Observed>);
impl Drop for ActiveRequest {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Clone)]
struct ScriptedTransport {
    responses: Arc<HashMap<String, RedirectResponse>>,
    observed: Arc<Observed>,
    delay: Duration,
    first_delay: Option<Duration>,
}

impl ScriptedTransport {
    fn new(
        responses: impl IntoIterator<Item = (String, RedirectResponse)>,
        delay: Duration,
    ) -> Self {
        Self {
            responses: Arc::new(responses.into_iter().collect()),
            observed: Arc::new(Observed::default()),
            delay,
            first_delay: None,
        }
    }
    fn with_first_delay(mut self, delay: Duration) -> Self {
        self.first_delay = Some(delay);
        self
    }
    fn calls(&self) -> Vec<String> {
        self.observed.calls.lock().unwrap().clone()
    }
}

impl RedirectTransport for ScriptedTransport {
    async fn get_redirect(&self, url: &Url) -> Result<RedirectResponse, OriginResolveError> {
        let call_index = {
            let mut calls = self.observed.calls.lock().unwrap();
            let index = calls.len();
            calls.push(url.to_string());
            index
        };
        let active = self.observed.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.observed.peak.fetch_max(active, Ordering::SeqCst);
        let _active = ActiveRequest(self.observed.clone());
        let delay = if call_index == 0 {
            self.first_delay.unwrap_or(self.delay)
        } else {
            self.delay
        };
        tokio::time::sleep(delay).await;
        self.responses
            .get(url.as_str())
            .cloned()
            .ok_or_else(|| OriginResolveError(format!("unexpected request: {url}")))
    }
}

fn original(id: usize) -> String {
    format!("https://www.freesteamkeys.com/open/{id}")
}
fn redirect(location: &str) -> RedirectResponse {
    RedirectResponse {
        status: 302,
        location: Some(location.into()),
    }
}

#[tokio::test]
async fn known_givee_and_gleam_destinations_stop_at_their_location_header() {
    for destination in [
        "https://givee.club/en/event/42#claim",
        "https://gleam.io/4cb3E/free-steam-keys-of-one-gun-guy",
    ] {
        let transport =
            ScriptedTransport::new([(original(0), redirect(destination))], Duration::ZERO);
        let resolver = OriginResolver::with_transport(transport.clone());
        assert_eq!(
            resolver.resolve(&original(0)).await.unwrap(),
            destination.trim_end_matches("#claim")
        );
        assert_eq!(transport.calls(), [original(0)]);
    }
}

#[tokio::test]
async fn ordinary_external_destinations_are_opening_links_without_following_their_site() {
    let destination = "https://store.steampowered.com/app/440/";
    let transport = ScriptedTransport::new([(original(0), redirect(destination))], Duration::ZERO);
    let resolver = OriginResolver::with_transport(transport.clone());
    assert_eq!(resolver.resolve(&original(0)).await.unwrap(), destination);
    assert_eq!(transport.calls(), [original(0)]);
}

#[tokio::test]
async fn five_hops_are_allowed_and_a_sixth_request_is_never_issued() {
    for succeeds in [true, false] {
        let responses = (0..5).map(|index| {
            let location = if succeeds && index == 4 {
                "https://gleam.io/AbC12/game".to_owned()
            } else {
                format!("/open/{}", index + 1)
            };
            (original(index), redirect(&location))
        });
        let transport = ScriptedTransport::new(responses, Duration::ZERO);
        let resolver = OriginResolver::with_transport(transport.clone());
        let result = resolver.resolve(&original(0)).await;
        if succeeds {
            assert_eq!(result.unwrap(), "https://gleam.io/AbC12/game");
        } else {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("too many redirects")
            );
        }
        assert_eq!(transport.calls(), (0..5).map(original).collect::<Vec<_>>());
    }
}

#[tokio::test]
async fn repeated_relative_redirects_detect_a_loop_before_requesting_it_again() {
    let transport = ScriptedTransport::new(
        [
            (original(0), redirect("/open/1")),
            (original(1), redirect("/open/0")),
        ],
        Duration::ZERO,
    );
    let resolver = OriginResolver::with_transport(transport.clone());
    assert!(
        resolver
            .resolve(&original(0))
            .await
            .unwrap_err()
            .to_string()
            .contains("loop")
    );
    assert_eq!(transport.calls(), [original(0), original(1)]);
}

#[tokio::test]
async fn only_location_bearing_redirect_statuses_are_followed() {
    for status in [301, 302, 303, 307, 308] {
        let transport = ScriptedTransport::new(
            [(
                original(0),
                RedirectResponse {
                    status,
                    location: Some("https://gleam.io/AbC12/game".into()),
                },
            )],
            Duration::ZERO,
        );
        let resolver = OriginResolver::with_transport(transport);
        assert!(
            resolver.resolve(&original(0)).await.is_ok(),
            "HTTP {status}"
        );
    }
    for status in [200, 204, 300, 304, 305, 403, 503] {
        let transport = ScriptedTransport::new(
            [(
                original(0),
                RedirectResponse {
                    status,
                    location: Some("https://gleam.io/AbC12/game".into()),
                },
            )],
            Duration::ZERO,
        );
        let resolver = OriginResolver::with_transport(transport);
        assert!(
            resolver.resolve(&original(0)).await.is_err(),
            "HTTP {status}"
        );
    }
}

#[tokio::test]
async fn unsupported_sources_destinations_statuses_and_headers_fail_without_external_requests() {
    let responses = [
        RedirectResponse {
            status: 200,
            location: Some("https://gleam.io/AbC12/game".into()),
        },
        RedirectResponse {
            status: 302,
            location: None,
        },
        redirect(" "),
        redirect("javascript:alert(1)"),
        redirect("file:///C:/private.txt"),
        redirect("http://127.0.0.1/admin"),
        redirect("http://printer.local/admin"),
        redirect("https://user:password@gleam.io/AbC12/game"),
        redirect("https://gleam.io:8443/AbC12/game"),
        redirect("http://www.freesteamkeys.com/open/1"),
    ];
    for response in responses {
        let transport = ScriptedTransport::new([(original(0), response)], Duration::ZERO);
        let resolver = OriginResolver::with_transport(transport.clone());
        assert!(resolver.resolve(&original(0)).await.is_err());
        assert_eq!(transport.calls(), [original(0)]);
    }
    let transport = ScriptedTransport::new([], Duration::ZERO);
    let resolver = OriginResolver::with_transport(transport.clone());
    for source in [
        "https://givee.club/event/42",
        "http://www.freesteamkeys.com/open/0",
        "https://www.freesteamkeys.com.evil.example/open/0",
    ] {
        assert!(resolver.resolve(source).await.is_err());
    }
    assert!(transport.calls().is_empty());
}

#[tokio::test]
async fn simultaneous_batches_share_two_global_request_permits() {
    let transport = ScriptedTransport::new(
        (0..8).map(|index| (original(index), redirect("https://gleam.io/AbC12/game"))),
        Duration::from_millis(20),
    );
    let resolver = OriginResolver::with_transport(transport.clone());
    let (first, second) = tokio::join!(
        resolver.resolve_batch((0..4).map(original).collect()),
        resolver.resolve_batch((4..8).map(original).collect()),
    );
    assert_eq!(first.len(), 4);
    assert_eq!(second.len(), 4);
    assert!(
        first
            .into_iter()
            .chain(second)
            .all(|(_, result)| result.is_ok())
    );
    assert_eq!(transport.observed.peak.load(Ordering::SeqCst), 2);
    assert_eq!(transport.observed.active.load(Ordering::SeqCst), 0);
    assert_eq!(transport.calls().len(), 8);
}

#[tokio::test]
async fn one_five_second_deadline_cancels_the_whole_batch_and_keeps_every_original() {
    let transport = ScriptedTransport::new(
        [(original(0), redirect("https://gleam.io/AbC12/game"))],
        Duration::from_secs(60),
    )
    .with_first_delay(Duration::ZERO);
    let resolver = OriginResolver::with_transport(transport.clone());
    // The first copy completes; the second copy hangs. Each input must retain
    // its own outcome when the common deadline cancels the remaining work.
    let urls = vec![
        original(0),
        original(0),
        original(1),
        original(2),
        original(3),
    ];
    let started = Instant::now();
    let results = resolver.resolve_batch(urls.clone()).await;
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_millis(4_500), "{elapsed:?}");
    assert!(
        elapsed < Duration::from_secs(9),
        "a batch must not spend five seconds per URL: {elapsed:?}"
    );
    assert_eq!(
        results
            .iter()
            .map(|(url, _)| url.clone())
            .collect::<Vec<_>>(),
        urls
    );
    assert_eq!(
        results.iter().filter(|(_, result)| result.is_ok()).count(),
        1
    );
    assert_eq!(
        results.iter().filter(|(_, result)| result.is_err()).count(),
        4
    );
    assert_eq!(transport.calls().len(), 3, "queued lookups never started");
    assert_eq!(
        transport.observed.active.load(Ordering::SeqCst),
        0,
        "timed-out futures release permits"
    );
}

#[tokio::test]
async fn a_single_lookup_also_has_a_five_second_deadline() {
    let transport = ScriptedTransport::new([], Duration::from_secs(60));
    let resolver = OriginResolver::with_transport(transport.clone());
    let started = Instant::now();
    let error = resolver.resolve(&original(0)).await.unwrap_err();
    assert!(error.to_string().contains("timed out"));
    assert!(started.elapsed() < Duration::from_secs(9));
    assert_eq!(transport.observed.active.load(Ordering::SeqCst), 0);
}
