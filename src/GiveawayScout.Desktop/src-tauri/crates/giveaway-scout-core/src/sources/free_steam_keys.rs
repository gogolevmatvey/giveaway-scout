use std::collections::HashMap;

use serde::Deserialize;
use url::Url;

use crate::models::{GameStore, GiveawayItem, GiveawayKind, GiveawaySource};

use super::{SourceParseError, normalize_text};

pub const FREE_STEAM_KEYS_FEED_URL: &str =
    "https://www.freesteamkeys.com/api/giveaways?type=game&sort=date";

const FEED: &str = "FreeSteamKeys";

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct FeedItem {
    id: u64,
    #[serde(rename = "type")]
    giveaway_type: String,
    status: String,
    title: Option<String>,
    description: Option<String>,
    giveaway_url: Option<String>,
    // The public API documentation uses `url`; current responses use `giveaway_url`.
    url: Option<String>,
}

pub fn parse_free_steam_keys(json: &str) -> Result<Vec<GiveawayItem>, SourceParseError> {
    let rows: Vec<FeedItem> = serde_json::from_str(json)
        .map_err(|error| invalid(format!("invalid JSON giveaway list: {error}")))?;
    let mut seen = HashMap::new();
    let mut items = Vec::new();

    for row in &rows {
        if row.id == 0 {
            return Err(invalid("giveaway ID must be a positive integer"));
        }
        let active = match row.status.trim().to_ascii_lowercase().as_str() {
            "active" => true,
            "expired" | "ended" | "inactive" | "closed" => false,
            _ => return Err(invalid(format!("unknown giveaway status: {}", row.status))),
        };
        let full_game = match row.giveaway_type.trim().to_ascii_lowercase().as_str() {
            "game" => true,
            "dlc" | "demo" | "playtest" => false,
            _ => {
                return Err(invalid(format!(
                    "unknown giveaway type: {}",
                    row.giveaway_type
                )));
            }
        };
        if let Some(previous) = seen.insert(row.id, row) {
            if previous != row {
                return Err(invalid(format!("conflicting giveaway ID: {}", row.id)));
            }
            continue;
        }
        if !active || !full_game {
            continue;
        }

        let incomplete = |field| SourceParseError::IncompleteCard { feed: FEED, field };
        let title = row
            .title
            .as_deref()
            .and_then(|title| normalize_text(std::iter::once(title)))
            .ok_or_else(|| incomplete("title"))?;
        let href = row
            .giveaway_url
            .as_deref()
            .or(row.url.as_deref())
            .filter(|href| !href.trim().is_empty())
            .ok_or_else(|| incomplete("giveaway URL"))?;
        let url = Url::parse(href.trim()).map_err(|source| SourceParseError::InvalidItemUrl {
            href: href.to_owned(),
            source,
        })?;
        if !matches!(url.scheme(), "https" | "http") || url.host_str().is_none() {
            return Err(SourceParseError::NonWebItemUrl { feed: FEED });
        }

        items.push(GiveawayItem {
            source: GiveawaySource::FreeSteamKeys,
            source_id: row.id.to_string(),
            title,
            url: url.to_string(),
            kind: GiveawayKind::Giveaway,
            // The endpoint explicitly returns Steam giveaways; redemption is optional.
            store: GameStore::Steam,
            details: row
                .description
                .as_deref()
                .and_then(|text| normalize_text(std::iter::once(text))),
        });
    }

    Ok(items)
}

fn invalid(message: impl Into<String>) -> SourceParseError {
    SourceParseError::InvalidData {
        feed: FEED,
        message: message.into(),
    }
}
