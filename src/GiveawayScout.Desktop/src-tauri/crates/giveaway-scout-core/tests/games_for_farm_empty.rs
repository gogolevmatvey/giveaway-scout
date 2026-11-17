use giveaway_scout_core::{
    Database, GAMES_FOR_FARM_FEED_URL, GiveawaySource, InitialSyncMode, PageFetchError,
    PageFetcher, SourceFailureReason, apply_source_check, check_sources, parse_games_for_farm,
};

const EMPTY_EN: &str = include_str!("fixtures/html/games-for-farm-empty-en.html");
const EMPTY_RU: &str = include_str!("fixtures/html/games-for-farm-empty-ru.html");
const ONE_GIVEAWAY: &str = r#"<div class="razdachi__card"><a class="card-content__title" href="/game?n=91">A new game</a></div>"#;

#[test]
fn complete_known_home_without_active_sections_is_verified_empty_in_both_languages() {
    for page in [EMPTY_EN, EMPTY_RU] {
        assert!(parse_games_for_farm(page).unwrap().is_empty());
    }
}

#[test]
fn empty_home_requires_its_complete_site_and_navigation_structure() {
    let incomplete = [
        EMPTY_EN.replace("page-index", "page-archive"),
        EMPTY_EN.replace("games_for_farm_site", "another_site"),
        EMPTY_EN.replace("https://give.gamesforfarm.com", "https://example.com"),
        EMPTY_EN.replace("https://give.gamesforfarm.com", "http://give.gamesforfarm.com"),
        EMPTY_EN.replace("https://give.gamesforfarm.com", "https://give.gamesforfarm.com/giveaways"),
        EMPTY_EN.replace("<header class=\"header games_for_farm_site\"><a href=\"/\">Games for Farm</a></header>", ""),
        EMPTY_EN.replace("<footer><a href=\"/\">Games for Farm</a></footer>", ""),
        EMPTY_EN.replace("<div class=\"page_modals\"></div>", ""),
        EMPTY_EN.replace("class=\"main_banner\"", "class=\"maintenance\""),
        EMPTY_EN.replace("class=\"history_buttons\"", "class=\"unknown_section\""),
        EMPTY_EN.replace("href=\"/giveaways\"", "href=\"/login\""),
        EMPTY_EN.replace("href=\"/drawings\"", "href=\"https://example.com/drawings\""),
        EMPTY_EN.replace("</head>", ""),
        EMPTY_EN.replace("</body>", ""),
        EMPTY_EN.replace("</html>", ""),
        // A superficially complete shell cannot substitute a missing final section.
        format!("{}</body></html>", EMPTY_EN.split("<footer>").next().unwrap()),
    ];
    for page in incomplete {
        assert!(parse_games_for_farm(&page).is_err(), "{page}");
    }
}

#[test]
fn familiar_home_shell_does_not_hide_unknown_active_content_or_a_challenge() {
    for unexpected in [
        r#"<section class="new-active-list"><a href="/game?n=92">A game</a></section>"#,
        r#"<article class="renamed-card"><a href="/game?n=92">A game</a></article>"#,
        r#"<form action="/verify"><input name="captcha"></form>"#,
        r#"<p class="maintenance">Temporarily unavailable</p>"#,
    ] {
        // Reject both an unfamiliar main section and content placed inside the
        // known banner. Checking only banner/archive presence would miss these.
        for page in [
            EMPTY_EN.replace(
                "<section class=\"history_buttons\">",
                &format!("{unexpected}<section class=\"history_buttons\">"),
            ),
            EMPTY_EN.replace(
                "<div class=\"main_banner__decs\">",
                &format!("{unexpected}<div class=\"main_banner__decs\">"),
            ),
        ] {
            assert!(parse_games_for_farm(&page).is_err(), "{page}");
        }
    }
    let disguised_link =
        EMPTY_EN.replace("href=\"https://gamesforfarm.com\"", "href=\"/game?n=92\"");
    assert!(parse_games_for_farm(&disguised_link).is_err());
}

#[test]
fn known_empty_main_content_does_not_hide_body_sibling_errors_or_new_content() {
    for unexpected in [
        r#"<section class="maintenance"><h1>Unavailable</h1><form action="/verify"><input name="captcha"></form></section>"#,
        r#"<form action="/verify"><input name="captcha"></form>"#,
        r#"<article class="new-active-list"><a href="/game?n=92">A game</a></article>"#,
        "Temporarily unavailable",
        r#"<div id="sbbfrcc"><p>Unexpected visible content</p></div>"#,
    ] {
        for page in [
            EMPTY_EN.replace(
                "<div class=\"page_content\">",
                &format!("{unexpected}<div class=\"page_content\">"),
            ),
            EMPTY_EN.replace("<footer>", &format!("{unexpected}<footer>")),
        ] {
            assert!(parse_games_for_farm(&page).is_err(), "{page}");
        }
    }
}

#[test]
fn active_cards_still_use_the_existing_strict_parser_and_stable_old_domain() {
    let page = EMPTY_EN.replace(
        "<section class=\"history_buttons\">",
        &format!("{ONE_GIVEAWAY}<section class=\"history_buttons\">"),
    );
    let items = parse_games_for_farm(&page).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].source_id, "/game?n=91");
    assert_eq!(items[0].url, "https://freeanywhere.net/game?n=91");
    for incomplete in [
        r#"<div class="razdachi__card"><span class="card-content__title">Missing link</span></div>"#,
        r#"<div class="rozygryshi__card"><a class="card-content__title" href="/game?n=92"> </a></div>"#,
    ] {
        let page = EMPTY_EN.replace(
            "<section class=\"history_buttons\">",
            &format!("{incomplete}<section class=\"history_buttons\">"),
        );
        assert!(parse_games_for_farm(&page).is_err(), "{page}");
    }
}

struct HomeFetcher<'a>(&'a str);

impl PageFetcher for HomeFetcher<'_> {
    async fn get_text(&self, url: &'static str) -> Result<String, PageFetchError> {
        assert_eq!(url, GAMES_FOR_FARM_FEED_URL);
        Ok(self.0.to_owned())
    }
}

#[tokio::test]
async fn verified_empty_home_recovers_a_parse_failure_and_initializes_only_the_checked_source() {
    for mode in [InitialSyncMode::Silent, InitialSyncMode::NotifyCurrent] {
        let mut database = Database::open_in_memory().unwrap();
        let mut settings = database.settings().unwrap();
        settings.initial_sync_mode = Some(mode);
        database.update_settings(&settings).unwrap();
        let unrecognized = EMPTY_EN.replace("main_banner__wrapper", "unknown_active_wrapper");
        let failed = check_sources(&HomeFetcher(&unrecognized), false, true, false, false).await;
        assert_eq!(failed.failures.len(), 1);
        assert_eq!(failed.failures[0].reason, SourceFailureReason::InvalidFeed);
        assert!(
            apply_source_check(&mut database, failed)
                .unwrap()
                .baseline_deferred
        );
        assert!(
            !database
                .source_tracking_initialized(GiveawaySource::GamesForFarm)
                .unwrap()
        );

        let empty = check_sources(&HomeFetcher(EMPTY_EN), false, true, false, false).await;
        assert_eq!(empty.checked_sources, [GiveawaySource::GamesForFarm]);
        assert!(empty.items.is_empty());
        assert!(empty.failures.is_empty());
        let recovered = apply_source_check(&mut database, empty).unwrap();
        assert!(recovered.is_initial_synchronization);
        assert!(!recovered.baseline_deferred);
        assert!(recovered.failures.is_empty());
        assert!(recovered.new_items.is_empty());
        assert!(
            database
                .source_tracking_initialized(GiveawaySource::GamesForFarm)
                .unwrap()
        );
        assert!(
            !database
                .source_tracking_initialized(GiveawaySource::GiveeClub)
                .unwrap()
        );
        assert_eq!(database.counts().unwrap().tracked_giveaways, 0);
        assert_eq!(database.counts().unwrap().notification_history, 0);

        // A publication appearing after a genuine empty baseline is new, even
        // in Silent mode; repeated checks must not replay it.
        let active = check_sources(&HomeFetcher(ONE_GIVEAWAY), false, true, false, false).await;
        assert_eq!(
            apply_source_check(&mut database, active)
                .unwrap()
                .new_items
                .len(),
            1
        );
        let repeated = check_sources(&HomeFetcher(ONE_GIVEAWAY), false, true, false, false).await;
        assert!(
            apply_source_check(&mut database, repeated)
                .unwrap()
                .new_items
                .is_empty()
        );
        assert_eq!(database.counts().unwrap().notification_history, 1);
    }
}

#[tokio::test]
async fn empty_home_preserves_existing_history_read_state_and_deduplication_after_a_failure() {
    let mut database = Database::open_in_memory().unwrap();
    let mut settings = database.settings().unwrap();
    settings.initial_sync_mode = Some(InitialSyncMode::NotifyCurrent);
    database.update_settings(&settings).unwrap();
    let active = check_sources(&HomeFetcher(ONE_GIVEAWAY), false, true, false, false).await;
    assert_eq!(
        apply_source_check(&mut database, active)
            .unwrap()
            .new_items
            .len(),
        1
    );
    let notification_id = database.notifications().unwrap()[0].id.clone();
    database.mark_notification_read(&notification_id).unwrap();
    let history = database.notifications().unwrap();
    let counts = database.counts().unwrap();

    let failed = check_sources(
        &HomeFetcher("<html><body>Unrecognized response</body></html>"),
        false,
        true,
        false,
        false,
    )
    .await;
    assert_eq!(
        apply_source_check(&mut database, failed)
            .unwrap()
            .failures
            .len(),
        1
    );
    let empty = check_sources(&HomeFetcher(EMPTY_RU), false, true, false, false).await;
    let outcome = apply_source_check(&mut database, empty).unwrap();
    assert!(outcome.failures.is_empty());
    assert!(outcome.new_items.is_empty());
    assert!(!outcome.is_initial_synchronization);
    assert_eq!(database.notifications().unwrap(), history);
    assert_eq!(database.counts().unwrap(), counts);

    let active = check_sources(&HomeFetcher(ONE_GIVEAWAY), false, true, false, false).await;
    assert!(
        apply_source_check(&mut database, active)
            .unwrap()
            .new_items
            .is_empty()
    );
    assert_eq!(database.notifications().unwrap(), history);
    assert_eq!(database.counts().unwrap(), counts);
}

#[tokio::test]
async fn a_later_empty_home_does_not_delete_tracked_giveaways_or_notification_history() {
    let mut database = Database::open_in_memory().unwrap();
    let mut settings = database.settings().unwrap();
    settings.initial_sync_mode = Some(InitialSyncMode::NotifyCurrent);
    database.update_settings(&settings).unwrap();
    let active = check_sources(&HomeFetcher(ONE_GIVEAWAY), false, true, false, false).await;
    apply_source_check(&mut database, active).unwrap();
    let history = database.notifications().unwrap();
    assert_eq!(history.len(), 1);
    let counts = database.counts().unwrap();
    for page in [EMPTY_RU, EMPTY_EN] {
        let empty = check_sources(&HomeFetcher(page), false, true, false, false).await;
        let outcome = apply_source_check(&mut database, empty).unwrap();
        assert!(outcome.failures.is_empty());
        assert!(outcome.new_items.is_empty());
        assert_eq!(database.counts().unwrap(), counts);
        assert_eq!(database.notifications().unwrap(), history);
    }
    let repeated = check_sources(&HomeFetcher(ONE_GIVEAWAY), false, true, false, false).await;
    assert!(
        apply_source_check(&mut database, repeated)
            .unwrap()
            .new_items
            .is_empty()
    );
}
