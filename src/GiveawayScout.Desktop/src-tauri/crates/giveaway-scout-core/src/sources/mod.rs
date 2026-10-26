mod games_for_farm;
mod givee_club;
mod zeros;

pub use games_for_farm::{GAMES_FOR_FARM_FEED_URL, parse_games_for_farm};
pub use givee_club::{GIVEE_CLUB_FEED_URL, parse_givee_club};
pub(crate) use zeros::fetch_zeros;
pub use zeros::{ZEROS_FEED_URL, parse_zeros};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SourceParseError {
    #[error("{feed}: invalid giveaway data: {message}")]
    InvalidData { feed: &'static str, message: String },
    #[error("{feed}: no recognizable giveaway cards; cannot confirm an empty feed")]
    UnrecognizedFeed { feed: &'static str },
    #[error("{feed}: incomplete giveaway card (missing {field}); page was not processed")]
    IncompleteCard {
        feed: &'static str,
        field: &'static str,
    },
    #[error("{feed}: giveaway link must use HTTP or HTTPS")]
    NonWebItemUrl { feed: &'static str },
    #[error("invalid item URL {href}: {source}")]
    InvalidItemUrl {
        href: String,
        #[source]
        source: url::ParseError,
    },
}

pub(super) fn item_url(
    base_url: &url::Url,
    href: &str,
    feed: &'static str,
) -> Result<url::Url, SourceParseError> {
    let url = base_url
        .join(href)
        .map_err(|source| SourceParseError::InvalidItemUrl {
            href: href.to_owned(),
            source,
        })?;
    if !matches!(url.scheme(), "https" | "http") || url.host_str().is_none() {
        return Err(SourceParseError::NonWebItemUrl { feed });
    }
    Ok(url)
}

pub(super) fn normalize_text<'a>(parts: impl Iterator<Item = &'a str>) -> Option<String> {
    let normalized = parts
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ");
    (!normalized.is_empty()).then_some(normalized)
}
