//! 、0.22：事件存储。
//!
//! 事件表是唯一真源：
//! 因此三件事必须成立：seq 单调、payload 无损往返、并发追加被正确串行化。

use std::sync::Arc;

use mc_common::time::Timestamp;
use mc_storage::events::{EventEnvelope, NewEvent};
use mc_storage::Database;
use serde_json::json;

fn open_temp() -> (tempfile::TempDir, Arc<Database>) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("minecontext.db");
    let db = Arc::new(Database::open(&path).unwrap());
    (dir, db)
}

fn sample_event(kind: &str, at_ms: i64) -> NewEvent {
    NewEvent::new(kind, Timestamp::from_millis(at_ms), json!({ "n": at_ms }))
}

#[test]
fn event_append_is_monotonic() {
    let (_dir, db) = open_temp();

    let first = db.append_events(&[sample_event("a", 1_000)]).unwrap();
    let second = db
        .append_events(&[sample_event("b", 1_100), sample_event("c", 1_200)])
        .unwrap();
    let third = db.append_events(&[sample_event("d", 1_300)]).unwrap();

    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 2);
    assert_eq!(third.len(), 1);

    let all: Vec<i64> = first
        .iter()
        .chain(second.iter())
        .chain(third.iter())
        .copied()
        .collect();
    for pair in all.windows(2) {
        assert!(pair[0] < pair[1], "seq 必须严格递增: {pair:?}");
    }

    assert_eq!(db.last_seq().unwrap(), *all.last().unwrap());
    assert_eq!(db.event_count().unwrap(), 4);
}

// 0.18b — 空批次是合法的 no-op
#[test]
fn appending_empty_batch_is_a_noop() {
    let (_dir, db) = open_temp();

    let ids = db.append_events(&[]).unwrap();
    assert!(ids.is_empty());
    assert_eq!(db.event_count().unwrap(), 0);
    assert_eq!(db.last_seq().unwrap(), 0);
}

// 0.19 — payload 必须无损往返（含中文、嵌套、null、大整数）
#[test]
fn event_payload_roundtrip() {
    let (_dir, db) = open_temp();

    let payload = json!({
        "observation_id": "01926f4e-0000-7000-8000-000000000001",
        "app_name": "Visual Studio Code",
        "window_title": "main.rs — 事件溯源",
        "change_kind": "PixelMajor",
        "confidence": 0.91,
        "nested": { "objects": ["左右门状态", "MQTT"], "depth": { "n": 3 } },
        "nullable": null,
        "big": 9_007_199_254_740_993_i64,
        "flag": true
    });

    let event = NewEvent::new(
        "observation_captured",
        Timestamp::from_millis(1_756_000_000_000),
        payload.clone(),
    );
    let ids = db.append_events(&[event]).unwrap();

    let read_back: Vec<EventEnvelope> = db.read_events(0, 10).unwrap();
    assert_eq!(read_back.len(), 1);

    let got = &read_back[0];
    assert_eq!(got.seq, ids[0]);
    assert_eq!(got.kind, "observation_captured");
    assert_eq!(got.at, Timestamp::from_millis(1_756_000_000_000));
    assert_eq!(got.actor, "system");
    assert_eq!(got.schema_version, mc_storage::events::EVENT_SCHEMA_VERSION);
    assert_eq!(got.payload, payload, "payload 必须逐字段无损");
}

// 0.19b — 读取范围与排序
#[test]
fn read_events_respects_range_and_order() {
    let (_dir, db) = open_temp();

    for i in 0..10 {
        db.append_events(&[sample_event("tick", 1_000 + i)])
            .unwrap();
    }

    let all = db.read_events(0, 100).unwrap();
    assert_eq!(all.len(), 10);
    for pair in all.windows(2) {
        assert!(pair[0].seq < pair[1].seq);
    }

    let after_five = db.read_events(all[4].seq, 100).unwrap();
    assert_eq!(after_five.len(), 5);
    assert_eq!(after_five[0].seq, all[5].seq);

    let limited = db.read_events(0, 3).unwrap();
    assert_eq!(limited.len(), 3);
}

// 0.19c — 按时间范围读取（日报/时间段总结要用）
#[test]
fn read_events_by_time_range() {
    let (_dir, db) = open_temp();

    for i in 0..5 {
        db.append_events(&[sample_event("t", 1_000 + i * 1_000)])
            .unwrap();
    }

    let mid = db
        .read_events_in_time_range(Timestamp::from_millis(2_000), Timestamp::from_millis(4_000))
        .unwrap();

    assert_eq!(mid.len(), 2, "应为 [2000, 4000) 的两条");
    assert_eq!(mid[0].at, Timestamp::from_millis(2_000));
    assert_eq!(mid[1].at, Timestamp::from_millis(3_000));
}

// 0.22 — 并发追加必须被单写者串行化，且不出现 SQLITE_BUSY
#[test]
fn single_writer_serializes_concurrent_appends() {
    const THREADS: usize = 8;
    const PER_THREAD: usize = 100;

    let (_dir, db) = open_temp();
    let mut handles = Vec::new();

    for t in 0..THREADS {
        let db = Arc::clone(&db);
        handles.push(std::thread::spawn(move || {
            for i in 0..PER_THREAD {
                db.append_events(&[sample_event("concurrent", (t * 1_000 + i) as i64)])
                    .expect("并发追加不应失败（单写者串行化）");
            }
        }));
    }

    for h in handles {
        h.join().expect("worker thread must not panic");
    }

    let total = THREADS * PER_THREAD;
    assert_eq!(db.event_count().unwrap(), total as i64);

    let all = db.read_events(0, total + 10).unwrap();
    assert_eq!(all.len(), total);

    // seq 连续且唯一
    let mut seqs: Vec<i64> = all.iter().map(|e| e.seq).collect();
    seqs.sort_unstable();
    seqs.dedup();
    assert_eq!(seqs.len(), total, "seq 出现重复");
    assert_eq!(seqs[0], 1);
    assert_eq!(*seqs.last().unwrap(), total as i64);
}

// 0.22b — 批量事务：一个批次要么全成功要么全失败
#[test]
fn batch_append_is_atomic() {
    let (_dir, db) = open_temp();

    let batch: Vec<NewEvent> = (0..50).map(|i| sample_event("batch", i)).collect();
    let ids = db.append_events(&batch).unwrap();
    assert_eq!(ids.len(), 50);
    assert_eq!(db.event_count().unwrap(), 50);

    // 批次内 seq 连续
    for pair in ids.windows(2) {
        assert_eq!(pair[1], pair[0] + 1);
    }
}

// 0.19d — 事件必须先于派生数据可用：重启后仍能读到（append-only 的持久性）
#[test]
fn events_survive_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("minecontext.db");

    {
        let db = Database::open(&path).unwrap();
        db.append_events(&[sample_event("persisted", 42)]).unwrap();
    }

    let db = Database::open(&path).unwrap();
    let events = db.read_events(0, 10).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "persisted");
    assert_eq!(db.last_seq().unwrap(), events[0].seq);
}
