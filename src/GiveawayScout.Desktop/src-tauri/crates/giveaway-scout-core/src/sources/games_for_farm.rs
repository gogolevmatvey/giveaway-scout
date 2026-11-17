use scraper::{ElementRef, Html, Selector};
use url::Url;

use crate::models::{GameStore, GiveawayItem, GiveawayKind, GiveawaySource};

use super::{SourceParseError, item_url, normalize_text};

pub const GAMES_FOR_FARM_FEED_URL: &str = "https://freeanywhere.net/";

pub fn parse_games_for_farm(html: &str) -> Result<Vec<GiveawayItem>, SourceParseError> {
    let document = Html::parse_document(html);
    let card_selector = selector(".razdachi__card, .rozygryshi__card");
    if document.select(&card_selector).next().is_none() {
        if is_known_empty_home(html, &document) {
            return Ok(Vec::new());
        }
        return Err(SourceParseError::UnrecognizedFeed {
            feed: "Games for Farm",
        });
    }
    let base_url =
        Url::parse(GAMES_FOR_FARM_FEED_URL).expect("Games for Farm embedded feed URL is valid");
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
        let url = item_url(&base_url, relative_url, "Games for Farm")?;
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
                        "gog" | "gog.com" => Some(GameStore::Gog),
                        "epic" | "epicgames" | "epic games" | "epic games store" => {
                            Some(GameStore::EpicGames)
                        }
                        _ => None,
                    },
                )
                .unwrap_or_default(),
            title,
            url: url.to_string(),
            kind: if card
                .value()
                .classes()
                .any(|class| class == "rozygryshi__card")
            {
                GiveawayKind::Raffle
            } else {
                GiveawayKind::Giveaway
            },
            details: card
                .select(&details_selector)
                .next()
                .and_then(|element| normalize_text(element.text())),
        });
    }

    Ok(items)
}

// The site's home page omits both active lists when they are empty. Do not
// interpret arbitrary card-free HTML as success: that would also complete the
// initial baseline for an error page. This is the complete, observed no-feed
// content structure; localized text is deliberately not part of its signature.
const EMPTY_HOME_CONTENT: &str = r#"
<div class="page_content">
  <section class="main_banner">
    <div class="container"><div class="main_banner__area"><div class="main_banner__wrapper">
      <div class="main_banner__title"><h1>_</h1></div>
      <div class="main_banner__decs"><span>_ <a href="https://gamesforfarm.com">_</a></span></div>
    </div></div></div>
  </section>
  <section class="history_buttons">
    <div class="container"><div class="history_buttons__list">
      <a href="/giveaways" class="history_buttons__element">
        <div class="history_buttons__image"><img></div><div class="history_buttons__title">_</div>
      </a>
      <a href="/drawings" class="history_buttons__element">
        <div class="history_buttons__image"><img></div><div class="history_buttons__title">_</div>
      </a>
    </div></div>
  </section>
</div>
"#;

fn is_known_empty_home(html: &str, document: &Html) -> bool {
    // Html's parser repairs truncated documents. A repaired shell must not
    // masquerade as the complete homepage received from the server.
    let lowercase = html.to_ascii_lowercase();
    if !lowercase.trim_end().ends_with("</html>")
        || !lowercase.contains("</head>")
        || !lowercase.contains("</body>")
    {
        return false;
    }
    for required in [
        "html > body.page-index > header.games_for_farm_site",
        "html > body.page-index > footer",
        "html > body.page-index > .page_modals",
    ] {
        if document.select(&selector(required)).count() != 1 {
            return false;
        }
    }
    let body_selector = selector("html > body.page-index");
    let Some(body) = document.select(&body_selector).next() else {
        return false;
    };
    // A maintenance banner or verification form can be a sibling of the main
    // content. Only the observed site's surrounding components are allowed.
    for child in body.children() {
        if child
            .value()
            .as_text()
            .is_some_and(|text| !text.trim().is_empty())
        {
            return false;
        }
        let Some(element) = ElementRef::wrap(child) else {
            continue;
        };
        let mut classes: Vec<_> = element.value().classes().collect();
        classes.sort_unstable();
        let known = match element.value().name() {
            "script" | "style" => true,
            "header" => classes == ["games_for_farm_site", "header"],
            "footer" => classes.is_empty(),
            "div" => {
                matches!(
                    classes.as_slice(),
                    ["js-preloader"]
                        | ["events"]
                        | ["mobile-nav"]
                        | ["page_content"]
                        | ["page_modals"]
                ) || (classes.is_empty()
                    && matches!(element.value().id(), Some("sbbhscc" | "sbbfrcc"))
                    && element
                        .children()
                        .filter_map(ElementRef::wrap)
                        .next()
                        .is_none()
                    && normalize_text(element.text()).is_none())
            }
            _ => false,
        };
        if !known {
            return false;
        }
    }
    let canonical_selector = selector("html > head > link[rel='canonical']");
    let mut canonicals = document.select(&canonical_selector);
    let Some(canonical) = canonicals
        .next()
        .and_then(|element| element.value().attr("href"))
    else {
        return false;
    };
    let Ok(base_url) = Url::parse(canonical) else {
        return false;
    };
    if canonicals.next().is_some() || base_url.as_str() != "https://give.gamesforfarm.com/" {
        return false;
    }
    let content_selector = selector("html > body.page-index > .page_content");
    let mut contents = document.select(&content_selector);
    let Some(content) = contents.next() else {
        return false;
    };
    if contents.next().is_some() {
        return false;
    }
    let expected = Html::parse_fragment(EMPTY_HOME_CONTENT);
    let expected_content = expected
        .select(&selector(".page_content"))
        .next()
        .expect("embedded empty homepage structure is valid");
    matches_empty_home_structure(content, expected_content, &base_url)
}

fn matches_empty_home_structure(
    actual: ElementRef<'_>,
    expected: ElementRef<'_>,
    base: &Url,
) -> bool {
    let mut actual_classes: Vec<_> = actual.value().classes().collect();
    let mut expected_classes: Vec<_> = expected.value().classes().collect();
    actual_classes.sort_unstable();
    expected_classes.sort_unstable();
    if actual.value().name() != expected.value().name() || actual_classes != expected_classes {
        return false;
    }
    if let Some(expected_href) = expected.value().attr("href") {
        let Some(actual_href) = actual.value().attr("href") else {
            return false;
        };
        if base.join(actual_href).ok() != base.join(expected_href).ok() {
            return false;
        }
    }
    let has_direct_text = |element: ElementRef<'_>| {
        element.children().any(|child| {
            child
                .value()
                .as_text()
                .is_some_and(|text| !text.trim().is_empty())
        })
    };
    if has_direct_text(expected) {
        if normalize_text(actual.text()).is_none() {
            return false;
        }
    } else if has_direct_text(actual) {
        return false;
    }
    let actual_children: Vec<_> = actual.children().filter_map(ElementRef::wrap).collect();
    let expected_children: Vec<_> = expected.children().filter_map(ElementRef::wrap).collect();
    actual_children.len() == expected_children.len()
        && actual_children
            .into_iter()
            .zip(expected_children)
            .all(|(actual, expected)| matches_empty_home_structure(actual, expected, base))
}

fn selector(value: &str) -> Selector {
    Selector::parse(value).expect("hard-coded selector should be valid")
}
