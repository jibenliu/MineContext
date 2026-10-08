use mc_common::error::AppError;
use mc_common::observability::warn;
use mc_common::time::Timestamp;
use mc_storage::Database;

pub fn record_capture_failure(db: &Database, at: Timestamp, error: &AppError) {
    record_failure(db, at, "capture", error);
}

pub fn record_embedding_failure(db: &Database, at: Timestamp, error: &AppError) {
    record_failure(db, at, "embedding", error);
}

fn record_failure(db: &Database, at: Timestamp, component: &'static str, error: &AppError) {
    if let Err(write_error) = db.record_failure(at, component, error, "warn") {
        warn!(
            event = "failure_record_write_failed",
            component,
            code = write_error.code().as_str(),
            original_code = error.code().as_str(),
            at_ms = at.as_millis(),
            "后台失败记录无法写入数据库"
        );
    }
}
