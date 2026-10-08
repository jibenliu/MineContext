//! 通用设置 KV（旧 `electron-store` 的等价面）。
//!
//! 渲染层用它存的是**任意 JSON**：布尔（`todoList-finished`）、
//! 对象数组（`todoList`）、采集设置对象（`capture.atom`）。
//! 因此这一层不能按「键值都是字符串」实现 —— 那会把布尔读成 `"true"`，
//! 前端 `if (isFinished)` 于是永远为真。

use mc_common::error::ErrorCode;
use mc_common::time::Timestamp;
use mc_storage::Database;

fn open() -> (tempfile::TempDir, Database) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("minecontext.db");
    let db = Database::open(&path).unwrap();
    (dir, db)
}

fn ms(value: i64) -> Timestamp {
    Timestamp::from_millis(value)
}

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

#[test]
fn missing_setting_is_none() {
    let (_dir, db) = open();
    assert!(db.get_setting("nope").unwrap().is_none());
}

#[test]
fn round_trips_arbitrary_json() {
    let (_dir, db) = open();

    let values = [
        serde_json::json!(true),
        serde_json::json!(42),
        serde_json::json!("文本"),
        serde_json::json!([1, 2, 3]),
        serde_json::json!([{ "id": 1, "content": "写测试", "status": 0 }]),
        serde_json::json!({ "interval": 30, "targets": ["display-1"], "nested": { "a": null } }),
    ];

    for (index, value) in values.iter().enumerate() {
        let key = format!("key-{index}");
        db.set_setting(&key, value, ms(T0)).unwrap();
        assert_eq!(
            db.get_setting(&key).unwrap().as_ref(),
            Some(value),
            "键 {key} 必须原样往返"
        );
    }
}

#[test]
fn overwrites_existing_value() {
    let (_dir, db) = open();
    db.set_setting("k", &serde_json::json!(1), ms(T0)).unwrap();
    db.set_setting("k", &serde_json::json!({"a": 2}), ms(T0 + 1))
        .unwrap();

    assert_eq!(
        db.get_setting("k").unwrap(),
        Some(serde_json::json!({"a": 2}))
    );
    assert_eq!(db.setting_keys().unwrap(), vec!["k".to_string()]);
}

#[test]
fn clear_removes_and_is_idempotent() {
    let (_dir, db) = open();
    db.set_setting("k", &serde_json::json!(1), ms(T0)).unwrap();

    assert_eq!(db.clear_setting("k").unwrap(), 1);
    assert_eq!(db.clear_setting("k").unwrap(), 0);
    assert!(db.get_setting("k").unwrap().is_none());
}

#[test]
fn keys_are_listed_in_stable_order() {
    let (_dir, db) = open();
    for key in ["b", "a", "c"] {
        db.set_setting(key, &serde_json::json!(1), ms(T0)).unwrap();
    }
    assert_eq!(
        db.setting_keys().unwrap(),
        vec!["a".to_string(), "b".to_string(), "c".to_string()]
    );
}

#[test]
fn invalid_keys_are_rejected() {
    let (_dir, db) = open();

    let error = db
        .set_setting("", &serde_json::json!(1), ms(T0))
        .expect_err("空键必须被拒绝");
    assert_eq!(error.code(), ErrorCode::DomainInvalidRange);

    let long = "x".repeat(200);
    assert!(db
        .set_setting(&long, &serde_json::json!(1), ms(T0))
        .is_err());
    assert!(db.get_setting(&long).is_err());
}

/// 存进去的东西必须是合法 JSON（否则读出来时才发现坏了就太晚了）
#[test]
fn corrupted_value_is_reported_with_a_stable_code() {
    let (_dir, db) = open();
    db.with_write(|conn| {
        conn.execute(
            "INSERT INTO app_settings (key, value, updated_at) VALUES ('bad', 'not json', '2026-09-30 09:00:00')",
            [],
        )?;
        Ok(())
    })
    .unwrap();

    let error = db.get_setting("bad").expect_err("坏值必须报错");
    assert_eq!(error.code(), ErrorCode::StorageCorrupt);
}
