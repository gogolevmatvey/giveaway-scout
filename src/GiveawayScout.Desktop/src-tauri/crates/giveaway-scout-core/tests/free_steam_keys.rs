use giveaway_scout_core::{GameStore, GiveawayKind, GiveawaySource, parse_free_steam_keys};
use serde_json::{Value, json};

const CAPTURED_FEED: &str = include_str!("fixtures/json/free-steam-keys.json");

fn row() -> Value {
    serde_json::from_str::<Vec<Value>>(CAPTURED_FEED)
        .unwrap()
        .remove(0)
}

fn parse(
    rows: Vec<Value>,
) -> Result<Vec<giveaway_scout_core::GiveawayItem>, giveaway_scout_core::SourceParseError> {
    parse_free_steam_keys(&serde_json::to_string(&rows).unwrap())
}

#[test]
fn captured_gleam_giveaways_keep_api_order_and_browser_redirect_links() {
    // Public API capture, 2026-11-07. No Gleam requests are needed to discover them.
    let items = parse_free_steam_keys(CAPTURED_FEED).unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].title, "Sorcerer Adventure");
    assert_eq!(items[0].source_id, "258");
    assert_eq!(items[0].tracking_key(), "FreeSteamKeys:258");
    assert_eq!(items[1].title, "One Gun Guy");
    assert_eq!(items[1].source_id, "257");
    assert_eq!(
        items[1].url,
        "https://www.freesteamkeys.com/open/one-gun-guy"
    );
    for item in items {
        assert_eq!(item.source, GiveawaySource::FreeSteamKeys);
        assert_eq!(item.kind, GiveawayKind::Giveaway);
        assert_eq!(item.store, GameStore::Steam);
        assert!(item.details.unwrap().to_ascii_lowercase().contains("gleam"));
    }
}

#[test]
fn tracking_uses_numeric_id_even_when_title_slug_and_description_change() {
    let first = parse(vec![row()]).unwrap().remove(0);
    let mut updated = row();
    updated["title"] = json!("Corrected game name");
    updated["description"] = json!("Updated conditions");
    updated["giveaway_url"] = json!("https://www.freesteamkeys.com/open/corrected-name");
    let updated = parse(vec![updated]).unwrap().remove(0);
    assert_eq!(first.tracking_key(), updated.tracking_key());
}

#[test]
fn documented_url_field_is_supported_only_when_current_field_is_absent() {
    let mut documented = json!({
        "id": 10,
        "title": "Cyberpunk Arena Giveaway",
        "type": "Game",
        "status": "Active",
        "url": "https://example.com/giveaway"
    });
    assert_eq!(
        parse(vec![documented.clone()]).unwrap()[0].url,
        "https://example.com/giveaway"
    );
    documented["giveaway_url"] = json!("https://example.com/current");
    assert_eq!(
        parse(vec![documented.clone()]).unwrap()[0].url,
        "https://example.com/current"
    );
    documented["giveaway_url"] = json!("");
    assert!(parse(vec![documented]).is_err());
}

#[test]
fn description_is_optional_and_text_whitespace_is_normalized() {
    let mut value = row();
    value["title"] = json!("  Some\n game\t title  ");
    value["description"] = json!("  Complete\n two\t tasks.  ");
    let item = parse(vec![value.clone()]).unwrap().remove(0);
    assert_eq!(item.title, "Some game title");
    assert_eq!(item.details.as_deref(), Some("Complete two tasks."));
    for description in [Value::Null, json!(" \n ")] {
        value["description"] = description;
        assert!(parse(vec![value.clone()]).unwrap()[0].details.is_none());
    }
    value.as_object_mut().unwrap().remove("description");
    value.as_object_mut().unwrap().remove("redemption");
    assert_eq!(parse(vec![value]).unwrap()[0].store, GameStore::Steam);
}

#[test]
fn empty_array_and_explicitly_inactive_or_non_game_items_are_valid_empty_results() {
    assert!(parse_free_steam_keys("[]").unwrap().is_empty());
    for status in ["Expired", "Ended", "Inactive", "Closed"] {
        assert!(
            parse(vec![json!({"id": 1, "type": "Game", "status": status})])
                .unwrap()
                .is_empty()
        );
    }
    for kind in ["DLC", "Demo", "Playtest"] {
        assert!(
            parse(vec![json!({"id": 1, "type": kind, "status": "Active"})])
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn malformed_responses_are_errors_instead_of_empty_or_partial_success() {
    for text in [
        "",
        " ",
        "{}",
        "null",
        "[",
        r#"{"error":"maintenance"}"#,
        "<html>Cloudflare</html>",
    ] {
        assert!(parse_free_steam_keys(text).is_err(), "accepted {text:?}");
    }
    for (field, value) in [
        ("id", json!(0)),
        ("id", json!(-1)),
        ("id", json!(1.5)),
        ("id", json!("258")),
        ("id", Value::Null),
        ("title", json!(" \t ")),
        ("title", Value::Null),
        ("title", json!(123)),
        ("status", json!("Paused?")),
        ("status", json!("")),
        ("status", Value::Null),
        ("type", json!("Software?")),
        ("type", Value::Null),
        ("giveaway_url", json!("")),
        ("giveaway_url", json!("/open/sorcerer-adventure")),
        ("giveaway_url", json!("not a url")),
        ("giveaway_url", json!("javascript:alert(1)")),
        ("giveaway_url", json!("file:///tmp/a")),
        ("giveaway_url", json!("ftp://example.com/a")),
        ("giveaway_url", Value::Null),
        ("description", json!(123)),
    ] {
        let mut invalid = row();
        invalid["id"] = json!(999);
        invalid[field] = value;
        assert!(
            parse(vec![row(), invalid]).is_err(),
            "accepted invalid {field}"
        );
    }
    for field in ["id", "type", "status", "title", "giveaway_url"] {
        let mut invalid = row();
        invalid.as_object_mut().unwrap().remove(field);
        assert!(parse(vec![invalid]).is_err(), "accepted missing {field}");
    }
}

#[test]
fn repeated_identical_ids_are_deduplicated_and_conflicting_ids_reject_the_feed() {
    assert_eq!(parse(vec![row(), row()]).unwrap().len(), 1);
    let mut changed = row();
    changed["title"] = json!("Conflicting title");
    assert!(parse(vec![row(), changed]).is_err());
    let mut changed = row();
    changed["status"] = json!("Expired");
    assert!(parse(vec![row(), changed]).is_err());
}
