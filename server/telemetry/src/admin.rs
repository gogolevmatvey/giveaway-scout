//! Explicit manual commands, never run by normal tests or the collector.
use crate::{ACTIVE_PATH, Error, store::installation_key};
use reqwest::{Client, StatusCode, redirect::Policy};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
use uuid::Uuid;

const ORIGIN: &str = "https://historyalive.ru";
const DATABASE: &str = "/var/lib/giveaway-scout-telemetry/usage.sqlite3";
const SETTINGS: &str = "/etc/giveaway-scout-telemetry";
const LOCAL: &str = "http://127.0.0.1:8793";

fn client(https: bool) -> Result<Client, Error> {
    Ok(Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(Policy::none())
        .https_only(https)
        .no_proxy()
        .build()?)
}

fn connect(database: &Path) -> Result<Connection, Error> {
    // Administrative commands must never accidentally create a new database.
    let connection = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.execute_batch("PRAGMA foreign_keys=ON;")?;
    Ok(connection)
}

fn require(condition: bool, message: &'static str) -> Result<(), Error> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

pub fn remove_client_probe(database: &Path, salt: &[u8]) -> Result<(), Error> {
    let key = installation_key(salt, "00000000-0000-4000-8000-000000000001");
    let mut connection = connect(database)?;
    let transaction = connection.transaction()?;
    let version: Option<String> = transaction
        .query_row(
            "SELECT version FROM installations WHERE id=?",
            [&key],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(version) = version {
        require(
            version == "1.0.0-client-test",
            "Refusing to remove an unexpected version",
        )?;
        transaction.execute("DELETE FROM installations WHERE id=?", [&key])?;
    }
    transaction.commit()?;
    Ok(())
}

pub async fn cleanup_client_probe() -> Result<Value, Error> {
    let salt = std::fs::read(Path::new(SETTINGS).join("salt"))?;
    remove_client_probe(Path::new(DATABASE), &salt)?;
    let metrics: Value = client(false)?
        .get(format!("{LOCAL}/metrics"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(json!({"syntheticClientFixtureRemoved":true,"metrics":metrics}))
}

pub async fn verify_live() -> Result<Value, Error> {
    let public = client(true)?;
    let local = client(false)?;
    let dashboard = format!("{ORIGIN}/giveaway-scout/statistics/");
    require(
        public.get(&dashboard).send().await?.status() == StatusCode::UNAUTHORIZED,
        "Statistics must require separate authentication",
    )?;
    let password = std::fs::read_to_string(Path::new(SETTINGS).join("dashboard-password"))?;
    let response = public
        .get(&dashboard)
        .basic_auth("giveaway-scout", Some(password.trim()))
        .send()
        .await?;
    require(
        response.status() == StatusCode::OK,
        "Authenticated dashboard failed",
    )?;
    require(
        response.text().await?.contains("Активны за 30 дней"),
        "Dashboard content missing",
    )?;

    let identifier = Uuid::new_v4().hyphenated().to_string();
    let key = installation_key(
        &std::fs::read(Path::new(SETTINGS).join("salt"))?,
        &identifier,
    );
    // Always clean up just our own synthetic installation, including after a failed request.
    let result: Result<(), Error> = async {
        let payload = json!({"installationId":identifier,"appVersion":"1.0.0-live-test"});
        for _ in 0..2 {
            let response = public
                .post(format!("{ORIGIN}{ACTIVE_PATH}"))
                .json(&payload)
                .send()
                .await?;
            require(
                response.status() == StatusCode::NO_CONTENT,
                "HTTPS collector rejected fixture",
            )?;
        }
        let metrics: Value = local
            .get(format!("{LOCAL}/metrics"))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        require(
            metrics["activeToday"]
                .as_i64()
                .is_some_and(|value| value >= 1),
            "Synthetic activity missing from metrics",
        )?;
        let connection = connect(Path::new(DATABASE))?;
        let rows: i64 =
            connection.query_row("SELECT COUNT(*) FROM activity WHERE id=?", [&key], |row| {
                row.get(0)
            })?;
        require(rows == 1, "Daily duplicates must not increase counters")?;
        let response = public
            .post(format!("{ORIGIN}{ACTIVE_PATH}"))
            .json(&json!({"history":[]}))
            .send()
            .await?;
        require(
            response.status() == StatusCode::BAD_REQUEST,
            "Unexpected fields accepted",
        )?;
        Ok(())
    }
    .await;
    {
        let connection = connect(Path::new(DATABASE))?;
        connection.execute("DELETE FROM installations WHERE id=?", [&key])?;
        let rows: i64 =
            connection.query_row("SELECT COUNT(*) FROM activity WHERE id=?", [&key], |row| {
                row.get(0)
            })?;
        require(rows == 0, "Synthetic activity was not removed")?;
    }
    result?;
    let paths = ["/", "/login/", "/robots.txt", "/wp-json/"];
    for path in paths {
        require(
            public.get(format!("{ORIGIN}{path}")).send().await?.status() == StatusCode::OK,
            "Existing site check failed",
        )?;
    }
    let metrics: Value = local
        .get(format!("{LOCAL}/metrics"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(
        json!({"httpsCollector":true,"deduplication":true,"separateDashboardAuth":true,
        "invalidPayloadRejected":true,"syntheticDataRemoved":true,"siteChecks":paths,"currentMetrics":metrics}),
    )
}
