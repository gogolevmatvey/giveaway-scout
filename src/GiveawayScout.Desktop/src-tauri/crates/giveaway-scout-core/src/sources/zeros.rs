//! Public, anonymous `getConfig` protocol used by http://zeros.group/free/.
//! Never executes the page's JavaScript or sends login/claim commands.
use std::{collections::HashSet, time::Duration};

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use time::{OffsetDateTime, PrimitiveDateTime, UtcOffset, format_description::well_known::Rfc3339};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};

use super::{SourceParseError, normalize_text};
use crate::{GameStore, GiveawayItem, GiveawayKind, GiveawaySource, PageFetchError};

pub const ZEROS_FEED_URL: &str = "http://zeros.group/free/";
const SOCKET_URL: &str = "ws://zeros.group:8863/";
const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

pub(crate) async fn fetch_zeros(timeout: Duration) -> Result<String, PageFetchError> {
    fetch_with_timeout(SOCKET_URL, timeout).await
}

async fn fetch_with_timeout(endpoint: &str, timeout: Duration) -> Result<String, PageFetchError> {
    tokio::time::timeout(timeout, fetch_config_from(endpoint))
        .await
        .map_err(|_| {
            PageFetchError::Timeout("Zeros: public configuration request timed out".into())
        })?
}

async fn fetch_config_from(endpoint: &str) -> Result<String, PageFetchError> {
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_MESSAGE_BYTES));
    let (mut socket, _) = connect_async_with_config(endpoint, Some(config), false)
        .await
        .map_err(socket_error)?;
    socket
        .send(Message::text("getConfig"))
        .await
        .map_err(socket_error)?;
    let result = async {
        // Bound unrelated server messages as well as payload size and total time.
        for _ in 0..32 {
            match socket.next().await {
                Some(Ok(Message::Text(message))) => {
                    if let Some(payload) = message.strip_prefix("config ") {
                        return Ok(payload.to_owned());
                    }
                    if message.starts_with("connectionRejected") {
                        return Err(PageFetchError::ZerosConnectionLimit);
                    }
                }
                Some(Ok(Message::Close(_))) | None => break,
                Some(Err(error)) => return Err(socket_error(error)),
                _ => {}
            }
        }
        Err(PageFetchError::Connection(
            "Zeros: connection ended without public configuration".into(),
        ))
    }
    .await;
    // Release the site's single-connection slot; do not keep a background session.
    let _ = socket.close(None).await;
    result
}

fn socket_error(error: tokio_tungstenite::tungstenite::Error) -> PageFetchError {
    use tokio_tungstenite::tungstenite::Error;
    let message = format!("Zeros WebSocket: {error}");
    match error {
        Error::Io(error) if error.kind() == std::io::ErrorKind::TimedOut => {
            PageFetchError::Timeout(message)
        }
        Error::Io(_) | Error::ConnectionClosed | Error::AlreadyClosed => {
            PageFetchError::Connection(message)
        }
        Error::Http(response) => PageFetchError::Http {
            status: response.status().as_u16(),
            message,
        },
        _ => PageFetchError::Message(message),
    }
}

#[derive(Deserialize)]
struct Config {
    periods: Vec<Period>,
}

#[derive(Deserialize)]
struct Period {
    period_id: String,
    period_name: String,
    stock_count: u64,
    is_expired: bool,
    #[serde(default)]
    is_not_started: bool,
    #[serde(default)]
    seconds_until_start: i64,
    deadline: Option<String>,
}

pub fn parse_zeros(json: &str) -> Result<Vec<GiveawayItem>, SourceParseError> {
    parse_at(json, OffsetDateTime::now_utc())
}

fn invalid(message: impl Into<String>) -> SourceParseError {
    SourceParseError::InvalidData {
        feed: "Zeros",
        message: message.into(),
    }
}

fn parse_at(json: &str, now: OffsetDateTime) -> Result<Vec<GiveawayItem>, SourceParseError> {
    let config: Config = serde_json::from_str(json)
        .map_err(|_| invalid("missing or invalid periods configuration"))?;
    if config.periods.len() > 10_000 {
        return Err(invalid("too many periods"));
    }
    let mut seen = HashSet::new();
    let mut items = Vec::new();
    for period in config.periods {
        let id = period.period_id.trim();
        let title = normalize_text(std::iter::once(period.period_name.as_str()))
            .ok_or_else(|| invalid("empty period_name"))?;
        if id.is_empty() || id.len() > 256 || title.len() > 4096 || !seen.insert(id.to_owned()) {
            return Err(invalid("empty, oversized or duplicate period identity"));
        }
        let expired = match period.deadline.as_deref().filter(|value| !value.is_empty()) {
            Some(deadline) => parse_deadline(deadline)? <= now,
            None => false,
        };
        if period.is_expired
            || expired
            || period.is_not_started
            || period.seconds_until_start > 0
            || period.stock_count == 0
        {
            continue;
        }
        items.push(GiveawayItem {
            source: GiveawaySource::Zeros,
            source_id: id.to_owned(),
            title,
            // The site has no supported period deep link. Never invent a URL from untrusted JSON.
            url: ZEROS_FEED_URL.to_owned(),
            kind: GiveawayKind::Giveaway,
            store: GameStore::Steam,
            details: None,
            legacy_tracking_key_prefixes: Vec::new(),
        });
    }
    Ok(items)
}

fn parse_deadline(value: &str) -> Result<OffsetDateTime, SourceParseError> {
    if let Ok(value) = OffsetDateTime::parse(value, &Rfc3339) {
        return Ok(value);
    }
    // The site's parseBeijingTimeMs treats timestamps without an offset as UTC+08.
    let format = time::format_description::parse_borrowed::<2>(
        "[year]-[month]-[day]T[hour]:[minute]:[second]",
    )
    .expect("valid fixed timestamp format");
    PrimitiveDateTime::parse(value, &format)
        .map(|value| {
            value.assume_offset(UtcOffset::from_hms(8, 0, 0).expect("valid Beijing offset"))
        })
        .map_err(|_| invalid("invalid deadline timestamp"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadlines_without_offset_use_beijing_time_and_offsets_are_respected() {
        let expected = OffsetDateTime::parse("2026-10-21T08:24:00Z", &Rfc3339).unwrap();
        for value in [
            "2026-10-21T16:24:00",
            "2026-10-21T16:24:00.000+08:00",
            "2026-10-21T08:24:00Z",
        ] {
            assert_eq!(parse_deadline(value).unwrap(), expected);
        }
    }

    #[tokio::test]
    async fn public_request_only_reads_config_then_closes_without_claim_or_auth() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("ws://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            assert_eq!(
                socket.next().await.unwrap().unwrap(),
                Message::text("getConfig")
            );
            socket
                .send(Message::text("ping ignored status"))
                .await
                .unwrap();
            socket
                .send(Message::text(r#"config {"periods":[]}"#))
                .await
                .unwrap();
            assert!(matches!(
                socket.next().await.unwrap().unwrap(),
                Message::Close(_)
            ));
        });
        assert_eq!(
            fetch_with_timeout(&endpoint, Duration::from_secs(2))
                .await
                .unwrap(),
            r#"{"periods":[]}"#
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn one_ip_rejection_is_an_error_and_is_not_retried() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("ws://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            assert_eq!(
                socket.next().await.unwrap().unwrap(),
                Message::text("getConfig")
            );
            socket
                .send(Message::text("connectionRejected one connection per IP"))
                .await
                .unwrap();
            assert!(matches!(
                socket.next().await.unwrap().unwrap(),
                Message::Close(_)
            ));
            assert!(
                tokio::time::timeout(Duration::from_millis(100), listener.accept())
                    .await
                    .is_err()
            );
        });
        let error = fetch_with_timeout(&endpoint, Duration::from_secs(2))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("one connection per IP"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn silent_server_is_bounded_by_the_whole_request_timeout() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("ws://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            assert_eq!(
                socket.next().await.unwrap().unwrap(),
                Message::text("getConfig")
            );
            let _ = socket.next().await;
        });
        assert!(
            fetch_with_timeout(&endpoint, Duration::from_millis(100))
                .await
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );
        server.await.unwrap();
    }
}
