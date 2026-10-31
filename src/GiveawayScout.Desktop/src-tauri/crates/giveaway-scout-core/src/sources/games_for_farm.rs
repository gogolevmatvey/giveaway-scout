use scraper::{Html, Selector};
use url::Url;

use crate::models::{GameStore, GiveawayItem, GiveawayKind, GiveawaySource};

use super::{SourceParseError, item_url, normalize_text};

pub const GAMES_FOR_FARM_FEED_URL: &str = "https://freeanywhere.net/";

pub fn parse_games_for_farm(html: &str) -> Result<Vec<GiveawayItem>, SourceParseError> {
    let document = Html::parse_document(html);
    if document
        .select(&selector(".razdachi__card, .rozygryshi__card"))
        .next()
        .is_none()
    {
        return Err(SourceParseError::UnrecognizedFeed {
            feed: "Games for Farm",
        });
    }
    let base_url =
        Url::parse(GAMES_FOR_FARM_FEED_URL).expect("Games for Farm embedded feed URL is valid");
    let mut items = parse_cards(
        &document,
        &base_url,
        ".razdachi__card",
        GiveawayKind::Giveaway,
    )?;
    items.extend(parse_cards(
        &document,
        &base_url,
        ".rozygryshi__card",
        GiveawayKind::Raffle,
    )?);
    Ok(items)
}

fn parse_cards(
    document: &Html,
    base_url: &Url,
    card_selector: &str,
    kind: GiveawayKind,
) -> Result<Vec<GiveawayItem>, SourceParseError> {
    let card_selector = selector(card_selector);
    let title_selector = selector("a.card-content__title");
    let details_selector = selector(".card-content__feature");
    let platform_selector = selector(".card-content__platforms img[alt]");
    let mut items = Vec::new();

    for card in document.select(&card_selector) {
        let incomplete = |field| SourceParseError::IncompleteCard {
            feed: "Games for Farm",
            field,
        };
        let title_link = card
            .select(&title_selector)
            .next()
            .ok_or_else(|| incomplete("title link"))?;
        let relative_url = title_link
            .value()
            .attr("href")
            .filter(|href| !href.trim().is_empty())
            .ok_or_else(|| incomplete("link"))?;
        let url = item_url(base_url, relative_url, "Games for Farm")?;
        if url.origin() == base_url.origin() && url.path().to_ascii_lowercase().starts_with("/user")
        {
            continue;
        }
        let title = normalize_text(title_link.text()).ok_or_else(|| incomplete("title"))?;
        items.push(GiveawayItem {
            source: GiveawaySource::GamesForFarm,
            source_id: relative_url.to_owned(),
            store: card
                .select(&platform_selector)
                .filter_map(|element| element.value().attr("alt"))
                .find_map(
                    |platform| match platform.trim().to_ascii_lowercase().as_str() {
                        "steam" => Some(GameStore::Steam),
                        "epic" | "epicgames" | "epic games" | "epic games store" => {
                            Some(GameStore::EpicGames)
                        }
                        _ => None,
                    },
                )
                .unwrap_or_default(),
            title,
            url: url.to_string(),
            kind,
            details: card
                .select(&details_selector)
                .next()
                .and_then(|element| normalize_text(element.text())),
        });
    }

    Ok(items)
}

fn selector(value: &str) -> Selector {
    Selector::parse(value).expect("hard-coded selector should be valid")
}
