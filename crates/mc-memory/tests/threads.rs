//! 实体抽取与 Thread。
//!
//! 「APEX-389 我昨天查到哪里了？」这个问题要能回答，前提是系统知道
//! **同一个 ticket 在不同天出现过**。Thread 就是把「同一个实体的活动」
//! 串成一条线索，回答「这件事从哪开始、到哪了」。
//!
//! 三条要求：
//! - 抽取是**确定性纯函数**（离线可测，不调模型）；
//! - 别名要能归并（`APEX-389` / `apex 389` / `APEX_389` 是同一个东西）；
//! - Thread 保留 provenance（哪些是看见的、哪些是猜的）。

use chrono::NaiveDate;
use mc_common::time::Timestamp;
use mc_domain::activity::Provenance;
use mc_memory::entity::{extract, Entity, EntityKind, EntityRegistry};
use mc_memory::thread::{build_threads, thread_brief, ThreadActivity};

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

fn at(offset_secs: i64) -> Timestamp {
    Timestamp::from_millis(T0 + offset_secs * 1_000)
}

fn activity(id: &str, title: &str, day: i64, inferred: bool) -> ThreadActivity {
    ThreadActivity {
        id: id.to_string(),
        title: title.to_string(),
        start: at(day * 86_400),
        end: at(day * 86_400 + 1800),
        provenance: if inferred {
            Provenance::Inferred {
                model: "stub".to_string(),
            }
        } else {
            Provenance::Rule {
                rule_id: "coding".to_string(),
            }
        },
    }
}

#[test]
fn entity_extraction_from_activity_titles() {
    let entities = extract("排查 APEX-389 的验收标准");

    assert_eq!(entities.len(), 1, "{entities:?}");
    assert_eq!(entities[0].kind, EntityKind::Issue);
    assert_eq!(entities[0].canonical, "APEX-389");
}

#[test]
fn extraction_handles_files_and_ignores_noise() {
    let entities = extract("修复 crates/mc-domain/src/stage.rs 里的边界");
    assert_eq!(entities.len(), 1, "{entities:?}");
    assert_eq!(entities[0].kind, EntityKind::FileNote);
    assert!(
        entities[0].canonical.ends_with("stage.rs"),
        "文件实体应当是路径：{entities:?}"
    );

    // 普通词不该被当成实体
    assert!(extract("写代码").is_empty());
    assert!(extract("上午主要在重构").is_empty());
}

// 别名归并
#[test]
fn entity_aliases_are_merged() {
    let registry = EntityRegistry::with_defaults();

    let variants = ["APEX-389", "apex-389", "APEX 389", "APEX_389"];
    let canonical: Vec<String> = variants
        .iter()
        .map(|variant| registry.resolve(variant).canonical)
        .collect();

    assert!(
        canonical.windows(2).all(|pair| pair[0] == pair[1]),
        "别名变体必须归并到同一个实体：{canonical:?}"
    );

    // 用户自定义别名
    let registry = registry.merge_alias("验收标准跟进", "APEX-389");
    assert_eq!(registry.resolve("验收标准跟进").canonical, "APEX-389");
}

#[test]
fn different_entities_are_not_merged() {
    let registry = EntityRegistry::with_defaults();
    assert_ne!(
        registry.resolve("APEX-389").canonical,
        registry.resolve("APEX-390").canonical,
        "相邻编号不能被当成同一个实体"
    );
}

#[test]
fn activities_linked_by_same_entity() {
    let registry = EntityRegistry::with_defaults();
    let activities = vec![
        activity("act-1", "排查 APEX-389 的验收标准", 0, false),
        activity("act-2", "写文档", 0, false),
        activity("act-3", "继续 APEX-389 的修复", 1, false),
    ];

    let threads = build_threads(&activities, &registry);

    assert_eq!(threads.len(), 1, "只有 APEX-389 构成线索：{threads:?}");
    assert_eq!(threads[0].entity.canonical, "APEX-389");
    let ids: Vec<&str> = threads[0]
        .activities
        .iter()
        .map(|activity| activity.id.as_str())
        .collect();
    assert_eq!(
        ids,
        vec!["act-1", "act-3"],
        "按时间排序，且与实体无关的不进来"
    );
    assert_eq!(threads[0].days.len(), 2, "跨了两天");
}

#[test]
fn thread_brief_summarizes_cross_day_progress() {
    let registry = EntityRegistry::with_defaults();
    let activities = vec![
        activity("act-1", "排查 APEX-389 的验收标准", 0, false),
        activity("act-2", "继续 APEX-389 的修复", 1, false),
        activity("act-3", "APEX-389 回归验证", 3, false),
    ];
    let threads = build_threads(&activities, &registry);

    let brief = thread_brief(&threads[0]);

    assert!(brief.contains("APEX-389"), "{brief}");
    assert!(
        brief.contains("3 天") || brief.contains("跨 3 天"),
        "进展要说明跨了几天：{brief}"
    );
    for id_title in ["验收标准", "修复", "回归验证"] {
        assert!(brief.contains(id_title), "每天的进展都要出现：{brief}");
    }
    // 每条进展带日期（用户要能对上自己的记忆）
    assert!(brief.contains("2026-09-30"), "{brief}");
}

// provenance 区分
#[test]
fn thread_respects_provenance() {
    let registry = EntityRegistry::with_defaults();
    let activities = vec![
        activity("act-1", "排查 APEX-389", 0, false),
        activity("act-2", "APEX-389 的后续处理", 1, true),
    ];

    let threads = build_threads(&activities, &registry);
    let brief = thread_brief(&threads[0]);

    assert!(
        brief.contains("推测") || brief.contains("inferred"),
        "猜出来的进展必须标注出来：{brief}"
    );
    assert_eq!(
        threads[0].inferred_count(),
        1,
        "Thread 要能说出其中有多少条是推断的"
    );
    assert_eq!(threads[0].observed_count(), 1);
}

// 同一个活动提到两个实体 → 两条线索都要包含它
#[test]
fn one_activity_can_join_several_threads() {
    let registry = EntityRegistry::with_defaults();
    let activities = vec![
        activity("act-1", "APEX-389 牵连到 BUG-12", 0, false),
        activity("act-2", "BUG-12 单独处理", 1, false),
    ];

    let threads = build_threads(&activities, &registry);

    assert_eq!(threads.len(), 2, "{threads:?}");
    let apex = threads
        .iter()
        .find(|thread| thread.entity.canonical == "APEX-389")
        .unwrap();
    assert_eq!(apex.activities.len(), 1);
    let bug = threads
        .iter()
        .find(|thread| thread.entity.canonical == "BUG-12")
        .unwrap();
    assert_eq!(bug.activities.len(), 2);
}

// 空输入不炸，也不产出空线索
#[test]
fn no_entities_means_no_threads() {
    let registry = EntityRegistry::with_defaults();
    let activities = vec![activity("act-1", "写代码", 0, false)];
    assert!(build_threads(&activities, &registry).is_empty());
}

// 文件实体的路径变体要归并（同一个文件的相对/绝对写法）
#[test]
fn file_entity_variants_merge_to_one_thread() {
    let registry = EntityRegistry::with_defaults();
    let activities = vec![
        activity("act-1", "改 src/stage.rs", 0, false),
        activity("act-2", "又看了一遍 src/stage.rs", 1, false),
    ];

    let threads = build_threads(&activities, &registry);
    assert_eq!(threads.len(), 1, "{threads:?}");
    assert_eq!(threads[0].activities.len(), 2);
}

// Thread 的日期列表要唯一且有序
#[test]
fn thread_days_are_unique_and_sorted() {
    let registry = EntityRegistry::with_defaults();
    let activities = vec![
        activity("act-3", "APEX-389 第三天", 3, false),
        activity("act-1", "APEX-389 第一天", 0, false),
        activity("act-2", "APEX-389 第一天下午", 0, false),
    ];

    let threads = build_threads(&activities, &registry);
    let days: Vec<NaiveDate> = threads[0].days.clone();

    assert_eq!(days.len(), 2, "同一天只算一次：{days:?}");
    assert!(days[0] < days[1]);
}

// Entity 的展示名与规范化名分开：展示保留用户看到的样子
#[test]
fn entity_keeps_display_form() {
    let entities = extract("排查 APEX-389 的验收标准");
    assert_eq!(entities[0].display, "APEX-389");
    assert_eq!(entities[0].canonical, "APEX-389");

    let registry = EntityRegistry::with_defaults();
    let resolved = registry.resolve("apex 389");
    assert_eq!(resolved.canonical, "APEX-389");
    assert_eq!(resolved.display, "apex 389", "展示名保留原文，不要擅自改写");
    let _ = Entity {
        kind: EntityKind::Issue,
        canonical: "X-1".to_string(),
        display: "X-1".to_string(),
    };
}

// ---------------------------------------------------------------- 落库实体

/// 从**落库的实体关联**组装线索。
///
/// 抽取是确定性的，但「哪个活动属于哪个实体」一旦落库就是事实，
/// 线索必须以后者为准：活动标题被用户改掉之后，历史关联不该消失。
#[test]
fn threads_can_be_built_from_persisted_links() {
    use mc_memory::entity::{Entity, EntityKind};
    use mc_memory::thread::{build_threads_from_links, ThreadActivity, ThreadEntityLink};

    let activity = ThreadActivity {
        id: "act-1".to_string(),
        // 标题里**没有**任何实体形态的词
        title: "上午在改代码".to_string(),
        start: mc_common::time::Timestamp::from_millis(T0),
        end: mc_common::time::Timestamp::from_millis(1_790_760_600_000),
        provenance: mc_domain::activity::Provenance::Observed,
    };

    let links = [ThreadEntityLink {
        activity_id: "act-1".to_string(),
        entity: Entity {
            kind: EntityKind::Issue,
            canonical: "APEX-389".to_string(),
            display: "apex-389".to_string(),
        },
    }];

    let threads = build_threads_from_links(std::slice::from_ref(&activity), &links, "UTC");
    assert_eq!(threads.len(), 1, "落库的关联必须能建出线索");
    assert_eq!(threads[0].entity.canonical, "APEX-389");
    // 展示名保留原文
    assert_eq!(threads[0].entity.display, "apex-389");
    assert_eq!(threads[0].activities.len(), 1);
}

/// 没有任何关联的活动不产生线索（而不是退回标题抽取）
#[test]
fn activities_without_links_produce_no_threads() {
    use mc_memory::thread::{build_threads_from_links, ThreadActivity};

    let activity = ThreadActivity {
        id: "act-1".to_string(),
        title: "修复 APEX-389".to_string(),
        start: mc_common::time::Timestamp::from_millis(T0),
        end: mc_common::time::Timestamp::from_millis(1_790_760_600_000),
        provenance: mc_domain::activity::Provenance::Observed,
    };

    assert!(
        build_threads_from_links(&[activity], &[], "UTC").is_empty(),
        "没有关联就不该凭标题猜出线索 —— 落库路径必须以关联表为准"
    );
}

/// 一个活动属于多条线索（提到两个实体时）
#[test]
fn one_activity_can_appear_in_several_threads_from_links() {
    use mc_memory::entity::{Entity, EntityKind};
    use mc_memory::thread::{build_threads_from_links, ThreadActivity, ThreadEntityLink};

    let activity = ThreadActivity {
        id: "act-1".to_string(),
        title: "APEX-389 与 BUG-12".to_string(),
        start: mc_common::time::Timestamp::from_millis(T0),
        end: mc_common::time::Timestamp::from_millis(1_790_760_600_000),
        provenance: mc_domain::activity::Provenance::Observed,
    };

    let links = [
        ThreadEntityLink {
            activity_id: "act-1".to_string(),
            entity: Entity {
                kind: EntityKind::Issue,
                canonical: "APEX-389".to_string(),
                display: "APEX-389".to_string(),
            },
        },
        ThreadEntityLink {
            activity_id: "act-1".to_string(),
            entity: Entity {
                kind: EntityKind::Issue,
                canonical: "BUG-12".to_string(),
                display: "BUG-12".to_string(),
            },
        },
    ];

    let threads = build_threads_from_links(&[activity], &links, "UTC");
    assert_eq!(threads.len(), 2);
    assert!(threads.iter().all(|thread| thread.activities.len() == 1));
}
