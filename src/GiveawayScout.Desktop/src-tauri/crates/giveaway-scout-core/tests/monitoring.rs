use std::collections::HashMap;

use giveaway_scout_core::{
    Database, GAMES_FOR_FARM_FEED_URL, GIVEE_CLUB_FEED_URL, GiveawaySource, HttpPageFetcher,
    PageFetchError, PageFetcher, apply_source_check, check_sources,
};

struct FixtureFetcher {
    responses: HashMap<&'static str, Result<&'static str, &'static str>>,
}

impl PageFetcher for FixtureFetcher {
    async fn get_text(&self, url: &'static str) -> Result<String, PageFetchError> {
        match self.responses.get(url).expect("fixture URL should exist") {
            Ok(html) => Ok((*html).to_owned()),
            Err(message) => Err(PageFetchError::Message((*message).to_owned())),
        }
    }
}

#[tokio::test]
async fn checks_enabled_sources_and_combines_items() {
    let fetcher = FixtureFetcher {
        responses: HashMap::from([
            (
                GIVEE_CLUB_FEED_URL,
                Ok(include_str!("fixtures/html/givee-club.html")),
            ),
            (
                GAMES_FOR_FARM_FEED_URL,
                Ok(include_str!("fixtures/html/games-for-farm.html")),
            ),
        ]),
    };

    let result = check_sources(&fetcher, true, true, false).await;

    assert_eq!(result.items.len(), 4);
    assert_eq!(result.checked_sources.len(), 2);
    assert!(result.failures.is_empty());
}

#[tokio::test]
async fn failure_of_one_source_keeps_results_from_the_other() {
    let fetcher = FixtureFetcher {
        responses: HashMap::from([
            (GIVEE_CLUB_FEED_URL, Err("temporary network failure")),
            (
                GAMES_FOR_FARM_FEED_URL,
                Ok(include_str!("fixtures/html/games-for-farm.html")),
            ),
        ]),
    };

    let result = check_sources(&fetcher, true, true, false).await;

    assert_eq!(result.items.len(), 2);
    assert_eq!(result.failures.len(), 1);
    assert_eq!(result.failures[0].source, GiveawaySource::GiveeClub);
    assert_eq!(result.failures[0].message, "temporary network failure");
}

#[tokio::test]
async fn disabled_source_is_not_requested() {
    let fetcher = FixtureFetcher {
        responses: HashMap::from([(
            GIVEE_CLUB_FEED_URL,
            Ok(include_str!("fixtures/html/givee-club.html")),
        )]),
    };

    let result = check_sources(&fetcher, true, false, false).await;

    assert_eq!(result.items.len(), 2);
    assert_eq!(result.checked_sources, [GiveawaySource::GiveeClub]);
    assert!(result.failures.is_empty());
}

#[tokio::test]
async fn failed_initial_check_defers_baseline_and_prevents_false_notifications() {
    let failed_fetcher = FixtureFetcher {
        responses: HashMap::from([
            (GIVEE_CLUB_FEED_URL, Err("temporary network failure")),
            (
                GAMES_FOR_FARM_FEED_URL,
                Ok(include_str!("fixtures/html/games-for-farm.html")),
            ),
        ]),
    };
    let successful_fetcher = FixtureFetcher {
        responses: HashMap::from([
            (
                GIVEE_CLUB_FEED_URL,
                Ok(include_str!("fixtures/html/givee-club.html")),
            ),
            (
                GAMES_FOR_FARM_FEED_URL,
                Ok(include_str!("fixtures/html/games-for-farm.html")),
            ),
        ]),
    };
    let mut database = Database::open_in_memory().unwrap();

    let failed = check_sources(&failed_fetcher, true, true, false).await;
    let deferred = apply_source_check(&mut database, failed).unwrap();
    assert!(deferred.baseline_deferred);
    assert!(!database.tracking_initialized().unwrap());
    assert_eq!(database.counts().unwrap().tracked_giveaways, 0);

    let successful = check_sources(&successful_fetcher, true, true, false).await;
    let baseline = apply_source_check(&mut database, successful).unwrap();
    assert!(baseline.is_initial_synchronization);
    assert!(!baseline.baseline_deferred);
    assert!(baseline.new_items.is_empty());
    assert_eq!(database.counts().unwrap().tracked_giveaways, 3);
}

#[test]
fn no_enabled_sources_does_not_initialize_tracking() {
    let mut database = Database::open_in_memory().unwrap();

    let outcome = apply_source_check(&mut database, Default::default()).unwrap();

    assert!(!outcome.check_performed);
    assert!(!database.tracking_initialized().unwrap());
}

#[tokio::test]
async fn unrecognized_html_cannot_complete_a_baseline_and_replay_existing_giveaways() {
    for failed_source in [GIVEE_CLUB_FEED_URL, GAMES_FOR_FARM_FEED_URL] {
        let valid = HashMap::from([
            (
                GIVEE_CLUB_FEED_URL,
                Ok(include_str!("fixtures/html/givee-club.html")),
            ),
            (
                GAMES_FOR_FARM_FEED_URL,
                Ok(include_str!("fixtures/html/games-for-farm.html")),
            ),
        ]);
        let mut responses = valid.clone();
        responses.insert(
            failed_source,
            Ok("<html><body>Temporarily unavailable</body></html>"),
        );
        let mut database = Database::open_in_memory().unwrap();
        let check = check_sources(&FixtureFetcher { responses }, true, true, false).await;
        assert_eq!(check.failures.len(), 1, "{failed_source}");
        let deferred = apply_source_check(&mut database, check).unwrap();
        assert!(deferred.baseline_deferred);
        assert!(!database.tracking_initialized().unwrap());
        assert_eq!(database.counts().unwrap().notification_history, 0);
        let restored = check_sources(&FixtureFetcher { responses: valid }, true, true, false).await;
        let baseline = apply_source_check(&mut database, restored).unwrap();
        assert!(baseline.is_initial_synchronization);
        assert!(baseline.new_items.is_empty());
        assert_eq!(database.counts().unwrap().tracked_giveaways, 3);
        assert_eq!(database.counts().unwrap().notification_history, 0);
    }
}

#[tokio::test]
async fn parse_failure_preserves_known_items_and_does_not_block_the_other_initialized_source() {
    let mut database = Database::open_in_memory().unwrap();
    let valid = FixtureFetcher {
        responses: HashMap::from([
            (
                GIVEE_CLUB_FEED_URL,
                Ok(include_str!("fixtures/html/givee-club.html")),
            ),
            (
                GAMES_FOR_FARM_FEED_URL,
                Ok(include_str!("fixtures/html/games-for-farm.html")),
            ),
        ]),
    };
    let baseline = check_sources(&valid, true, true, false).await;
    apply_source_check(&mut database, baseline).unwrap();
    let changed = FixtureFetcher {
        responses: HashMap::from([
            (GIVEE_CLUB_FEED_URL, Ok("<html>Maintenance</html>")),
            (
                GAMES_FOR_FARM_FEED_URL,
                Ok(
                    r#"<div class="razdachi__card"><a class="card-content__title" href="/game?n=43">A truly new game</a></div>"#,
                ),
            ),
        ]),
    };
    let result = check_sources(&changed, true, true, false).await;
    let outcome = apply_source_check(&mut database, result).unwrap();
    assert_eq!(outcome.failures.len(), 1);
    assert_eq!(outcome.failures[0].source, GiveawaySource::GiveeClub);
    assert_eq!(outcome.new_items.len(), 1);
    assert_eq!(outcome.new_items[0].source, GiveawaySource::GamesForFarm);
    assert_eq!(database.counts().unwrap().tracked_giveaways, 4);
    assert_eq!(database.counts().unwrap().notification_history, 1);
    let restored = check_sources(&valid, true, true, false).await;
    let outcome = apply_source_check(&mut database, restored).unwrap();
    assert!(outcome.failures.is_empty());
    assert!(outcome.new_items.is_empty());
    assert!(!outcome.is_initial_synchronization);
    assert_eq!(database.counts().unwrap().tracked_giveaways, 4);
    assert_eq!(database.counts().unwrap().notification_history, 1);
}

#[tokio::test]
#[ignore = "manual network check of public feeds; no profile, database or notifications"]
async fn live_source_markup_is_supported_by_production_parsers() {
    let fetcher = HttpPageFetcher::with_policy(30, 0).unwrap();
    let result = check_sources(&fetcher, true, true, false).await;
    assert!(result.failures.is_empty(), "{:?}", result.failures);
    for source in [GiveawaySource::GiveeClub, GiveawaySource::GamesForFarm] {
        let count = result
            .items
            .iter()
            .filter(|item| item.source == source)
            .count();
        assert!(
            count > 0,
            "live feed {source:?} has no public giveaway cards"
        );
        println!("{source:?}: {count} readable public cards");
    }
}

#[tokio::test]
async fn newly_enabled_source_synchronizes_current_items_without_alerting_old_publications() {
    for givee_first in [true, false] {
        let fetcher = FixtureFetcher {
            responses: HashMap::from([
                (
                    GIVEE_CLUB_FEED_URL,
                    Ok(include_str!("fixtures/html/givee-club.html")),
                ),
                (
                    GAMES_FOR_FARM_FEED_URL,
                    Ok(include_str!("fixtures/html/games-for-farm.html")),
                ),
            ]),
        };
        let mut database = Database::open_in_memory().unwrap();
        let mut settings = database.settings().unwrap();
        settings.givee_club_enabled = givee_first;
        settings.games_for_farm_enabled = !givee_first;
        database.update_settings(&settings).unwrap();
        let initial = check_sources(&fetcher, givee_first, !givee_first, false).await;
        apply_source_check(&mut database, initial).unwrap();
        assert_eq!(database.counts().unwrap().notification_history, 0);
        settings.givee_club_enabled = true;
        settings.games_for_farm_enabled = true;
        database.update_settings(&settings).unwrap();
        let added = check_sources(&fetcher, true, true, false).await;
        let outcome = apply_source_check(&mut database, added).unwrap();
        assert!(outcome.new_items.is_empty(), "givee_first={givee_first}");
        assert_eq!(database.counts().unwrap().tracked_giveaways, 3);
        assert_eq!(database.counts().unwrap().notification_history, 0);
    }
}
