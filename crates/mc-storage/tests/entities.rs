//! 实体关联表（`activity_entities`）持久化。
//!
//! 线索若是**每次查询时重新抽一遍实体**得到的，会有两个问题：
//! 1. 「APEX-389 这条线索一共动过几次」这类问题无法用 SQL 回答，只能把所有活动加载进内存再算；
//! 2. 隐私规则变化时，**已经抽出来的实体**没有任何地方可以清理 —— 被拦截的活动可能通过实体关联继续泄露。
//!
//! 因此实体关联要落库，并且**替换式写入**（按活动整体替换，而不是增量追加）：
//! 追加式写入在活动标题被用户改了之后会留下幽灵实体。

use mc_common::time::Timestamp;
use mc_storage::entities::ActivityEntity;
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

/// 实体关联是活动的**子表**（查询要 join 活动表拿开始时间），
/// 因此夹具必须先落活动 —— 这也顺带验证了「活动不存在时读不到实体」
/// 这条失败即关闭的行为。
///
/// **一次性写入全部活动**：`activities::store` 是覆盖式投影，
/// 分多次调用会让前面的活动消失。
fn seed_activities(db: &Database, rows: &[(&str, &str, i64)]) {
    let projection = mc_domain::projector::Projection {
        activities: rows
            .iter()
            .map(|(id, title, at_ms)| mc_domain::activity::ActivityView {
                id: id.to_string(),
                start: Timestamp::from_millis(*at_ms),
                end: Timestamp::from_millis(*at_ms + 60_000),
                title: title.to_string(),
                original_title: title.to_string(),
                category: Some("开发".to_string()),
                observations: Vec::new(),
                origin: mc_domain::activity::Provenance::Rule {
                    rule_id: "coding".to_string(),
                },
                confidence: 1.0,
                is_user_modified: false,
            })
            .collect(),
        noise: Vec::new(),
        ai_requests: 0,
        unknown_event_kinds: 0,
    };
    let seq = rows.iter().map(|(_, _, at)| *at).max().unwrap_or(T0);
    mc_storage::projectors::activities::store(db, &projection, 0, Timestamp::from_millis(seq))
        .expect("存活动");
}

fn entity(activity: &str, key: &str, display: &str, at_ms: i64) -> ActivityEntity {
    ActivityEntity {
        activity_id: activity.to_string(),
        kind: "issue".to_string(),
        key: key.to_string(),
        display: display.to_string(),
        start: ms(at_ms),
    }
}

#[test]
fn selected_activity_entities_exclude_other_rows_and_handle_empty_input() {
    let (_dir, db) = open();
    seed_activities(
        &db,
        &[("act-1", "first", T0), ("act-2", "second", T0 + 1000)],
    );
    db.replace_activity_entities("act-1", &[entity("act-1", "one", "ONE", T0)])
        .unwrap();
    db.replace_activity_entities("act-2", &[entity("act-2", "two", "TWO", T0 + 1000)])
        .unwrap();
    let selected = db
        .activity_entities_for_ids(&["act-2".into(), "act-2".into(), "missing".into()])
        .unwrap();
    assert_eq!(selected, vec![entity("act-2", "two", "TWO", T0 + 1000)]);
    assert!(db.activity_entities_for_ids(&[]).unwrap().is_empty());
}

#[test]
fn batch_sync_replaces_links_and_prunes_disallowed_activities() {
    let (_dir, db) = open();
    seed_activities(
        &db,
        &[("act-1", "first", T0), ("act-2", "second", T0 + 1000)],
    );
    db.replace_activity_entities(
        "act-2",
        &[
            entity("act-2", "old-1", "old-1", T0),
            entity("act-2", "old-2", "old-2", T0),
        ],
    )
    .unwrap();
    let changes = vec![("act-1".to_string(), vec![entity("act-1", "new", "NEW", T0)])];
    let cleared = db
        .sync_activity_entity_links(&changes, &["act-1".to_string()])
        .unwrap();
    assert_eq!(cleared, 1);
    assert_eq!(db.activity_entity_count().unwrap(), 1);
    assert_eq!(db.entity_count().unwrap(), 1);
    assert_eq!(db.entities_for_activity("act-1").unwrap()[0].display, "NEW");
    assert_eq!(db.sync_activity_entity_links(&[], &[]).unwrap(), 1);
    assert_eq!(db.entity_count().unwrap(), 0);
}

#[test]
fn batch_sync_rolls_back_when_a_link_write_fails() {
    let (_dir, db) = open();
    seed_activities(&db, &[("act-1", "first", T0)]);
    let original = entity("act-1", "old", "OLD", T0);
    db.replace_activity_entities("act-1", std::slice::from_ref(&original))
        .unwrap();
    db.with_write(|connection| {
        connection.execute_batch(
            "CREATE TRIGGER reject_link BEFORE INSERT ON activity_entities
             WHEN NEW.activity_id = 'missing'
             BEGIN SELECT RAISE(ABORT, 'link write failed'); END;",
        )
    })
    .unwrap();
    let result = db.sync_activity_entity_links(
        &[
            ("act-1".to_string(), vec![entity("act-1", "new", "NEW", T0)]),
            (
                "missing".to_string(),
                vec![entity("missing", "bad", "BAD", T0)],
            ),
        ],
        &["act-1".to_string(), "missing".to_string()],
    );
    assert!(result.is_err());
    assert_eq!(db.entities_for_activity("act-1").unwrap(), vec![original]);
}

// ---------------------------------------------------------------- 持久化

#[test]
fn activity_entities_are_persisted_and_readable() {
    let (_dir, db) = open();
    seed_activities(&db, &[("act-1", "修复 APEX-389", T0)]);

    db.replace_activity_entities(
        "act-1",
        &[
            entity("act-1", "APEX-389", "APEX-389", T0),
            entity("act-1", "BUG-12", "BUG-12", T0),
        ],
    )
    .expect("写入实体关联");

    let stored = db.entities_for_activity("act-1").unwrap();
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0].key, "APEX-389");
    assert_eq!(stored[0].display, "APEX-389");
    assert_eq!(db.activity_entity_count().unwrap(), 2);
}

// 替换式写入：同一活动重复同步不产生重复行
#[test]
fn replacing_entities_is_idempotent() {
    let (_dir, db) = open();
    seed_activities(&db, &[("act-1", "修复 APEX-389", T0)]);
    let entities = [entity("act-1", "APEX-389", "APEX-389", T0)];

    db.replace_activity_entities("act-1", &entities).unwrap();
    db.replace_activity_entities("act-1", &entities).unwrap();

    assert_eq!(db.activity_entity_count().unwrap(), 1);
}

// 标题被改掉之后，不再存在的实体必须消失（追加式写入会留下幽灵实体）
#[test]
fn entities_disappear_when_the_activity_no_longer_mentions_them() {
    let (_dir, db) = open();
    seed_activities(&db, &[("act-1", "修复 APEX-389", T0)]);

    db.replace_activity_entities("act-1", &[entity("act-1", "APEX-389", "APEX-389", T0)])
        .unwrap();

    db.replace_activity_entities("act-1", &[entity("act-1", "BUG-12", "BUG-12", T0)])
        .unwrap();

    let stored = db.entities_for_activity("act-1").unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].key, "BUG-12");
}

#[test]
fn entities_can_be_cleared_for_one_activity() {
    let (_dir, db) = open();
    seed_activities(&db, &[("act-1", "修复 APEX-389", T0)]);
    seed_activities(&db, &[("act-2", "再看 APEX-389", T0 + 1)]);

    db.replace_activity_entities("act-1", &[entity("act-1", "APEX-389", "APEX-389", T0)])
        .unwrap();
    db.replace_activity_entities("act-2", &[entity("act-2", "APEX-389", "APEX-389", T0 + 1)])
        .unwrap();

    assert_eq!(db.clear_activity_entities("act-1").unwrap(), 1);
    assert_eq!(db.activity_entity_count().unwrap(), 1);
    assert_eq!(db.entities_for_activity("act-2").unwrap().len(), 1);
}

// ---------------------------------------------------------------- 反查

// 「这个工单在哪些活动里出现过」必须能按时间升序直接查出来
#[test]
fn activities_for_an_entity_come_back_in_time_order() {
    let (_dir, db) = open();
    seed_activities(
        &db,
        &[
            ("act-late", "再看 APEX-389", T0 + 9),
            ("act-early", "开始 APEX-389", T0),
            ("act-other", "修 BUG-1", T0 + 5),
        ],
    );

    db.replace_activity_entities(
        "act-late",
        &[entity("act-late", "APEX-389", "APEX-389", T0 + 9)],
    )
    .unwrap();
    db.replace_activity_entities(
        "act-early",
        &[entity("act-early", "APEX-389", "apex-389", T0)],
    )
    .unwrap();
    db.replace_activity_entities(
        "act-other",
        &[entity("act-other", "BUG-1", "BUG-1", T0 + 5)],
    )
    .unwrap();

    let activities = db.activities_for_entity("issue", "APEX-389").unwrap();
    assert_eq!(
        activities
            .iter()
            .map(|item| item.activity_id.as_str())
            .collect::<Vec<_>>(),
        vec!["act-early", "act-late"],
        "同一实体的活动必须按时间升序，且不混入别的实体"
    );
    // 展示名保留原文（用户看到的样子），不因为归并而改写
    assert_eq!(activities[0].display, "apex-389");
}

// 同一活动可以属于多个实体（事实如此，不是重复）
#[test]
fn one_activity_can_belong_to_several_entities() {
    let (_dir, db) = open();
    seed_activities(&db, &[("act-1", "APEX-389 与 BUG-12", T0)]);

    db.replace_activity_entities(
        "act-1",
        &[
            entity("act-1", "APEX-389", "APEX-389", T0),
            entity("act-1", "BUG-12", "BUG-12", T0),
        ],
    )
    .unwrap();

    assert_eq!(
        db.activities_for_entity("issue", "APEX-389").unwrap().len(),
        1
    );
    assert_eq!(
        db.activities_for_entity("issue", "BUG-12").unwrap().len(),
        1
    );
}

// 空写入等于「这个活动没有任何实体」，必须能清干净（用户把标题改成了纯中文）
#[test]
fn writing_an_empty_set_clears_the_activity() {
    let (_dir, db) = open();
    seed_activities(&db, &[("act-1", "修复 APEX-389", T0)]);

    db.replace_activity_entities("act-1", &[entity("act-1", "APEX-389", "APEX-389", T0)])
        .unwrap();

    db.replace_activity_entities("act-1", &[]).unwrap();
    assert!(db.entities_for_activity("act-1").unwrap().is_empty());
    assert_eq!(db.activity_entity_count().unwrap(), 0);
}

// 去重：同一活动内重复的同 key 实体只留一行
#[test]
fn duplicate_entities_within_an_activity_are_deduplicated() {
    let (_dir, db) = open();
    seed_activities(&db, &[("act-1", "APEX-389 / apex-389", T0)]);

    db.replace_activity_entities(
        "act-1",
        &[
            entity("act-1", "APEX-389", "APEX-389", T0),
            entity("act-1", "APEX-389", "apex-389", T0),
        ],
    )
    .unwrap();

    assert_eq!(db.activity_entity_count().unwrap(), 1);
}
