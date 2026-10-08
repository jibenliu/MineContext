use mc_common::time::Timestamp;
use mc_storage::projectors::summaries::NewSummary;
use mc_storage::Database;

#[test]
fn summary_start_ranges_preserve_bounds_order_and_legacy_filters() {
    let directory = tempfile::tempdir().unwrap();
    let db = Database::open(directory.path().join("test.db")).unwrap();
    db.with_write(|connection| {
        connection.execute_batch(
            "INSERT INTO stages(id, start_utc_ms, state, day, derived_from_seq)
         VALUES ('stage-1', 100, 'closed', '2026-10-08', 0),
                ('stage-2', 200, 'closed', '2026-10-08', 0);",
        )
    })
    .unwrap();
    for (id, start, kind, stage) in [
        ("before", 99, "adhoc", None),
        ("b", 100, "stage", Some("stage-1")),
        ("a", 100, "stage", Some("stage-1")),
        ("middle", 150, "adhoc", None),
        ("after", 200, "stage", Some("stage-2")),
    ] {
        db.insert_summary(
            &NewSummary {
                id: id.into(),
                kind: kind.into(),
                stage_id: stage.map(str::to_string),
                template_id: "default".into(),
                start: Timestamp::from_millis(start),
                end: Timestamp::from_millis(300),
                title: id.into(),
                fields: Default::default(),
                body_markdown: "summary".into(),
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
    let ids = |from: Option<i64>, to: Option<i64>, stage: Option<&str>, kind: Option<&str>| {
        db.read_summaries_in_range(
            stage,
            kind,
            from.map(Timestamp::from_millis),
            to.map(Timestamp::from_millis),
        )
        .unwrap()
        .into_iter()
        .map(|summary| summary.id)
        .collect::<Vec<_>>()
    };
    assert_eq!(ids(Some(100), Some(200), None, None), ["a", "b", "middle"]);
    assert_eq!(ids(None, Some(100), None, None), ["before"]);
    assert_eq!(ids(Some(200), None, None, None), ["after"]);
    assert!(ids(Some(200), Some(100), None, None).is_empty());
    assert!(ids(Some(100), Some(100), None, None).is_empty());
    assert_eq!(ids(None, None, Some("stage-1"), Some("stage")), ["a", "b"]);
    assert_eq!(ids(Some(100), Some(200), None, Some("adhoc")), ["middle"]);
    assert_eq!(
        db.read_summaries(None, None).unwrap(),
        db.read_summaries_in_range(None, None, None, None).unwrap()
    );
}
