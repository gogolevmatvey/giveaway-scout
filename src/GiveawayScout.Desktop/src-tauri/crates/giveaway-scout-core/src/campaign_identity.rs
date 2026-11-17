use url::Url;

use crate::GiveawaySource;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampaignIdentity {
    pub key: String,
    pub url: String,
    pub origin_source: Option<GiveawaySource>,
    pub origin_name: Option<String>,
}

/// Only campaign-specific addresses are evidence of equality. Product pages,
/// names and the shared Zeros homepage deliberately cannot identify a campaign.
pub fn identify_campaign(
    source: GiveawaySource,
    source_id: &str,
    url: &str,
) -> Option<CampaignIdentity> {
    if source == GiveawaySource::Zeros {
        return None;
    }
    known_destination(url).or_else(|| {
        if source != GiveawaySource::GiveeClub {
            return None;
        }
        let path = source_id.trim_matches('/');
        known_destination(&format!("https://givee.club/{path}"))
    })
}

pub fn known_destination(value: &str) -> Option<CampaignIdentity> {
    let url = Url::parse(value.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return None;
    }
    let host = url.host_str()?;
    let host = host.strip_prefix("www.").unwrap_or(host);
    let parts: Vec<_> = url
        .path_segments()?
        .filter(|part| !part.is_empty())
        .collect();
    let (key, origin_source, name) = match host {
        "givee.club" => {
            let event = match parts.as_slice() {
                ["event", id] => *id,
                [language, "event", id]
                    if language.len() == 2
                        && language.bytes().all(|byte| byte.is_ascii_alphabetic()) =>
                {
                    *id
                }
                _ => return None,
            };
            let id = numeric_identifier(event)?;
            (
                format!("givee:event:{id}"),
                Some(GiveawaySource::GiveeClub),
                "Givee Club",
            )
        }
        "gleam.io" => {
            let code = match parts.as_slice() {
                [code] | [code, _] if valid_gleam_code(code) => *code,
                ["rewards" | "competitions", slug] => {
                    let code = slug.split('-').next()?;
                    if !valid_gleam_code(code) {
                        return None;
                    }
                    code
                }
                _ => return None,
            };
            (format!("gleam:{code}"), None, "Gleam")
        }
        "freeanywhere.net" | "gamesforfarm.com" => {
            if !matches!(parts.as_slice(), ["game"] | ["raffle"]) {
                return None;
            }
            let ids: Vec<_> = url.query_pairs().filter(|(key, _)| key == "n").collect();
            if ids.len() != 1 {
                return None;
            }
            let id = numeric_identifier(&ids[0].1)?;
            (
                format!("gamesForFarm:{}:{id}", parts[0]),
                Some(GiveawaySource::GamesForFarm),
                "Games for Farm",
            )
        }
        _ => return None,
    };
    let mut destination = url;
    destination.set_fragment(None);
    Some(CampaignIdentity {
        key,
        url: destination.to_string(),
        origin_source,
        origin_name: Some(name.to_owned()),
    })
}

fn numeric_identifier(value: &str) -> Option<u64> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let id = value.parse().ok()?;
    (id > 0).then_some(id)
}

fn valid_gleam_code(value: &str) -> bool {
    value.len() == 5
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
        && !matches!(
            value.to_ascii_lowercase().as_str(),
            "login" | "about" | "terms" | "admin" | "users" | "sites"
        )
}

pub fn is_free_steam_keys_redirect(value: &str) -> bool {
    Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && matches!(
                url.host_str(),
                Some("www.freesteamkeys.com" | "freesteamkeys.com")
            )
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && url.path().starts_with("/open/")
            && url.path().len() > "/open/".len()
    })
}
