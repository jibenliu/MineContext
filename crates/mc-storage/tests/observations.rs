//! 、1.31：观测持久化、幂等与时间线读取。
//!
//! 这是采集层真正落到数据库的地方。两条硬要求：
//! - **观测与事件同事务**：不允许出现「有事件没观测」或反过来
//! - **幂等**：同一份内容重复投递只留一行
//!
//! 另外验证 的原则：**数据库里不存图片二进制**，
//! 只存路径与哈希。

use mc_common::time::Timestamp;
use mc_storage::observations::{ImageRef, NewObservation, ObservationQuery};
use mc_storage::Database;
use mc_testkit::fixtures::FIXTURE_EPOCH_MS;

fn open() -> (tempfile::TempDir, Database) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("minecontext.db");
    let db = Database::open(&path).unwrap();
    (dir, db)
}

fn image_ref(hash: &str, bytes: u64) -> ImageRef {
    ImageRef {
        relative_path: format!("screenshots/2026/09/30/{hash}.png"),
        content_hash: hash.to_string(),
        thumbnail_path: Some(format!("thumbnails/2026/09/30/{hash}-thumb.png")),
        width: 640,
        height: 400,
        bytes,
    }
}

fn observation(id: &str, at_ms: i64, idempotency: &str) -> NewObservation {
    NewObservation {
        id: id.to_string(),
        ts: Timestamp::from_millis(at_ms),
        source_id: "screen:display-1".to_string(),
        kind: "screen".to_string(),
        app_name: Some("Visual Studio Code".to_string()),
        app_bundle_id: Some("com.microsoft.VSCode".to_string()),
        window_title: Some("main.rs — MineContext".to_string()),
        domain: None,
        display_id: Some("display-1".to_string()),
        scale_factor: Some(2.0),
        image: Some(image_ref("abc123", 12_345)),
        text_content: None,
        text_origin: None,
        change_kind: "pixel_major".to_string(),
        privacy_verdict: "allowed".to_string(),
        phash: Some(0xDEAD_BEEF_1234_5678),
        idempotency: idempotency.to_string(),
    }
}

// ---------------------------------------------------------------- 1.24 事务性

#[test]
fn observation_and_event_are_written_together() {
    let (_dir, db) = open();

    let outcome = db
        .insert_observation(&observation("obs-1", FIXTURE_EPOCH_MS, "idem-1"))
        .unwrap();

    assert!(outcome.inserted);
    assert_eq!(outcome.id, "obs-1");

    // 事件与观测必须同时存在（同一事务）
    assert_eq!(db.event_count().unwrap(), 1);
    let events = db.read_events(0, 10).unwrap();
    assert_eq!(events[0].kind, "observation_captured");
    assert_eq!(events[0].actor, "capture");
    assert_eq!(events[0].payload["observation_id"], "obs-1");
    assert_eq!(events[0].payload["change_kind"], "pixel_major");
}

#[test]
fn failed_observation_write_leaves_no_event() {
    let (_dir, db) = open();

    // 缺 id 的观测在数据库层面是合法的（TEXT PRIMARY KEY 允许空串以外任意值），
    // 但重复主键会失败。先写一条，再用**相同 id、不同幂等键**写第二条：
    // 主键冲突 → 整笔回滚 → 不应留下孤立事件。
    db.insert_observation(&observation("obs-1", FIXTURE_EPOCH_MS, "idem-1"))
        .unwrap();

    let conflicting = observation("obs-1", 1_790_758_860_000, "idem-2");
    let result = db.insert_observation(&conflicting);

    assert!(result.is_err(), "主键冲突必须报错而不是静默覆盖");
    assert_eq!(db.event_count().unwrap(), 1, "失败的那笔不应留下事件");
}

// ---------------------------------------------------------------- 1.31 幂等

#[test]
fn idempotency_prevents_duplicate_observation_rows() {
    let (_dir, db) = open();

    let obs = observation("obs-1", FIXTURE_EPOCH_MS, "same-idempotency");
    let first = db.insert_observation(&obs).unwrap();
    assert!(first.inserted);

    // 同一份内容（同一幂等键）再次投递 —— 例如队重重试、崩溃后恢复重放
    let second = db.insert_observation(&obs).unwrap();
    assert!(!second.inserted, "重复投递不应再插入一行");
    assert_eq!(second.id, first.id, "应返回已存在那行的 id");

    assert_eq!(db.observation_count().unwrap(), 1);
    assert_eq!(db.event_count().unwrap(), 1, "重复投递不应追加事件");
}

#[test]
fn different_content_creates_different_rows() {
    let (_dir, db) = open();

    db.insert_observation(&observation("obs-1", FIXTURE_EPOCH_MS, "idem-1"))
        .unwrap();
    db.insert_observation(&observation("obs-2", 1_790_758_860_000, "idem-2"))
        .unwrap();

    assert_eq!(db.observation_count().unwrap(), 2);
    assert_eq!(db.event_count().unwrap(), 2);
}

// ---------------------------------------------------------------- 字段往返

#[test]
fn observation_row_roundtrips_all_fields() {
    let (_dir, db) = open();
    let original = observation("obs-1", FIXTURE_EPOCH_MS, "idem-1");

    db.insert_observation(&original).unwrap();
    let rows = db.query_observations(&ObservationQuery::default()).unwrap();

    assert_eq!(rows.len(), 1);
    let row = &rows[0];

    assert_eq!(row.id, "obs-1");
    assert_eq!(row.ts, Timestamp::from_millis(FIXTURE_EPOCH_MS));
    assert_eq!(row.source_id, "screen:display-1");
    assert_eq!(row.app_name.as_deref(), Some("Visual Studio Code"));
    assert_eq!(row.app_bundle_id.as_deref(), Some("com.microsoft.VSCode"));
    assert_eq!(row.window_title.as_deref(), Some("main.rs — MineContext"));
    assert_eq!(row.display_id.as_deref(), Some("display-1"));
    assert_eq!(row.change_kind, "pixel_major");
    assert_eq!(row.privacy_verdict, "allowed");
    assert_eq!(row.scale_factor, Some(2.0));
    assert_eq!(row.analysis_status.as_deref(), Some("pending"));
}

/// phash 是 u64，但 SQLite 的 INTEGER 是有符号 64 位 —— 必须无损往返。
#[test]
fn phash_survives_the_unsigned_to_signed_roundtrip() {
    let (_dir, db) = open();
    let mut obs = observation("obs-1", FIXTURE_EPOCH_MS, "idem-1");
    obs.phash = Some(u64::MAX); // 全 1，作为 i64 是 -1

    db.insert_observation(&obs).unwrap();

    let rows = db.query_observations(&ObservationQuery::default()).unwrap();
    assert_eq!(rows[0].phash, Some(u64::MAX), "phash 必须无损往返");
}

/// ：DB 只存路径与哈希，不存图片二进制。
#[test]
fn observation_images_are_not_stored_in_db() {
    let (_dir, db) = open();
    db.insert_observation(&observation("obs-1", FIXTURE_EPOCH_MS, "idem-1"))
        .unwrap();

    let rows = db.query_observations(&ObservationQuery::default()).unwrap();
    let row = &rows[0];

    assert_eq!(
        row.image_path.as_deref(),
        Some("screenshots/2026/09/30/abc123.png")
    );
    assert_eq!(row.image_blob_hash.as_deref(), Some("abc123"));
    assert_eq!(
        row.thumbnail_path.as_deref(),
        Some("thumbnails/2026/09/30/abc123-thumb.png")
    );
    assert_eq!(row.image_bytes, Some(12_345));
    assert_eq!(row.image_w, Some(640));
    assert_eq!(row.image_h, Some(400));
}

/// 隐私拦截的观测不落图像 —— 只留一条可审计的元数据。
#[test]
fn privacy_blocked_observation_has_no_image() {
    let (_dir, db) = open();
    let mut obs = observation("obs-1", FIXTURE_EPOCH_MS, "idem-1");
    obs.privacy_verdict = "blocked".to_string();
    obs.image = None;
    obs.window_title = None;

    db.insert_observation(&obs).unwrap();

    let rows = db.query_observations(&ObservationQuery::default()).unwrap();
    assert_eq!(rows[0].image_path, None);
    assert_eq!(rows[0].image_blob_hash, None);
    assert_eq!(rows[0].privacy_verdict, "blocked");
}

#[test]
fn observation_without_image_is_allowed() {
    let (_dir, db) = open();
    let mut obs = observation("obs-1", FIXTURE_EPOCH_MS, "idem-1");
    obs.image = None; // 例如只有窗口元数据的廉价观测

    db.insert_observation(&obs).unwrap();

    let rows = db.query_observations(&ObservationQuery::default()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].image_path, None);
}

// ---------------------------------------------------------------- 时间线读取

#[test]
fn observations_are_returned_newest_first() {
    let (_dir, db) = open();

    for i in 0..5 {
        db.insert_observation(&observation(
            &format!("obs-{i}"),
            FIXTURE_EPOCH_MS + i * 1_000,
            &format!("idem-{i}"),
        ))
        .unwrap();
    }

    let rows = db.query_observations(&ObservationQuery::default()).unwrap();

    assert_eq!(rows.len(), 5);
    assert_eq!(rows[0].id, "obs-4", "应当最新在前（时间线语义）");
    assert_eq!(rows[4].id, "obs-0");
}

#[test]
fn observations_filter_by_time_range() {
    let (_dir, db) = open();

    for i in 0..5 {
        db.insert_observation(&observation(
            &format!("obs-{i}"),
            1_000_000 + i * 1_000,
            &format!("idem-{i}"),
        ))
        .unwrap();
    }

    let rows = db
        .query_observations(&ObservationQuery {
            from: Some(Timestamp::from_millis(1_002_000)),
            to: Some(Timestamp::from_millis(1_004_000)),
            ..Default::default()
        })
        .unwrap();

    // [from, to) —— 半开区间，避免相邻区间重复计算
    let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, vec!["obs-3", "obs-2"]);
}

#[test]
fn observations_respect_limit_and_offset() {
    let (_dir, db) = open();
    for i in 0..10 {
        db.insert_observation(&observation(
            &format!("obs-{i}"),
            1_000_000 + i * 1_000,
            &format!("idem-{i}"),
        ))
        .unwrap();
    }

    let page = db
        .query_observations(&ObservationQuery {
            limit: Some(3),
            offset: Some(2),
            ..Default::default()
        })
        .unwrap();

    let ids: Vec<&str> = page.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, vec!["obs-7", "obs-6", "obs-5"]);
}

#[test]
fn observations_filter_by_app() {
    let (_dir, db) = open();
    db.insert_observation(&observation("obs-1", 1_000_000, "idem-1"))
        .unwrap();
    let mut other = observation("obs-2", 1_001_000, "idem-2");
    other.app_name = Some("Google Chrome".to_string());
    db.insert_observation(&other).unwrap();

    let rows = db
        .query_observations(&ObservationQuery {
            app_name: Some("Google Chrome".to_string()),
            ..Default::default()
        })
        .unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "obs-2");
}

#[test]
fn observations_can_exclude_privacy_blocked() {
    let (_dir, db) = open();
    let mut blocked = observation("obs-1", 1_000_000, "idem-1");
    blocked.privacy_verdict = "blocked".to_string();
    db.insert_observation(&blocked).unwrap();
    db.insert_observation(&observation("obs-2", 1_001_000, "idem-2"))
        .unwrap();

    let rows = db
        .query_observations(&ObservationQuery {
            exclude_blocked: true,
            ..Default::default()
        })
        .unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "obs-2");
}

#[test]
fn latest_observation_is_available_for_the_home_card() {
    let (_dir, db) = open();
    assert!(db.latest_observation().unwrap().is_none());

    db.insert_observation(&observation("obs-1", 1_000_000, "idem-1"))
        .unwrap();
    db.insert_observation(&observation("obs-2", 1_002_000, "idem-2"))
        .unwrap();

    let latest = db.latest_observation().unwrap().expect("应当有最新观测");
    assert_eq!(latest.id, "obs-2");
}

#[test]
fn analysis_row_starts_pending_and_is_not_duplicated() {
    let (_dir, db) = open();
    let obs = observation("obs-1", 1_000_000, "idem-1");

    db.insert_observation(&obs).unwrap();
    db.insert_observation(&obs).unwrap(); // 重复投递

    let rows = db.query_observations(&ObservationQuery::default()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].analysis_status.as_deref(), Some("pending"));
}
