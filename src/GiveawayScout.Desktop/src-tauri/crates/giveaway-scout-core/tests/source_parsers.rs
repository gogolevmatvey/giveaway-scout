use giveaway_scout_core::{
    GameStore, GiveawayKind, GiveawaySource, parse_games_for_farm, parse_givee_club,
};

#[test]
fn givee_store_uses_platform_marker_instead_of_title_or_steam_tasks() {
    let items = parse_givee_club(
        r#"
        <a class="event-card" href="/event/1">
          <span class="event-title">Steam in a game title</span>
          <i class="event-platform event-platform-epicgames"></i>
          <i class="event-requirement-steam"></i>
        </a>
        <a class="event-card" href="/event/2">
          <span class="event-title">A game</span>
          <i class="event-platform event-platform-steam"></i>
        </a>
        <a class="event-card" href="/event/3">
          <span class="event-title">Another game</span>
          <i class="event-platform event-platform-gog"></i>
          <i class="event-requirement-steam"></i>
        </a>
    "#,
    )
    .unwrap();
    assert_eq!(
        items.iter().map(|item| item.store).collect::<Vec<_>>(),
        [GameStore::EpicGames, GameStore::Steam, GameStore::Unknown]
    );
}

#[test]
fn games_for_farm_store_uses_platform_icons_instead_of_artwork_or_title() {
    let items = parse_games_for_farm(r#"
        <div class="razdachi__card">
          <a class="card-content__title" href="/game?n=1">A game</a>
          <div class="card-content__platforms"><img alt="steam" src="/assets/images/icons-2/steam.png"></div>
        </div>
        <div class="rozygryshi__card">
          <img alt="Steam" src="https://shared.fastly.steamstatic.com/header.jpg">
          <a class="card-content__title" href="/game?n=2">Epic giveaway</a>
          <div class="card-content__platforms"><img alt="vk"><img alt="Epic Games"></div>
        </div>
        <div class="razdachi__card">
          <a class="card-content__title" href="/game?n=3">Steam in a title</a>
          <img alt="steam">
        </div>
    "#).unwrap();
    assert_eq!(
        items.iter().map(|item| item.store).collect::<Vec<_>>(),
        [GameStore::Steam, GameStore::Unknown, GameStore::EpicGames]
    );
}

#[test]
fn givee_club_parser_extracts_giveaway_and_raffle() {
    let items = parse_givee_club(include_str!("fixtures/html/givee-club.html")).unwrap();

    assert_eq!(items.len(), 2);
    assert_eq!(items[0].source, GiveawaySource::GiveeClub);
    assert_eq!(items[0].source_id, "event/100");
    assert_eq!(items[0].tracking_key(), "GiveeClub:event/100");
    assert_eq!(items[0].title, "Test game");
    assert_eq!(items[0].kind, GiveawayKind::Giveaway);
    assert_eq!(items[0].url, "https://givee.club/event/100");
    assert_eq!(items[0].details.as_deref(), Some("Раздача награда сразу"));
    assert_eq!(items[1].kind, GiveawayKind::Raffle);
}

#[test]
fn games_for_farm_parser_skips_service_cards_and_uses_stable_link_key() {
    let items = parse_games_for_farm(include_str!("fixtures/html/games-for-farm.html")).unwrap();

    assert_eq!(items.len(), 2);
    assert_eq!(items[0].source, GiveawaySource::GamesForFarm);
    assert_eq!(items[0].kind, GiveawayKind::Giveaway);
    assert_eq!(items[0].title, "First game");
    assert_eq!(items[0].url, "https://freeanywhere.net/game?n=42");
    assert_eq!(items[0].details.as_deref(), Some("Steam"));
    assert_eq!(items[1].kind, GiveawayKind::Raffle);
    assert_eq!(items[1].details.as_deref(), Some("Two keys"));

    for item in &items {
        assert_eq!(item.source_id, "/game?n=42");
        assert_eq!(item.tracking_key(), "GamesForFarm:/game?n=42");
        assert_eq!(
            item.legacy_tracking_key_prefixes,
            [
                "GamesForFarm:Giveaway:/game?n=42:",
                "GamesForFarm:Raffle:/game?n=42:",
            ]
        );
    }
}

#[test]
fn games_for_farm_tracking_key_does_not_depend_on_kind_or_image() {
    let items = parse_games_for_farm(include_str!("fixtures/html/games-for-farm.html")).unwrap();

    assert_ne!(items[0].kind, items[1].kind);
    assert_eq!(items[0].tracking_key(), items[1].tracking_key());
}

#[test]
fn unrecognized_pages_are_errors_not_verified_empty_feeds() {
    for html in [
        "",
        "<html><body>Maintenance</body></html>",
        "<form>Verify you are human</form>",
        "<main><a href='/event/1'>A renamed layout</a></main>",
    ] {
        assert!(parse_givee_club(html).is_err(), "Givee: {html}");
        assert!(
            parse_games_for_farm(html).is_err(),
            "Games for Farm: {html}"
        );
    }
}

#[test]
fn incomplete_givee_card_rejects_the_whole_page_instead_of_silently_skipping_a_baseline_item() {
    for incomplete in [
        r#"<a class="event-card"><span class="event-title">Missing link</span></a>"#,
        r#"<a class="event-card" href=" "><span class="event-title">Empty link</span></a>"#,
        r#"<a class="event-card" href="/event/empty"><span class="event-title"> </span></a>"#,
        r#"<a class="event-card" href="/event/renamed"><span class="new-title">Changed title</span></a>"#,
    ] {
        let page = format!(
            "{}{incomplete}",
            include_str!("fixtures/html/givee-club.html")
        );
        assert!(parse_givee_club(&page).is_err(), "{incomplete}");
    }
}

#[test]
fn incomplete_games_for_farm_card_rejects_the_whole_page_including_other_valid_cards() {
    for incomplete in [
        r#"<div class="rozygryshi__card"><span class="card-content__title">Missing link</span></div>"#,
        r#"<div class="razdachi__card"><a class="card-content__title">Missing href</a></div>"#,
        r#"<div class="razdachi__card"><a class="card-content__title" href="">Empty link</a></div>"#,
        r#"<div class="razdachi__card"><a class="card-content__title" href="/game?n=123"> </a></div>"#,
    ] {
        let page = format!(
            "{}{incomplete}",
            include_str!("fixtures/html/games-for-farm.html")
        );
        assert!(parse_games_for_farm(&page).is_err(), "{incomplete}");
    }
}

#[test]
fn non_web_links_are_parse_errors_before_they_can_enter_tracking_or_history() {
    for url in [
        "javascript:alert(1)",
        "file:///tmp/game",
        "data:text/plain,game",
    ] {
        let givee = format!(
            r#"<a class="event-card" href="{url}"><span class="event-title">Game</span></a>"#
        );
        let games_for_farm = format!(
            r#"<div class="razdachi__card"><a class="card-content__title" href="{url}">Game</a></div>"#
        );
        assert!(parse_givee_club(&givee).is_err(), "{url}");
        assert!(parse_games_for_farm(&games_for_farm).is_err(), "{url}");
    }
}

#[test]
fn known_service_cards_are_ignored_even_with_absolute_links_or_no_display_title() {
    let service = r#"
      <div class="razdachi__card"><a class="card-content__title" href="/user#settings">Content 18+</a></div>
      <div class="rozygryshi__card"><a class="card-content__title" href="https://freeanywhere.net/user#settings"> </a></div>
    "#;
    assert!(parse_games_for_farm(service).unwrap().is_empty());
    let page = format!(
        "{}{service}",
        include_str!("fixtures/html/games-for-farm.html")
    );
    assert_eq!(parse_games_for_farm(&page).unwrap().len(), 2);
}
