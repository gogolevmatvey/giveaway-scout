use giveaway_scout_telemetry::{
    ACTIVE_PATH, dashboard, serve,
    store::{Metrics, Store},
    validate_payload,
};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::sync::Arc;
use tempfile::TempDir;
use time::{Date, Duration, Month};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
};

const ID: &str = "11111111-1111-4111-8111-111111111111";
const OTHER: &str = "22222222-2222-4222-8222-222222222222";

fn date() -> Date {
    Date::from_calendar_date(2026, Month::October, 25).unwrap()
}

fn store() -> (TempDir, Arc<Store>) {
    let temporary = TempDir::new().unwrap();
    let store = Store::open(&temporary.path().join("usage.sqlite3"), vec![b'x'; 32]).unwrap();
    (temporary, Arc::new(store))
}

#[test]
fn existing_python_database_and_hmac_keys_are_reused_without_duplicates() {
    let temporary = TempDir::new().unwrap();
    let database = temporary.path().join("usage.sqlite3");
    {
        let connection = Connection::open(&database).unwrap();
        // Python's iterdump emits activity before installations; restore with FK checks off.
        connection
            .execute_batch("PRAGMA foreign_keys=OFF;")
            .unwrap();
        connection
            .execute_batch(include_str!("fixtures/python-v1.sql"))
            .unwrap();
    }
    let store = Store::open(&database, vec![b'x'; 32]).unwrap();
    let expected: Metrics =
        serde_json::from_str(include_str!("fixtures/python-v1-metrics.json")).unwrap();
    assert_eq!(store.metrics(date()).unwrap(), expected);
    store.record(ID, "1.0.1", date()).unwrap();
    assert_eq!(
        store.metrics(date()).unwrap(),
        expected,
        "same salt must produce the same HMAC key as Python"
    );
    drop(store);
    let reopened = Store::open(&database, vec![b'x'; 32]).unwrap();
    assert_eq!(
        reopened.metrics(date()).unwrap(),
        expected,
        "data survives restart"
    );
    let connection = Connection::open(&database).unwrap();
    let keys: Vec<String> = connection
        .prepare("SELECT id FROM installations")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(keys.len(), 2);
    assert!(
        keys.iter()
            .all(|key| key.len() == 64 && key != ID && key != OTHER)
    );
}

#[test]
fn deduplicates_daily_activity_and_keeps_first_seen_when_version_changes() {
    let (_temporary, store) = store();
    store
        .record(ID, "1.0.0", date() - Duration::days(10))
        .unwrap();
    store.record(ID, "1.0.0", date()).unwrap();
    store.record(ID, "1.0.1", date()).unwrap();
    store
        .record(OTHER, "1.0.0", date() - Duration::days(5))
        .unwrap();
    let data = store.metrics(date()).unwrap();
    assert_eq!(
        (
            data.observed_installations,
            data.active_today,
            data.active7_days,
            data.active30_days,
            data.first_seen7_days
        ),
        (2, 1, 2, 2, 1)
    );
    assert_eq!(data.daily[0].installations, 1);
    assert_eq!(data.versions.len(), 2);
}

#[test]
fn retention_boundary_is_90_days_and_maintenance_cascades_inactive_installations() {
    let (temporary, store) = store();
    store
        .record(ID, "1.0.0", date() - Duration::days(90))
        .unwrap();
    store
        .record(OTHER, "1.0.0", date() - Duration::days(89))
        .unwrap();
    store.maintenance(date()).unwrap();
    let data = store.metrics(date()).unwrap();
    assert_eq!(data.observed_installations, 1);
    assert_eq!(data.active30_days, 0);
    let connection = Connection::open(temporary.path().join("usage.sqlite3")).unwrap();
    let rows: i64 = connection
        .query_row("SELECT COUNT(*) FROM activity", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 1);
}

#[test]
fn activity_windows_include_exactly_7_and_30_calendar_days() {
    let (_temporary, store) = store();
    for (id, days) in [
        (ID, 6),
        (OTHER, 7),
        ("33333333-3333-4333-8333-333333333333", 29),
        ("44444444-4444-4444-8444-444444444444", 30),
    ] {
        store
            .record(id, "1.0.0", date() - Duration::days(days))
            .unwrap();
    }
    let data = store.metrics(date()).unwrap();
    assert_eq!(
        (data.active7_days, data.active30_days, data.first_seen7_days),
        (1, 3, 1)
    );
    assert_eq!(data.daily.len(), 3);
}

#[test]
fn concurrent_duplicate_posts_cannot_inflate_counters() {
    let (_temporary, store) = store();
    std::thread::scope(|scope| {
        for _ in 0..16 {
            let store = Arc::clone(&store);
            scope.spawn(move || store.record(ID, "1.0.0", date()).unwrap());
        }
    });
    let data = store.metrics(date()).unwrap();
    assert_eq!(
        (
            data.observed_installations,
            data.active_today,
            data.daily[0].installations
        ),
        (1, 1, 1)
    );
}

#[test]
fn rejects_unknown_fields_invalid_types_uuid_variants_and_versions() {
    let valid = json!({"installationId": ID, "appVersion": "1.0.0"});
    validate_payload(&serde_json::to_vec(&valid).unwrap()).unwrap();
    for invalid in [
        Value::Null,
        json!([]),
        json!({}),
        json!({"installationId": ID, "appVersion": "1.0.0", "history": []}),
        json!({"installationId": true, "appVersion": "1.0.0"}),
        json!({"installationId": "computer-name", "appVersion": "1.0.0"}),
        json!({"installationId": "ABCDEFAB-1111-4111-8111-111111111111", "appVersion": "1.0.0"}),
        json!({"installationId": "11111111111141118111111111111111", "appVersion": "1.0.0"}),
        json!({"installationId": "11111111-1111-1111-8111-111111111111", "appVersion": "1.0.0"}),
        json!({"installationId": "11111111-1111-4111-1111-111111111111", "appVersion": "1.0.0"}),
        json!({"installationId": ID, "appVersion": "<script>"}),
        json!({"installationId": ID, "appVersion": "1.0.0\n"}),
        json!({"installationId": ID, "appVersion": "10000.0.0"}),
        json!({"installationId": ID, "appVersion": format!("1.0.0-{}", "a".repeat(25))}),
    ] {
        assert!(validate_payload(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }
    assert!(
        validate_payload(
            format!(r#"{{"installationId":"{ID}","appVersion":"1.0.0","appVersion":"2.0.0"}}"#)
                .as_bytes()
        )
        .is_err()
    );
    assert!(validate_payload(&[0xff]).is_err());
}

#[test]
fn fails_closed_without_a_sufficient_server_salt() {
    let temporary = TempDir::new().unwrap();
    assert!(Store::open(&temporary.path().join("usage.sqlite3"), vec![0; 31]).is_err());
    assert!(!temporary.path().join("usage.sqlite3").exists());
}

#[test]
fn dashboard_escapes_database_text_and_retains_the_existing_template() {
    let mut data: Metrics =
        serde_json::from_str(include_str!("fixtures/python-v1-metrics.json")).unwrap();
    data.versions[0].version = "<script>alert('x')</script>".into();
    let page = dashboard(&data);
    assert!(page.contains("Активны за 30 дней"));
    assert!(!page.contains("<script>"));
    assert!(page.contains("&lt;script&gt;"));
    assert!(!page.contains("__CARDS__"));
    assert!(!page.contains(ID));
}

#[test]
fn manual_probe_cleanup_removes_only_the_expected_synthetic_version() {
    let (temporary, store) = store();
    let probe = "00000000-0000-4000-8000-000000000001";
    let database = temporary.path().join("usage.sqlite3");
    store.record(ID, "1.0.0", date()).unwrap();
    store.record(probe, "1.0.0-client-test", date()).unwrap();
    giveaway_scout_telemetry::admin::remove_client_probe(&database, &[b'x'; 32]).unwrap();
    let metrics = store.metrics(date()).unwrap();
    assert_eq!(
        (metrics.observed_installations, metrics.active_today),
        (1, 1)
    );
    store.record(probe, "1.0.0", date()).unwrap();
    assert!(giveaway_scout_telemetry::admin::remove_client_probe(&database, &[b'x'; 32]).is_err());
    assert_eq!(store.metrics(date()).unwrap().observed_installations, 2);
    let missing = temporary.path().join("missing.sqlite3");
    assert!(giveaway_scout_telemetry::admin::remove_client_probe(&missing, &[b'x'; 32]).is_err());
    assert!(!missing.exists());
}

struct HttpResponse {
    status: u16,
    headers: String,
    body: Vec<u8>,
}

async fn request(
    address: std::net::SocketAddr,
    method: &str,
    path: &str,
    body: &[u8],
    extra: &str,
) -> HttpResponse {
    let headers = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n{extra}\r\n",
        body.len()
    );
    let mut wire = headers.into_bytes();
    wire.extend_from_slice(body);
    raw_request(address, &wire).await
}

async fn raw_request(address: std::net::SocketAddr, wire: &[u8]) -> HttpResponse {
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream.write_all(wire).await.unwrap();
    let mut bytes = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        stream.read_to_end(&mut bytes),
    )
    .await
    .unwrap()
    .unwrap();
    let split = bytes
        .windows(4)
        .position(|chunk| chunk == b"\r\n\r\n")
        .unwrap();
    let headers = String::from_utf8(bytes[..split].to_vec()).unwrap();
    let status = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
    HttpResponse {
        status,
        headers: headers.to_lowercase(),
        body: bytes[split + 4..].to_vec(),
    }
}

#[tokio::test]
async fn real_http_contract_preserves_routes_validation_dashboard_and_daily_deduplication() {
    let (_temporary, store) = store();
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(serve(listener, store, async {
        let _ = stopped.await;
    }));
    let payload = serde_json::to_vec(&json!({"installationId":ID, "appVersion":"1.0.0"})).unwrap();
    for _ in 0..2 {
        assert_eq!(
            request(
                address,
                "POST",
                ACTIVE_PATH,
                &payload,
                "Content-Type: application/json\r\n"
            )
            .await
            .status,
            204
        );
    }
    assert_eq!(
        request(
            address,
            "POST",
            ACTIVE_PATH,
            &[b'x'; 513],
            "Content-Type: application/json\r\n"
        )
        .await
        .status,
        413
    );
    assert_eq!(
        request(
            address,
            "POST",
            ACTIVE_PATH,
            b"",
            "Content-Type: application/json\r\n"
        )
        .await
        .status,
        413
    );
    assert_eq!(
        request(address, "POST", ACTIVE_PATH, &payload, "")
            .await
            .status,
        415
    );
    assert_eq!(
        request(
            address,
            "POST",
            ACTIVE_PATH,
            b"{}",
            "Content-Type: application/json\r\n"
        )
        .await
        .status,
        400
    );
    assert_eq!(
        request(
            address,
            "POST",
            ACTIVE_PATH,
            b"{",
            "Content-Type: application/json\r\n"
        )
        .await
        .status,
        400
    );
    assert_eq!(
        request(
            address,
            "POST",
            "/unknown",
            &payload,
            "Content-Type: application/json\r\n"
        )
        .await
        .status,
        404
    );
    assert_eq!(
        request(
            address,
            "POST",
            &format!("{ACTIVE_PATH}?x=1"),
            &payload,
            "Content-Type: application/json\r\n"
        )
        .await
        .status,
        404
    );
    assert_eq!(
        request(address, "GET", ACTIVE_PATH, b"", "").await.status,
        405
    );
    assert_eq!(
        request(address, "GET", "/unknown", b"", "").await.status,
        404
    );
    assert_eq!(
        request(address, "DELETE", ACTIVE_PATH, b"", "")
            .await
            .status,
        405
    );
    assert_eq!(
        request(address, "GET", "/health", b"", "").await.status,
        200
    );
    let metrics = request(address, "GET", "/metrics", b"", "").await;
    assert_eq!(metrics.status, 200);
    assert!(metrics.headers.contains("cache-control: no-store"));
    let data: Value = serde_json::from_slice(&metrics.body).unwrap();
    assert_eq!(data["activeToday"], 1);
    assert_eq!(data["active7Days"], 1);
    assert_eq!(data["active30Days"], 1);
    assert_eq!(data["firstSeen7Days"], 1);
    assert_eq!(data["retentionDays"], 90);
    let chunked = format!(
        "POST {ACTIVE_PATH} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{{}}\r\n0\r\n\r\n"
    );
    assert_eq!(raw_request(address, chunked.as_bytes()).await.status, 415);
    let missing_length = format!(
        "POST {ACTIVE_PATH} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\r\n"
    );
    assert_eq!(
        raw_request(address, missing_length.as_bytes()).await.status,
        413
    );
    let dashboard = request(address, "GET", "/dashboard/", b"", "").await;
    assert_eq!(dashboard.status, 200);
    assert!(
        dashboard
            .headers
            .contains("content-type: text/html; charset=utf-8")
    );
    assert!(dashboard.headers.contains("frame-ancestors 'none'"));
    let page = String::from_utf8(dashboard.body).unwrap();
    assert!(page.contains("Активны за 30 дней"));
    assert!(!page.contains(ID));
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn refuses_a_listener_exposed_to_other_hosts() {
    let (_temporary, store) = store();
    let listener = TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, 0))
        .await
        .unwrap();
    assert!(
        serve(listener, store, std::future::pending())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn database_failure_returns_503_without_leaking_internal_error_details() {
    let (temporary, store) = store();
    let connection = Connection::open(temporary.path().join("usage.sqlite3")).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_insert BEFORE INSERT ON installations BEGIN SELECT RAISE(ABORT, 'private database detail'); END;").unwrap();
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(serve(listener, store, async {
        let _ = stopped.await;
    }));
    let payload = serde_json::to_vec(&json!({"installationId":ID, "appVersion":"1.0.0"})).unwrap();
    let response = request(
        address,
        "POST",
        ACTIVE_PATH,
        &payload,
        "Content-Type: application/json\r\n",
    )
    .await;
    assert_eq!(response.status, 503);
    assert!(response.body.is_empty());
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}
