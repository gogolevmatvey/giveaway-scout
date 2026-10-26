use std::time::Duration;

use giveaway_scout_core::storage::UsageStatisticsRequest;
use reqwest::{Client, StatusCode};
use tauri::{AppHandle, Manager};
use time::OffsetDateTime;

use crate::StorageState;

const ENDPOINT: &str = "https://historyalive.ru/giveaway-scout/api/v1/active";

fn allowed_environment(development: bool, ui_tests: bool) -> bool {
    !development && !ui_tests
}

fn payload(request: &UsageStatisticsRequest, version: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "installationId": request.installation_id,
        "appVersion": version,
    }))
    .expect("fixed string payload is serializable")
}

fn build_client() -> Result<Client, reqwest::Error> {
    Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(5))
        .user_agent("GiveawayScout-Usage/1")
        .build()
}

async fn send(
    client: &Client,
    request: &UsageStatisticsRequest,
    version: &str,
) -> Result<bool, reqwest::Error> {
    let response = client
        .post(ENDPOINT)
        .header("Content-Type", "application/json")
        .body(payload(request, version))
        .send()
        .await?;
    Ok(response.status() == StatusCode::NO_CONTENT)
}

pub(crate) fn start(app: &AppHandle) {
    if !allowed_environment(
        crate::development_profile(&app.config().identifier, tauri::is_dev()),
        cfg!(feature = "ui-tests"),
    ) {
        return;
    }
    let client = match build_client() {
        Ok(client) => client,
        Err(_) => {
            log::warn!("usage statistics transport is unavailable");
            return;
        }
    };
    let app = app.clone();
    tauri::async_runtime::spawn(run(app, client));
}

async fn run(app: AppHandle, client: Client) {
    loop {
        let request = {
            let storage = app.state::<StorageState>();
            storage.database.lock().ok().and_then(|database| {
                database
                    .usage_statistics_request(OffsetDateTime::now_utc())
                    .ok()
                    .flatten()
            })
        };
        if let Some(request) = request
            && send(&client, &request, env!("CARGO_PKG_VERSION"))
                .await
                .unwrap_or(false)
        {
            let storage = app.state::<StorageState>();
            if let Ok(database) = storage.database.lock() {
                let _ = database.usage_statistics_succeeded(&request);
            }
        }
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn development_and_ui_tests_are_never_counted() {
        assert!(allowed_environment(false, false));
        for (development, ui_tests) in [(true, false), (false, true), (true, true)] {
            assert!(!allowed_environment(development, ui_tests));
        }
    }

    #[test]
    fn payload_contains_only_random_id_and_app_version() {
        let request = UsageStatisticsRequest {
            installation_id: "random".into(),
            day: "2026-10-25".into(),
        };
        let value: serde_json::Value = serde_json::from_slice(&payload(&request, "1.0.0")).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"installationId":"random","appVersion":"1.0.0"})
        );
    }

    #[tokio::test]
    #[ignore = "manual HTTPS smoke test: creates a synthetic UUID; remove it on the owner server afterwards"]
    async fn live_https_transport_accepts_synthetic_installation() {
        let request = UsageStatisticsRequest {
            installation_id: "00000000-0000-4000-8000-000000000001".into(),
            day: OffsetDateTime::now_utc().date().to_string(),
        };
        assert!(
            send(&build_client().unwrap(), &request, "1.0.0-client-test")
                .await
                .unwrap()
        );
    }
}
