use mc_common::time::Timestamp;
use mc_search::{DocumentKind, SearchFilters};
use mc_server::retrieval::retrieve_filtered;
use mc_storage::projectors::summaries::NewSummary;
use mc_storage::Database;

#[test]
fn summary_time_filter_is_applied_before_reading_bodies() {
    let directory = tempfile::tempdir().unwrap();
    let db = Database::open(directory.path().join("test.db")).unwrap();
    for (id, start) in [("before", 99), ("inside", 100), ("after", 200)] {
        db.insert_summary(
            &NewSummary {
                id: id.into(),
                kind: "adhoc".into(),
                stage_id: None,
                template_id: "default".into(),
                start: Timestamp::from_millis(start),
                end: Timestamp::from_millis(300),
                title: "needle".into(),
                fields: Default::default(),
                body_markdown: "needle summary".into(),
                quality: "fallback".into(),
                model: None,
                prompt_tokens: 0,
                completion_tokens: 0,
                scope: None,
            },
            Timestamp::from_millis(300),
        )
        .unwrap();
    }
    db.with_write(|connection| {
        connection.execute(
            "UPDATE summaries SET body_markdown = x'FF' WHERE id != 'inside'",
            [],
        )
    })
    .unwrap();
    let filters = SearchFilters {
        from: Some(Timestamp::from_millis(100)),
        to: Some(Timestamp::from_millis(200)),
        kinds: vec![DocumentKind::Summary],
        ..Default::default()
    };
    let hits = retrieve_filtered(&db, "needle", 10, filters.clone(), None)
        .expect("out-of-range bodies must not be read or decoded");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].document.id, "inside");
    db.with_write(|connection| {
        connection.execute(
            "UPDATE summaries SET body_markdown = x'FF' WHERE id = 'inside'",
            [],
        )
    })
    .unwrap();
    assert!(retrieve_filtered(&db, "needle", 10, filters, None).is_err());
}
