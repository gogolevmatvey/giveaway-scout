pub mod admin;
pub mod store;

use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    Method, Request, Response, StatusCode,
    body::{Bytes, Incoming},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use regex::Regex;
use serde::Deserialize;
use std::{
    convert::Infallible,
    future::Future,
    sync::{Arc, OnceLock},
    time::Duration,
};
use store::{Metrics, Store};
use time::{Date, OffsetDateTime};
use tokio::{
    net::TcpListener,
    sync::Semaphore,
    task::JoinSet,
    time::{MissedTickBehavior, timeout},
};
use uuid::{Uuid, Variant, Version};

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub const ACTIVE_PATH: &str = "/giveaway-scout/api/v1/active";
const MAX_BODY: usize = 512;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(7);
const MAX_CONNECTIONS: usize = 32;
type HttpResponse = Response<Full<Bytes>>;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Payload {
    pub installation_id: String,
    pub app_version: String,
}

pub fn validate_payload(body: &[u8]) -> Result<Payload, Error> {
    let payload: Payload = serde_json::from_slice(body)?;
    let id = Uuid::parse_str(&payload.installation_id)?;
    static VERSION: OnceLock<Regex> = OnceLock::new();
    let version = VERSION.get_or_init(|| {
        Regex::new(r"\A[0-9]{1,4}\.[0-9]{1,4}\.[0-9]{1,4}(?:-[a-zA-Z0-9.-]{1,24})?\z")
            .expect("constant version expression")
    });
    if id.get_variant() != Variant::RFC4122
        || id.get_version() != Some(Version::Random)
        || id.hyphenated().to_string() != payload.installation_id
        || !version.is_match(&payload.app_version)
    {
        return Err("Invalid identifier or version".into());
    }
    Ok(payload)
}

fn today() -> Date {
    OffsetDateTime::now_utc().date()
}

fn respond(status: StatusCode, body: impl Into<Bytes>, html: bool) -> HttpResponse {
    let body = body.into();
    let mut response = Response::builder()
        .status(status)
        .header(
            "Content-Type",
            if html {
                "text/html; charset=utf-8"
            } else {
                "application/json"
            },
        )
        .header("Cache-Control", "no-store")
        .header("Content-Length", body.len())
        .header("Connection", "close");
    if html {
        response = response.header("Content-Security-Policy", "default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; frame-ancestors 'none'");
    }
    response
        .body(Full::new(body))
        .expect("constant response headers")
}

fn empty(status: StatusCode) -> HttpResponse {
    respond(status, Bytes::new(), false)
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

pub fn dashboard(data: &Metrics) -> String {
    let cards: String = [
        (data.active_today, "Активны сегодня"),
        (data.active7_days, "Активны за 7 дней"),
        (data.active30_days, "Активны за 30 дней"),
        (data.first_seen7_days, "Впервые за 7 дней"),
        (data.observed_installations, "В пределах срока хранения"),
    ]
    .into_iter()
    .map(|(value, label)| {
        format!("<article><span>{label}</span><strong>{value}</strong></article>")
    })
    .collect();
    let versions: String = data
        .versions
        .iter()
        .map(|row| {
            format!(
                "<tr><td>{}</td><td>{}</td></tr>",
                escape(&row.version),
                row.installations
            )
        })
        .collect();
    let daily: String = data
        .daily
        .iter()
        .map(|row| {
            format!(
                "<tr><td>{}</td><td>{}</td></tr>",
                escape(&row.date),
                row.installations
            )
        })
        .collect();
    include_str!("../dashboard.html")
        .replace("__DATE__", &escape(&data.date_utc))
        .replace("__CARDS__", &cards)
        .replace("__VERSIONS__", &versions)
        .replace("__DAILY__", &daily)
}

async fn handle(request: Request<Incoming>, store: Arc<Store>) -> HttpResponse {
    let path = request
        .uri()
        .path_and_query()
        .map_or("/", |value| value.as_str())
        .to_owned();
    if request.method() == Method::POST {
        if path != ACTIVE_PATH {
            return empty(StatusCode::NOT_FOUND);
        }
        let json = request
            .headers()
            .get("Content-Type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| {
                v.split(';')
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .eq_ignore_ascii_case("application/json")
            });
        if !json || request.headers().contains_key("Transfer-Encoding") {
            return empty(StatusCode::UNSUPPORTED_MEDIA_TYPE);
        }
        let length = request
            .headers()
            .get("Content-Length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0);
        if length == 0 || length > MAX_BODY {
            return empty(StatusCode::PAYLOAD_TOO_LARGE);
        }
        let body = match timeout(
            REQUEST_TIMEOUT,
            Limited::new(request.into_body(), MAX_BODY).collect(),
        )
        .await
        {
            Ok(Ok(body)) => body.to_bytes(),
            _ => return empty(StatusCode::BAD_REQUEST),
        };
        let payload = match validate_payload(&body) {
            Ok(value) if body.len() == length => value,
            _ => return empty(StatusCode::BAD_REQUEST),
        };
        let result = tokio::task::spawn_blocking(move || {
            store.record(&payload.installation_id, &payload.app_version, today())
        })
        .await;
        return empty(if matches!(result, Ok(Ok(()))) {
            StatusCode::NO_CONTENT
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        });
    }
    if request.method() != Method::GET {
        return empty(StatusCode::METHOD_NOT_ALLOWED);
    }
    match path.as_str() {
        "/health" => respond(StatusCode::OK, r#"{"status":"ok"}"#, false),
        "/metrics" | "/dashboard/" => {
            match tokio::task::spawn_blocking(move || store.metrics(today())).await {
                Ok(Ok(data)) if path == "/dashboard/" => {
                    respond(StatusCode::OK, dashboard(&data), true)
                }
                Ok(Ok(data)) => match serde_json::to_vec(&data) {
                    Ok(body) => respond(StatusCode::OK, body, false),
                    Err(_) => empty(StatusCode::SERVICE_UNAVAILABLE),
                },
                _ => empty(StatusCode::SERVICE_UNAVAILABLE),
            }
        }
        ACTIVE_PATH => empty(StatusCode::METHOD_NOT_ALLOWED),
        _ => empty(StatusCode::NOT_FOUND),
    }
}

/// Loopback only, bounded connections, no access/request/error detail logging.
pub async fn serve(
    listener: TcpListener,
    store: Arc<Store>,
    shutdown: impl Future<Output = ()>,
) -> Result<(), Error> {
    if !listener.local_addr()?.ip().is_loopback() {
        return Err("Collector must listen on loopback".into());
    }
    let semaphore = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let mut tasks = JoinSet::new();
    let maintenance_store = Arc::clone(&store);
    let maintenance = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(3600));
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let store = Arc::clone(&maintenance_store);
            if !matches!(
                tokio::task::spawn_blocking(move || store.maintenance(today())).await,
                Ok(Ok(()))
            ) {
                eprintln!("Telemetry maintenance failed");
            }
        }
    });
    tokio::pin!(shutdown);
    let result = loop {
        tokio::select! {
            () = &mut shutdown => break Ok(()),
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {},
            accepted = async {
                let permit = Arc::clone(&semaphore).acquire_owned().await.expect("semaphore stays open");
                let (stream, _) = listener.accept().await?;
                Ok::<_, std::io::Error>((permit, stream))
            } => {
                let (permit, stream) = match accepted {
                    Ok(value) => value,
                    Err(error) => break Err(error.into()),
                };
                let store = Arc::clone(&store);
                tasks.spawn(async move {
                    let _permit = permit;
                    let service = service_fn(move |request| {
                        let store = Arc::clone(&store);
                        async move { Ok::<_, Infallible>(handle(request, store).await) }
                    });
                    // Close after one request. Neither malformed input nor a slow peer is logged.
                    let connection = http1::Builder::new().keep_alive(false).max_buf_size(8192)
                        .serve_connection(TokioIo::new(stream), service);
                    let _ = timeout(CONNECTION_TIMEOUT, connection).await;
                });
            }
        }
    };
    maintenance.abort();
    let _ = maintenance.await;
    while tasks.join_next().await.is_some() {}
    result
}
