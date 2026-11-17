use giveaway_scout_core::{
    GiveawaySource, identify_campaign, is_free_steam_keys_redirect, known_destination,
};

#[test]
fn givee_languages_and_formatting_share_the_event_number() {
    let addresses = [
        "https://givee.club/event/42",
        "https://givee.club/ru/event/42",
        "https://www.givee.club/en/event/00042/?from=aggregator#claim",
    ];
    for address in addresses {
        let identity = known_destination(address).unwrap();
        assert_eq!(identity.key, "givee:event:42");
        assert_eq!(identity.origin_source, Some(GiveawaySource::GiveeClub));
        assert_eq!(identity.origin_name.as_deref(), Some("Givee Club"));
        assert!(!identity.url.contains('#'));
    }
    let recovered = identify_campaign(
        GiveawaySource::GiveeClub,
        "ru/event/42",
        "https://givee.club/ru",
    )
    .unwrap();
    assert_eq!(recovered.key, "givee:event:42");
    assert!(identify_campaign(GiveawaySource::FreeSteamKeys, "42", addresses[0]).is_some());
}

#[test]
fn gleam_variants_share_case_sensitive_campaign_codes() {
    for address in [
        "https://gleam.io/4cb3E",
        "https://gleam.io/4cb3E/free-steam-keys-of-one-gun-guy",
        "https://gleam.io/rewards/4cb3E-free-steam-keys-of-one-gun-guy",
        "https://gleam.io/competitions/4cb3E-free-steam-keys-of-one-gun-guy",
    ] {
        let identity = known_destination(address).unwrap();
        assert_eq!(identity.key, "gleam:4cb3E", "{address}");
        assert_eq!(identity.origin_source, None);
        assert_eq!(identity.origin_name.as_deref(), Some("Gleam"));
    }
    let differently_cased = known_destination("https://gleam.io/4cb3e/game").unwrap();
    assert_eq!(differently_cased.key, "gleam:4cb3e");
    assert_ne!(differently_cased.key, "gleam:4cb3E");
    for non_campaign in [
        "https://gleam.io/login",
        "https://gleam.io/about/product",
        "https://gleam.io/terms/privacy",
        "https://gleam.io/giveaways",
        "https://gleam.io/abc_1/game",
        "https://gleam.io/ABC123/game",
        "https://gleam.io/AbC12/game/extra",
    ] {
        assert!(known_destination(non_campaign).is_none(), "{non_campaign}");
    }
}

#[test]
fn farm_aliases_require_one_concrete_giveaway_or_raffle_identifier() {
    let first = known_destination("https://freeanywhere.net/game?n=00091&from=scout").unwrap();
    let alias = known_destination("https://gamesforfarm.com/game?n=91").unwrap();
    assert_eq!(first.key, alias.key);
    assert_eq!(first.origin_source, Some(GiveawaySource::GamesForFarm));
    assert_eq!(first.origin_name.as_deref(), Some("Games for Farm"));
    assert_ne!(
        first.key,
        known_destination("https://gamesforfarm.com/raffle?n=91")
            .unwrap()
            .key
    );
    for non_campaign in [
        "https://gamesforfarm.com/",
        "https://gamesforfarm.com/game",
        "https://gamesforfarm.com/game?n=0",
        "https://gamesforfarm.com/game?n=%2B91",
        "https://gamesforfarm.com/game?n=91&n=92",
        "https://gamesforfarm.com/game?n=91&n=91",
        "https://gamesforfarm.com/game?n=word",
        "https://gamesforfarm.com/page?n=91",
    ] {
        assert!(known_destination(non_campaign).is_none(), "{non_campaign}");
    }
}

#[test]
fn similar_hosts_credentials_invalid_numbers_and_product_pages_are_not_identity_evidence() {
    for non_campaign in [
        "https://www.www.givee.club/event/42",
        "https://givee.club.evil.example/event/42",
        "https://givee.club@evil.example/event/42",
        "https://user:pass@givee.club/event/42",
        "https://givee.club:8443/event/42",
        "https://givee.club/event/0",
        "https://givee.club/event/+42",
        "https://givee.club/event/18446744073709551616",
        "file:///event/42",
        "https://store.steampowered.com/app/440/",
        "http://zeros.group/free/",
    ] {
        assert!(known_destination(non_campaign).is_none(), "{non_campaign}");
    }
    for period in ["2026_1106", "2026_1107"] {
        assert!(
            identify_campaign(GiveawaySource::Zeros, period, "http://zeros.group/free/").is_none()
        );
    }
}

#[test]
fn only_exact_https_free_steam_keys_open_addresses_are_fetched() {
    for accepted in [
        "https://www.freesteamkeys.com/open/game",
        "https://freesteamkeys.com/open/game/?from=scout#claim",
    ] {
        assert!(is_free_steam_keys_redirect(accepted), "{accepted}");
    }
    for rejected in [
        "http://www.freesteamkeys.com/open/game",
        "https://www.freesteamkeys.com/api/giveaways",
        "https://www.freesteamkeys.com/open/",
        "https://www.freesteamkeys.com.evil.example/open/game",
        "https://www.www.freesteamkeys.com/open/game",
        "https://user@freesteamkeys.com/open/game",
        "https://freesteamkeys.com:8443/open/game",
        "https://gleam.io/AbC12/game",
    ] {
        assert!(!is_free_steam_keys_redirect(rejected), "{rejected}");
    }
}
