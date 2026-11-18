use giveaway_scout_core::Database;

#[test]
fn announced_versions_survive_restart_and_history_cleanup() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("test.db");
    {
        let database = Database::open(&path).unwrap();
        assert!(!database.update_was_announced("1.2.0").unwrap());
        database.mark_update_announced("1.2.0").unwrap();
        database.mark_update_announced("1.2.0").unwrap();
    }
    let mut database = Database::open(&path).unwrap();
    assert!(database.update_was_announced("1.2.0").unwrap());
    assert!(!database.update_was_announced("1.3.0").unwrap());
    database.mark_update_announced("1.3.0").unwrap();
    database.clear_giveaway_data().unwrap();
    assert!(database.update_was_announced("1.2.0").unwrap());
    assert!(database.update_was_announced("1.3.0").unwrap());
}

#[test]
fn banner_dismissal_is_persistent_version_specific_and_separate_from_toasts() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("test.db");
    let database = Database::open(&path).unwrap();
    database.mark_update_announced("1.2.0").unwrap();
    assert!(!database.update_banner_was_dismissed("1.2.0").unwrap());
    database.dismiss_update_banner("1.3.0").unwrap();
    database.dismiss_update_banner("1.3.0").unwrap();
    assert!(!database.update_was_announced("1.3.0").unwrap());
    drop(database);
    let mut database = Database::open(&path).unwrap();
    database.clear_giveaway_data().unwrap();
    assert!(database.update_banner_was_dismissed("1.3.0").unwrap());
    assert!(!database.update_banner_was_dismissed("1.4.0").unwrap());
    assert!(!database.update_banner_was_dismissed("1.2.0").unwrap());
    assert!(database.update_was_announced("1.2.0").unwrap());
}
