use giveaway_scout_core::{GiveawaySource, PageFetchError, SourceFailure, SourceFailureReason};

#[test]
fn stable_codes_do_not_depend_on_the_language_or_text_of_diagnostics() {
    for (error, expected) in [
        (
            PageFetchError::Timeout("technical timeout".into()),
            SourceFailureReason::Timeout,
        ),
        (
            PageFetchError::Connection("连接失败".into()),
            SourceFailureReason::Connection,
        ),
        (
            PageFetchError::Http {
                status: 429,
                message: "server text".into(),
            },
            SourceFailureReason::Http { status: 429 },
        ),
        (
            PageFetchError::ZerosConnectionLimit,
            SourceFailureReason::ZerosConnectionLimit,
        ),
        // Strings resembling a known error must not be parsed for localization.
        (
            PageFetchError::Message("timeout / one connection per IP".into()),
            SourceFailureReason::Unknown,
        ),
    ] {
        let technical = error.to_string();
        let failure = SourceFailure::from_fetch(GiveawaySource::Zeros, error);
        assert_eq!(failure.reason, expected);
        assert_eq!(failure.message, technical);
    }
}

#[test]
fn http_codes_are_taken_from_the_real_reqwest_error() {
    let response = tokio_tungstenite::tungstenite::http::Response::builder()
        .status(503)
        .body(String::new())
        .unwrap();
    let response = reqwest::Response::from(response);
    let error = response.error_for_status().unwrap_err();
    let failure = SourceFailure::from_fetch(GiveawaySource::GamesForFarm, error.into());
    assert_eq!(failure.reason, SourceFailureReason::Http { status: 503 });
    assert!(failure.message.contains("503"));
}

#[test]
fn ipc_uses_structured_reasons_and_old_payloads_fall_back_to_unknown() {
    let failure =
        SourceFailure::from_fetch(GiveawaySource::Zeros, PageFetchError::ZerosConnectionLimit);
    let json = serde_json::to_value(&failure).unwrap();
    assert_eq!(json["source"], "zeros");
    assert_eq!(json["reason"]["code"], "zerosConnectionLimit");
    let old: SourceFailure =
        serde_json::from_str(r#"{"source":"zeros","message":"legacy diagnostic"}"#).unwrap();
    assert_eq!(old.reason, SourceFailureReason::Unknown);
}
