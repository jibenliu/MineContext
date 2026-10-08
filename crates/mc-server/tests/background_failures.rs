use mc_common::error::{AppError, ErrorCode};
use mc_common::observability::{self, LogOptions};
use mc_common::time::Timestamp;
use mc_server::failures::{record_capture_failure, record_embedding_failure};
use mc_storage::Database;

#[test]
fn background_failures_persist_or_log_without_exposing_error_details() {
    let directory = tempfile::tempdir().unwrap();
    let log_path = directory.path().join("diagnostics.log");
    let guard = observability::init(LogOptions {
        file: Some(log_path.clone()),
        stderr: false,
        json: true,
        ..LogOptions::default()
    })
    .unwrap();
    let db = Database::open(directory.path().join("test.db")).unwrap();
    let at = Timestamp::from_millis(1_000);
    let original = AppError::new(
        ErrorCode::CaptureUnsupported,
        "private-screen-title sk-private-key",
    );
    record_capture_failure(&db, at, &original);
    record_embedding_failure(&db, at, &original);
    let records = db
        .with_read(|connection| {
            let mut statement = connection.prepare(
            "SELECT component, error_code, severity, at_utc_ms FROM pipeline_failures ORDER BY id"
        )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .unwrap();
    assert_eq!(
        records,
        vec![
            (
                "capture".into(),
                original.code().as_str().into(),
                "warn".into(),
                1_000
            ),
            (
                "embedding".into(),
                original.code().as_str().into(),
                "warn".into(),
                1_000
            ),
        ]
    );
    db.with_write(|connection| {
        connection.execute_batch(
            "CREATE TRIGGER reject_failure BEFORE INSERT ON pipeline_failures
         BEGIN SELECT RAISE(ABORT, 'private-database-path'); END;",
        )
    })
    .unwrap();
    record_capture_failure(&db, at, &original);
    record_embedding_failure(&db, at, &original);
    drop(guard);
    let log = std::fs::read_to_string(&log_path).unwrap();
    let warnings: Vec<serde_json::Value> = log
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .filter(|entry: &serde_json::Value| {
            entry["fields"]["event"] == "failure_record_write_failed"
        })
        .collect();
    assert_eq!(
        warnings.len(),
        2,
        "diagnostic write failures must remain visible: {log}"
    );
    for (warning, component) in warnings.iter().zip(["capture", "embedding"]) {
        assert_eq!(warning["level"], "WARN");
        assert_eq!(warning["fields"]["component"], component);
        assert_eq!(warning["fields"]["original_code"], original.code().as_str());
        assert!(warning["fields"]["code"]
            .as_str()
            .is_some_and(|code| !code.is_empty()));
    }
    for secret in [
        "private-screen-title",
        "sk-private-key",
        "private-database-path",
    ] {
        assert!(
            !log.contains(secret),
            "diagnostic fallback must not log private error details"
        );
    }
    db.with_write(|connection| connection.execute_batch("DROP TRIGGER reject_failure"))
        .unwrap();
    record_capture_failure(&db, at, &original);
    let count: i64 = db
        .with_read(|connection| {
            connection.query_row("SELECT COUNT(*) FROM pipeline_failures", [], |row| {
                row.get(0)
            })
        })
        .unwrap();
    assert_eq!(count, 3);
}
