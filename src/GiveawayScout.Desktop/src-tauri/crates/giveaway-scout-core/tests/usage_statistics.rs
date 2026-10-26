use giveaway_scout_core::Database;
use rusqlite::Connection;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_792_886_400).unwrap()
}

#[test]
fn opening_a_profile_does_not_generate_id_until_the_production_reporter_runs() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("profile.db");
    let database = Database::open(&path).unwrap();
    let connection = Connection::open(path).unwrap();
    let ids: i64 = connection
        .query_row("SELECT COUNT(*) FROM usage_statistics_state", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(ids, 0);
    let request = database.usage_statistics_request(now()).unwrap().unwrap();
    assert_eq!(
        Uuid::parse_str(&request.installation_id)
            .unwrap()
            .get_version_num(),
        4
    );
}

#[test]
fn retries_are_hourly_success_is_daily_and_identity_survives_restart_and_clear() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("profile.db");
    let database = Database::open(&path).unwrap();
    let first = database.usage_statistics_request(now()).unwrap().unwrap();
    assert!(
        database
            .usage_statistics_request(now() + Duration::minutes(59))
            .unwrap()
            .is_none()
    );
    drop(database);
    let database = Database::open(&path).unwrap();
    let retry = database
        .usage_statistics_request(now() + Duration::hours(1))
        .unwrap()
        .unwrap();
    assert_eq!(retry.installation_id, first.installation_id);
    database.usage_statistics_succeeded(&retry).unwrap();
    assert!(
        database
            .usage_statistics_request(now() + Duration::hours(2))
            .unwrap()
            .is_none()
    );
    drop(database);
    let mut database = Database::open(&path).unwrap();
    database.clear_giveaway_data().unwrap();
    let tomorrow = database
        .usage_statistics_request(now() + Duration::days(1))
        .unwrap()
        .unwrap();
    assert_eq!(tomorrow.installation_id, first.installation_id);
    database.usage_statistics_succeeded(&tomorrow).unwrap();
    assert!(
        database
            .usage_statistics_request(now() + Duration::days(1) + Duration::hours(2))
            .unwrap()
            .is_none()
    );
}

#[test]
fn v7_migration_is_transactional_and_preserves_settings_and_history() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("profile.db");
    let database = Database::open(&path).unwrap();
    let mut expected = database.settings().unwrap();
    expected.check_interval_minutes = 45;
    expected.keep_running_in_tray = true;
    database.update_settings(&expected).unwrap();
    drop(database);
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch("DROP TABLE usage_statistics_state; DELETE FROM schema_migrations WHERE version=8;
        INSERT INTO tracked_giveaways VALUES ('GiveeClub:event/42','giveeClub','event/42','Saved game','2026-10-20T00:00:00Z');
        CREATE TRIGGER fail_v8 BEFORE INSERT ON schema_migrations WHEN NEW.version=8
        BEGIN SELECT RAISE(FAIL, 'test migration failure'); END;").unwrap();
    drop(connection);
    assert!(Database::open(&path).is_err());
    let connection = Connection::open(&path).unwrap();
    let tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name='usage_statistics_state'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tables, 0);
    connection.execute_batch("DROP TRIGGER fail_v8").unwrap();
    drop(connection);
    let database = Database::open(&path).unwrap();
    assert_eq!(database.settings().unwrap(), expected);
    assert_eq!(database.counts().unwrap().tracked_giveaways, 1);
    assert!(database.usage_statistics_request(now()).unwrap().is_some());
}
