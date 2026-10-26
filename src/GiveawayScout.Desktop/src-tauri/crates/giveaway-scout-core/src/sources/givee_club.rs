use scraper::{Html, Selector};
use url::Url;

use crate::models::{GameStore, GiveawayItem, GiveawayKind, GiveawaySource};

use super::{SourceParseError, item_url, normalize_text};

pub const GIVEE_CLUB_FEED_URL: &str = "https://givee.club/ru";

pub fn parse_givee_club(html: &str) -> Result<Vec<GiveawayItem>, SourceParseError> {
    let document = Html::parse_document(html);
    let card_selector = selector(".event-card");
    if document.select(&card_selector).next().is_none() {
        return Err(SourceParseError::UnrecognizedFeed { feed: "Givee Club" });
    }
    let title_selector = selector(".event-title");
    let icon_selector = selector(".event-type-icon");
    let details_selector = selector(".event-type-label");
    let steam_selector = selector(".event-platform-steam");
    let epic_selector = selector(".event-platform-epicgames");
    let base_url = Url::parse(GIVEE_CLUB_FEED_URL).expect("Givee Club feed URL is valid");
    let mut items = Vec::new();

    for card in document.select(&card_selector) {
        let relative_url = card
            .value()
            .attr("href")
            .filter(|href| !href.trim().is_empty())
            .ok_or(SourceParseError::IncompleteCard {
                feed: "Givee Club",
                field: "link",
            })?;
        let title = card
            .select(&title_selector)
            .next()
            .and_then(|element| normalize_text(element.text()))
            .ok_or(SourceParseError::IncompleteCard {
                feed: "Givee Club",
                field: "title",
            })?;
        let is_raffle = card
            .select(&icon_selector)
            .next()
            .and_then(|element| element.value().attr("class"))
            .is_some_and(|classes| classes.to_ascii_lowercase().contains("raffle"));
        let url = item_url(&base_url, relative_url, "Givee Club")?;

        items.push(GiveawayItem {
            source: GiveawaySource::GiveeClub,
            source_id: relative_url.trim_matches('/').to_owned(),
            store: if card.select(&epic_selector).next().is_some() {
                GameStore::EpicGames
            } else if card.select(&steam_selector).next().is_some() {
                GameStore::Steam
            } else {
                GameStore::Unknown
            },
            title,
            url: url.to_string(),
            kind: if is_raffle {
                GiveawayKind::Raffle
            } else {
                GiveawayKind::Giveaway
            },
            details: card
                .select(&details_selector)
                .next()
                .and_then(|element| normalize_text(element.text())),
            legacy_tracking_key_prefixes: Vec::new(),
        });
    }

    Ok(items)
}

fn selector(value: &str) -> Selector {
    Selector::parse(value).expect("hard-coded selector should be valid")
}
