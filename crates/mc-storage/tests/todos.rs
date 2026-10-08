//! 首页数据面（`todo` / `tips`）。
//!
//! 数据面在 daemon 侧的 HTTP 接口上，行形状与排序必须逐字对齐，
//! 否则首页的任务卡片会顺序错乱或整块消失。
//! **时间格式最容易出错**：写入侧用 `dayjs(...).toISOString()` 写 `start_time`，
//! 渲染层也用 ISO 字符串做范围过滤（`startOf('day').toISOString()`）。
//! 因此 todo 的时间列**必须写 ISO**，与边界字符串才可比；写兼容层的
//! `YYYY-MM-DD HH:MM:SS` 会让 `start_time >= '2026-09-30T00:00:00.000Z'` 这类比较永远为假。

use mc_common::time::Timestamp;
use mc_storage::todos::{NewTodo, TodoPatch, TodoQuery};
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
const MINUTE: i64 = 60_000;

fn todo(content: &str, urgency: i64) -> NewTodo {
    NewTodo {
        content: content.to_string(),
        start_time: None,
        end_time: None,
        status: 0,
        urgency,
        assignee: None,
        reason: None,
    }
}

// ---------------------------------------------------------------- 行形状

#[test]
fn todo_rows_match_the_legacy_shape() {
    let (_dir, db) = open();
    let id = db
        .insert_todo(&todo("写一个待办的测试", 2), ms(T0))
        .unwrap();
    assert!(id > 0);

    let row = db.todo_by_id(id).unwrap().expect("必须能查到");
    assert_eq!(row.content, "写一个待办的测试");
    assert_eq!(row.status, 0);
    assert_eq!(row.urgency, 2);
    assert_eq!(row.end_time, None);
    assert_eq!(row.assignee, None);
    assert_eq!(row.reason, None);
    assert_eq!(
        row.start_time, "2026-09-30T09:00:00.000Z",
        "时间列必须写 ISO（前端用 ISO 字符串做范围比较）"
    );
    assert_eq!(row.created_at, "2026-09-30T09:00:00.000Z");
}

#[test]
fn explicit_times_and_optional_fields_are_preserved() {
    let (_dir, db) = open();
    let id = db
        .insert_todo(
            &NewTodo {
                content: "开会".to_string(),
                start_time: Some(ms(T0 + 30 * MINUTE)),
                end_time: Some(ms(T0 + 60 * MINUTE)),
                status: 0,
                urgency: 1,
                assignee: Some("我".to_string()),
                reason: Some("周会".to_string()),
            },
            ms(T0),
        )
        .unwrap();

    let row = db.todo_by_id(id).unwrap().unwrap();
    assert_eq!(row.start_time, "2026-09-30T09:30:00.000Z");
    assert_eq!(row.end_time.as_deref(), Some("2026-09-30T10:00:00.000Z"));
    assert_eq!(row.assignee.as_deref(), Some("我"));
    assert_eq!(row.reason.as_deref(), Some("周会"));
}

// ---------------------------------------------------------------- 查询

#[test]
fn list_orders_by_urgency_then_recency() {
    let (_dir, db) = open();
    let _low = db.insert_todo(&todo("低优先级", 0), ms(T0)).unwrap();
    let _high_early = db.insert_todo(&todo("高优先级（早）", 5), ms(T0)).unwrap();
    let _high_late = db
        .insert_todo(&todo("高优先级（晚）", 5), ms(T0 + MINUTE))
        .unwrap();

    let rows = db.list_todos(&TodoQuery::default()).unwrap();
    assert_eq!(
        rows.iter()
            .map(|row| row.content.as_str())
            .collect::<Vec<_>>(),
        vec!["高优先级（晚）", "高优先级（早）", "低优先级"],
        "必须按 urgency DESC, created_at DESC 排序"
    );
}

#[test]
fn list_filters_by_start_time_range() {
    let (_dir, db) = open();
    db.insert_todo(
        &NewTodo {
            start_time: Some(ms(T0)),
            ..todo("今天的", 0)
        },
        ms(T0),
    )
    .unwrap();
    db.insert_todo(
        &NewTodo {
            start_time: Some(ms(T0 + 24 * 60 * MINUTE)),
            ..todo("明天的", 0)
        },
        ms(T0),
    )
    .unwrap();

    let rows = db
        .list_todos(&TodoQuery {
            start: Some(ms(T0)),
            end: Some(ms(T0 + 12 * 60 * MINUTE)),
            ..TodoQuery::default()
        })
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].content, "今天的");
}

#[test]
fn list_filters_by_status() {
    let (_dir, db) = open();
    let done = db.insert_todo(&todo("已完成", 0), ms(T0)).unwrap();
    let _pending = db.insert_todo(&todo("待办", 0), ms(T0)).unwrap();
    db.update_todo(
        done,
        &TodoPatch {
            status: Some(1),
            ..TodoPatch::default()
        },
        ms(T0 + MINUTE),
    )
    .unwrap();

    let completed = db
        .list_todos(&TodoQuery {
            status: Some(1),
            ..TodoQuery::default()
        })
        .unwrap();
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].content, "已完成");
}

// ---------------------------------------------------------------- 更新

#[test]
fn patch_touches_only_the_given_fields() {
    let (_dir, db) = open();
    let id = db
        .insert_todo(
            &NewTodo {
                assignee: Some("我".to_string()),
                ..todo("原标题", 1)
            },
            ms(T0),
        )
        .unwrap();

    let changes = db
        .update_todo(
            id,
            &TodoPatch {
                content: Some("新标题".to_string()),
                urgency: Some(9),
                ..TodoPatch::default()
            },
            ms(T0 + MINUTE),
        )
        .unwrap();
    assert_eq!(changes, 1);

    let row = db.todo_by_id(id).unwrap().unwrap();
    assert_eq!(row.content, "新标题");
    assert_eq!(row.urgency, 9);
    assert_eq!(row.assignee.as_deref(), Some("我"), "没给的字段不能被清空");
    assert_eq!(row.status, 0);
}

#[test]
fn patch_can_clear_optional_fields() {
    let (_dir, db) = open();
    let id = db
        .insert_todo(
            &NewTodo {
                assignee: Some("我".to_string()),
                reason: Some("周会".to_string()),
                end_time: Some(ms(T0 + MINUTE)),
                ..todo("开会", 0)
            },
            ms(T0),
        )
        .unwrap();

    db.update_todo(
        id,
        &TodoPatch {
            assignee: Some(None),
            end_time: Some(None),
            ..TodoPatch::default()
        },
        ms(T0 + MINUTE),
    )
    .unwrap();

    let row = db.todo_by_id(id).unwrap().unwrap();
    assert_eq!(row.assignee, None);
    assert_eq!(row.end_time, None);
    assert_eq!(row.reason.as_deref(), Some("周会"), "别的字段不受影响");
}

/// 勾选/取消勾选：翻转状态，并在完成时补 `end_time`、取消时清掉。
///
/// 渲染层在本地做 `1 - status`，所以服务端**必须**也做翻转 ——
/// 否则前端显示的和库里存的会相反。
#[test]
fn toggle_flips_status_and_manages_end_time() {
    let (_dir, db) = open();
    let id = db.insert_todo(&todo("写测试", 0), ms(T0)).unwrap();

    db.toggle_todo_status(id, ms(T0 + MINUTE)).unwrap();
    let row = db.todo_by_id(id).unwrap().unwrap();
    assert_eq!(row.status, 1);
    assert_eq!(
        row.end_time.as_deref(),
        Some("2026-09-30T09:01:00.000Z"),
        "完成时补上完成时间"
    );

    db.toggle_todo_status(id, ms(T0 + 2 * MINUTE)).unwrap();
    let row = db.todo_by_id(id).unwrap().unwrap();
    assert_eq!(row.status, 0);
    assert_eq!(row.end_time, None, "取消完成时清掉完成时间");
}

#[test]
fn delete_removes_the_row() {
    let (_dir, db) = open();
    let id = db.insert_todo(&todo("待删", 0), ms(T0)).unwrap();

    assert_eq!(db.delete_todo(id).unwrap(), 1);
    assert_eq!(db.delete_todo(id).unwrap(), 0);
    assert!(db.todo_by_id(id).unwrap().is_none());
}

// ---------------------------------------------------------------- tips

#[test]
fn tips_round_trip_newest_first() {
    let (_dir, db) = open();
    db.insert_tip("先写失败的测试", ms(T0)).unwrap();
    db.insert_tip("再把测试跑绿", ms(T0 + MINUTE)).unwrap();

    let tips = db.list_tips(50).unwrap();
    assert_eq!(tips.len(), 2);
    assert_eq!(tips[0].content, "再把测试跑绿", "新的在前");
    assert!(tips[0].id > tips[1].id);
    assert!(!tips[0].created_at.is_empty());
}

#[test]
fn tips_limit_is_respected() {
    let (_dir, db) = open();
    for index in 0..5 {
        db.insert_tip(&format!("提示 {index}"), ms(T0 + index))
            .unwrap();
    }
    assert_eq!(db.list_tips(2).unwrap().len(), 2);
}
