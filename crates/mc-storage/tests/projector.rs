//! 投影落库与重放。
//!
//! 投影结果写进 `activities` / `activity_observations` 是**派生数据**，
//! 随时可以丢掉重算。因此这里要守住三件事：
//!
//! - 重复写同一份投影结果不产生重复行（3.35，幂等）；
//! - 位点必须和派生表在**同一个事务**里推进（3.36），
//!   否则崩在中间会留下「位点已推进但数据只写了一半」的永久空洞；
//! - 全量重放能重建出完全相同的数据（3.37）。

use mc_common::time::Timestamp;
use mc_domain::activity::{ActivityView, ObservationRef, OverrideKind, Provenance};
use mc_domain::observation::ObservationSummary;
use mc_domain::projector::{project, DomainEvent, Projection, ProjectionOptions};
use mc_domain::rules::RuleSet;
use mc_storage::projectors::activities::{self, StoredActivity};
use mc_storage::Database;

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

fn at(offset_secs: i64) -> Timestamp {
    Timestamp::from_millis(T0 + offset_secs * 1_000)
}

fn open_db() -> (tempfile::TempDir, Database) {
    let dir = tempfile::tempdir().expect("临时目录");
    let db = Database::open(dir.path().join("mc.sqlite")).expect("打开数据库");
    (dir, db)
}

fn rules() -> RuleSet {
    RuleSet::parse_yaml(
        r#"
version: 1
activities:
  - id: coding
    name: 写代码
    category: 开发
    triggers:
      apps: [VSCode]
"#,
    )
    .expect("规则文件应当能解析")
}

fn observation(id: &str, at_secs: i64, app: &str, title: &str) -> DomainEvent {
    DomainEvent::ObservationRecorded {
        observation: ObservationSummary {
            id: id.to_string(),
            at: at(at_secs),
            app_name: Some(app.to_string()),
            window_title: Some(title.to_string()),
            domain: None,
            text: None,
        },
    }
}

fn history() -> Vec<DomainEvent> {
    vec![
        observation("obs-1", 0, "VSCode", "main.rs"),
        observation("obs-2", 60, "VSCode", "main.rs"),
        observation("obs-3", 600, "Chrome", "Hacker News"),
        observation("obs-4", 660, "Chrome", "Hacker News"),
    ]
}

fn projection_of(events: &[DomainEvent]) -> Projection {
    project(events, &rules(), ProjectionOptions::default())
}

fn store_sample(db: &Database) -> Projection {
    let events = history();
    let mut seqs = Vec::new();
    for event in &events {
        let stored = mc_storage::NewEvent::new(
            event.kind().to_string(),
            event.at().unwrap(),
            event.payload(),
        );
        seqs.extend(db.append_events(&[stored]).expect("追加事件"));
    }
    let projection = projection_of(&events);
    activities::store(db, &projection, *seqs.last().unwrap(), at(700)).expect("投影应当能落库");
    projection
}

#[test]
fn storing_the_same_projection_twice_is_idempotent() {
    let (_dir, db) = open_db();
    let events = history();
    let mut seqs = Vec::new();
    for event in &events {
        seqs.extend(
            db.append_events(&[mc_storage::NewEvent::new(
                event.kind().to_string(),
                event.at().unwrap(),
                event.payload(),
            )])
            .expect("追加事件"),
        );
    }
    let last_seq = *seqs.last().unwrap();
    let projection = projection_of(&events);

    activities::store(&db, &projection, last_seq, at(700)).expect("第一次投影");
    let first_rows = activities::read_all(&db).expect("读回投影");
    let first_links = activities::link_count(&db).expect("读回关联");

    // 同一个位点重放一次（进程重启、手动重算都会走到这里）
    activities::store(&db, &projection, last_seq, at(700)).expect("第二次投影");
    let second_rows = activities::read_all(&db).expect("读回投影");
    let second_links = activities::link_count(&db).expect("读回关联");

    assert_eq!(
        serde_json::to_string(&first_rows).unwrap(),
        serde_json::to_string(&second_rows).unwrap(),
        "重复投影必须覆盖而不是追加"
    );
    assert_eq!(first_links, second_links, "关联行也不能翻倍");
}

#[test]
fn checkpoint_advances_only_when_the_rows_are_written() {
    let (_dir, db) = open_db();
    store_sample(&db);

    let checkpoint = activities::checkpoint(&db).expect("读位点");
    assert_eq!(checkpoint, Some(4), "位点要推进到已投影的最后一条事件");

    // 让写入在事务中途失败：两条活动抢同一个 id
    let broken = Projection {
        activities: vec![
            sample_view("act-dup", 0, 60),
            sample_view("act-dup", 600, 660),
        ],
        noise: Vec::new(),
        ai_requests: 0,
        unknown_event_kinds: 0,
    };

    let result = activities::store(&db, &broken, 99, at(700));
    assert!(result.is_err(), "重复 id 应当被主键拦住");
    assert_eq!(
        activities::checkpoint(&db).expect("读位点"),
        Some(4),
        "写入失败时位点不能前进，否则这段历史永远不会被重投影"
    );
    assert_eq!(
        activities::read_all(&db).expect("读回投影").len(),
        2,
        "失败的事务不能留下半截数据"
    );
}

#[test]
fn replay_from_zero_rebuilds_the_same_rows() {
    let (_dir, db) = open_db();
    store_sample(&db);
    let before = activities::read_all(&db).expect("读回投影");

    // 手动清空派生表（算法升级后重算就是这个动作）
    activities::clear(&db).expect("清空派生表");
    let events = history();
    let projection = projection_of(&events);
    activities::store(&db, &projection, 4, at(700)).expect("重放");

    let after = activities::read_all(&db).expect("读回投影");
    assert_eq!(
        serde_json::to_string(&before).unwrap(),
        serde_json::to_string(&after).unwrap(),
        "重放后必须逐字段一致"
    );
}

// 落库内容本身要能解释「为什么是它」
#[test]
fn stored_rows_keep_provenance_and_evidence() {
    let (_dir, db) = open_db();
    store_sample(&db);

    let rows = activities::read_all(&db).expect("读回投影");
    assert_eq!(rows.len(), 2);

    let coding = rows
        .iter()
        .find(|row| row.title == "写代码")
        .expect("规则命中应当产出「写代码」");
    assert_eq!(
        coding.origin,
        Provenance::Rule {
            rule_id: "coding".to_string()
        },
        "规则来源必须落库，否则重放后无法解释"
    );
    assert_eq!(coding.evidence, vec!["obs-1", "obs-2"]);
    assert_eq!(coding.legacy_id, 1, "渲染层按整数 id 取数据，必须稳定");

    let browsing = rows
        .iter()
        .find(|row| row.title == "Chrome")
        .expect("兜底活动应当产出");
    assert_eq!(browsing.origin, Provenance::Observed);
    assert_eq!(browsing.legacy_id, 2);
}

// 用户修正经落库 + 重放后仍然在
#[test]
fn user_override_survives_a_round_trip() {
    let (_dir, db) = open_db();
    let mut events = history();
    events.push(DomainEvent::ActivityOverridden {
        override_: OverrideKind::Rename {
            activity_id: "act-obs-1".to_string(),
            title: "重构活动引擎".to_string(),
            at: at(700),
        },
    });

    for event in &events {
        db.append_events(&[mc_storage::NewEvent::new(
            event.kind().to_string(),
            event.at().unwrap(),
            event.payload(),
        )])
        .expect("追加事件");
    }

    let projection = projection_of(&events);
    activities::store(&db, &projection, events.len() as i64, at(700)).expect("投影");

    let rows = activities::read_all(&db).expect("读回投影");
    assert_eq!(
        rows[0].title, "重构活动引擎",
        "库里的标题必须是用户改过的那个"
    );
    assert_eq!(rows[0].original_title.as_deref(), Some("写代码"));
}

fn sample_view(id: &str, start_secs: i64, end_secs: i64) -> ActivityView {
    ActivityView {
        id: id.to_string(),
        start: at(start_secs),
        end: at(end_secs),
        title: "写代码".to_string(),
        original_title: "写代码".to_string(),
        category: Some("开发".to_string()),
        observations: vec![ObservationRef {
            id: format!("obs-{start_secs}"),
            at: at(start_secs),
        }],
        origin: Provenance::Rule {
            rule_id: "coding".to_string(),
        },
        confidence: 1.0,
        is_user_modified: false,
    }
}

/// 只是因为 `StoredActivity` 需要被断言，这里显式引用一下字段，
/// 避免将来字段改名时测试悄悄失效。
#[allow(dead_code)]
fn _assert_shape(row: &StoredActivity) -> (String, i64) {
    (row.id.clone(), row.legacy_id)
}

// 从事件日志整条链路重放（mc-cli replay / 算法升级后重算走的就是它）
#[test]
fn replay_reads_the_event_log_and_writes_rows() {
    let (_dir, db) = open_db();
    let events = history();
    for event in &events {
        db.append_events(&[mc_storage::NewEvent::new(
            event.kind().to_string(),
            event.at().unwrap(),
            event.payload(),
        )])
        .expect("追加事件");
    }

    let projection = activities::replay(&db, &rules(), ProjectionOptions::default(), at(700))
        .expect("重放应当成功");

    assert_eq!(projection.activities.len(), 2);
    assert_eq!(
        activities::checkpoint(&db).expect("读位点"),
        Some(events.len() as i64),
        "位点要指向最后一条已投影事件"
    );

    let rows = activities::read_all(&db).expect("读回投影");
    assert_eq!(rows.len(), projection.activities.len());
    assert_eq!(rows[0].evidence, vec!["obs-1", "obs-2"]);
}

// 前向兼容：日志里混进本版本不认识的事件时，重放不能整个失败
#[test]
fn replay_tolerates_unknown_events_in_the_log() {
    let (_dir, db) = open_db();
    db.append_events(&[
        mc_storage::NewEvent::new(
            "observation.recorded",
            at(0),
            observation_payload("obs-1", 0),
        ),
        mc_storage::NewEvent::new(
            "observation.recorded",
            at(60),
            observation_payload("obs-2", 60),
        ),
        mc_storage::NewEvent::new("future.thing", at(90), serde_json::json!({"x": 1})),
    ])
    .expect("追加事件");

    let projection = activities::replay(&db, &rules(), ProjectionOptions::default(), at(120))
        .expect("不该因为一条未知事件就放弃整次重放");

    assert_eq!(projection.unknown_event_kinds, 1);
    assert_eq!(projection.activities.len(), 1, "已知事件仍要投影出活动");
}

fn observation_payload(id: &str, at_secs: i64) -> serde_json::Value {
    match observation(id, at_secs, "VSCode", "main.rs") {
        DomainEvent::ObservationRecorded { observation } => {
            serde_json::to_value(observation).expect("序列化观测")
        }
        _ => unreachable!(),
    }
}

// ---------------------------------------------------------------- 3.40 兼容 activity 表

fn seed_observation_with_image(db: &Database, id: &str, at_secs: i64) {
    let observation = mc_storage::observations::NewObservation {
        id: id.to_string(),
        ts: at(at_secs),
        source_id: "macos:screen".to_string(),
        kind: "screen".to_string(),
        app_name: Some("Visual Studio Code".to_string()),
        app_bundle_id: None,
        window_title: Some("main.rs".to_string()),
        domain: None,
        display_id: Some("display-1".to_string()),
        scale_factor: Some(2.0),
        image: Some(mc_storage::observations::ImageRef {
            relative_path: format!("screenshots/2026/09/30/{id}.png"),
            content_hash: format!("hash-{id}"),
            thumbnail_path: None,
            width: 100,
            height: 80,
            bytes: 1234,
        }),
        text_content: None,
        text_origin: None,
        change_kind: "pixel_major".to_string(),
        privacy_verdict: "allowed".to_string(),
        phash: Some(1),
        idempotency: format!("idem-{id}"),
    };
    db.insert_observation(&observation).expect("写入观测");
}

fn legacy_rows(db: &Database) -> Vec<serde_json::Value> {
    db.with_read(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, title, content, resources, metadata, start_time, end_time
             FROM activity ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, i64>("id")?,
                "title": row.get::<_, String>("title")?,
                "content": row.get::<_, Option<String>>("content")?,
                "resources": row.get::<_, Option<String>>("resources")?,
                "metadata": row.get::<_, Option<String>>("metadata")?,
                "start_time": row.get::<_, Option<String>>("start_time")?,
                "end_time": row.get::<_, Option<String>>("end_time")?,
            }))
        })?;
        rows.collect::<Result<Vec<_>, _>>()
    })
    .expect("读取兼容表")
}

fn store_with_legacy(db: &Database) -> Projection {
    let events = history();
    for (index, event) in events.iter().enumerate() {
        // 观测必须真实存在，resources 才能带上截图路径
        let id = format!("obs-{}", index + 1);
        seed_observation_with_image(db, &id, (index as i64) * 60);
        db.append_events(&[mc_storage::NewEvent::new(
            event.kind().to_string(),
            event.at().unwrap(),
            event.payload(),
        )])
        .expect("追加事件");
    }

    let projection = projection_of(&events);
    activities::store(db, &projection, events.len() as i64, at(700)).expect("投影");
    projection
}

#[test]
fn legacy_rows_keep_json_columns_as_strings() {
    let (_dir, db) = open_db();
    store_with_legacy(&db);

    let rows = legacy_rows(&db);
    assert_eq!(rows.len(), 2, "每个活动都要在兼容表里有一行");

    let resources = rows[0]["resources"]
        .as_str()
        .expect("resources 必须是 JSON 字符串：渲染层直接 JSON.parse，不是字符串就白屏");
    let parsed: serde_json::Value =
        serde_json::from_str(resources).expect("resources 必须是合法 JSON");
    assert_eq!(parsed[0]["type"], "image");
    assert_eq!(parsed[0]["id"], "obs-1");
    assert!(parsed[0]["path"].as_str().unwrap().ends_with("obs-1.png"));

    let metadata = rows[0]["metadata"]
        .as_str()
        .expect("metadata 也必须是 JSON 字符串");
    let meta: serde_json::Value = serde_json::from_str(metadata).unwrap();
    assert_eq!(meta["origin"], "rule");
    assert_eq!(meta["activity_id"], "act-obs-1");
    assert_eq!(meta["projector"], "activities");
}

// 时间列必须是旧格式：前端用 dayjs 解析 `YYYY-MM-DD HH:mm:ss`
#[test]
fn legacy_rows_use_sqlite_datetime_format() {
    let (_dir, db) = open_db();
    store_with_legacy(&db);

    let rows = legacy_rows(&db);
    let start = rows[0]["start_time"].as_str().unwrap();
    assert_eq!(
        start, "2026-09-30 09:00:00",
        "必须是 UTC 的 `YYYY-MM-DD HH:mm:ss`，换任何格式渲染层都会解析成 Invalid Date"
    );
    assert_eq!(
        rows[1]["start_time"].as_str().unwrap(),
        "2026-09-30 09:10:00"
    );
    assert_eq!(rows[1]["end_time"].as_str().unwrap(), "2026-09-30 09:11:00");
}

// 3.41（兼容表部分）
#[test]
fn legacy_ids_survive_replay_and_do_not_clobber_imported_rows() {
    let (_dir, db) = open_db();

    // 假设旧库已经有一行不是我们写的活动（迁移时会遇到）
    db.with_write(|conn| {
        conn.execute(
            "INSERT INTO activity (id, title, content, resources, metadata, start_time, end_time)
             VALUES (900, '旧数据', '', '[]', '{\"source\":\"legacy\"}',
                     '2026-09-01 00:00:00', '2026-09-01 00:10:00')",
            [],
        )?;
        Ok(())
    })
    .expect("插入旧数据");

    store_with_legacy(&db);
    let first = legacy_rows(&db);
    assert_eq!(first.len(), 3, "旧数据不能被投影抹掉");
    let legacy_row_ids: Vec<i64> = first.iter().map(|r| r["id"].as_i64().unwrap()).collect();
    assert!(legacy_row_ids.contains(&900), "旧行必须留着");

    // 再投影一次：我们写的行要覆盖，不能重复，也不能挪动别人的行
    let events = history();
    let projection = projection_of(&events);
    activities::store(&db, &projection, 4, at(700)).expect("再投影一次");

    let second = legacy_rows(&db);
    assert_eq!(
        serde_json::to_string(&first).unwrap(),
        serde_json::to_string(&second).unwrap(),
        "重复投影必须得到完全一样的兼容表"
    );
}

// 采集链路 → 事件日志 → 活动：这条环路不通，前面所有测试都只是自娱自乐
#[test]
fn capture_events_feed_the_activity_projector() {
    let (_dir, db) = open_db();

    // 采集写入的是 `observation_captured`（ 既有形状），
    // 投影器必须认它，否则线上会出现「有观测却永远没有活动」。
    for (index, at_secs) in [0i64, 60].iter().enumerate() {
        seed_observation_with_image(&db, &format!("obs-{}", index + 1), *at_secs);
    }
    assert_eq!(db.event_count().expect("事件数"), 2);
    assert_eq!(
        db.read_events(0, 10).unwrap()[0].kind,
        "observation_captured"
    );

    let projection = activities::replay(&db, &rules(), ProjectionOptions::default(), at(700))
        .expect("重放采集事件");

    assert_eq!(
        projection.activities.len(),
        1,
        "两条同应用的观测应当合成一个活动"
    );
    assert_eq!(
        projection.activities[0].observations.len(),
        2,
        "观测必须挂到活动上，否则时间线里就是一张张孤立截图"
    );
    assert_eq!(
        projection.activities[0].title, "写代码",
        "规则要在真实采集事件上生效（app_name 是 Visual Studio Code）"
    );
    assert_eq!(
        projection.activities[0].origin,
        Provenance::Rule {
            rule_id: "coding".to_string()
        }
    );
}

// 采集事件里的关键词文本要能驱动规则（否则关键词规则在真实数据上永远不命中）
#[test]
fn keyword_rules_work_on_real_capture_events() {
    let (_dir, db) = open_db();

    for (index, at_secs) in [0i64, 60].iter().enumerate() {
        let observation = mc_storage::observations::NewObservation {
            id: format!("obs-{}", index + 1),
            ts: at(*at_secs),
            source_id: "macos:screen".to_string(),
            kind: "screen".to_string(),
            app_name: Some("Google Chrome".to_string()),
            app_bundle_id: None,
            window_title: Some("APEX-389 需求评审".to_string()),
            domain: Some("jira.example.com".to_string()),
            display_id: None,
            scale_factor: Some(2.0),
            image: None,
            text_content: Some("需求评审 讨论 APEX-389 的验收标准".to_string()),
            text_origin: Some("ocr".to_string()),
            change_kind: "pixel_major".to_string(),
            privacy_verdict: "allowed".to_string(),
            phash: None,
            idempotency: format!("idem-{at_secs}"),
        };
        db.insert_observation(&observation).expect("写入观测");
    }

    let rules = RuleSet::parse_yaml(
        r#"
version: 1
activities:
  - id: requirement-review
    name: 需求评审
    category: 需求
    triggers:
      keywords: [需求评审]
"#,
    )
    .expect("规则文件");

    let projection =
        activities::replay(&db, &rules, ProjectionOptions::default(), at(700)).expect("重放");

    assert_eq!(projection.activities.len(), 1);
    assert_eq!(projection.activities[0].title, "需求评审");
    assert_eq!(
        projection.activities[0].origin,
        Provenance::Rule {
            rule_id: "requirement-review".to_string()
        }
    );
}

// 定时投影：日志没变就不该重算（真机上每 30 秒全量重放一次也要便宜）
#[test]
fn project_if_dirty_skips_when_the_log_has_not_changed() {
    let (_dir, db) = open_db();
    seed_observation_with_image(&db, "obs-1", 0);
    seed_observation_with_image(&db, "obs-2", 60);

    let first = activities::project_if_dirty(&db, &rules(), ProjectionOptions::default(), at(700))
        .expect("第一次应当真的投影");
    assert!(first.is_some(), "有未投影的事件就必须重算");

    let second = activities::project_if_dirty(&db, &rules(), ProjectionOptions::default(), at(760))
        .expect("第二次应当跳过");
    assert!(second.is_none(), "日志没变还重算，等于每 30 秒白烧一次 CPU");

    // 新观测到来后必须重新生效
    seed_observation_with_image(&db, "obs-3", 600);
    seed_observation_with_image(&db, "obs-4", 660);
    let third = activities::project_if_dirty(&db, &rules(), ProjectionOptions::default(), at(1300))
        .expect("有新事件就要重算")
        .expect("应当返回投影结果");
    assert_eq!(third.activities.len(), 2);
}
