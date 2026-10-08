//! 实体关联的同步与隐私清理。
//!
//! 落库之后必须回答两个新问题：
//!
//! 1. **同步是幂等的吗**？实体抽取是纯函数，重复同步不该反复写库；
//! 2. **隐私规则变化时会发生什么**？一条活动被判为拦截之后，
//!    它已经落库的实体关联必须被清掉 ——
//!    否则用户还能通过「相关记录」看到被拦截的内容。

use std::sync::Arc;

use mc_common::time::Timestamp;
use mc_server::{retrieval, ServerState};
use mc_storage::observations::NewObservation;
use mc_storage::Database;

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

struct Ctx {
    _dir: tempfile::TempDir,
    state: Arc<ServerState>,
}

fn ctx() -> Ctx {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let config = mc_config::load::load(&mc_config::load::LoadRequest::default()).unwrap();
    let state = Arc::new(ServerState::new(
        mc_config::ConfigHandle::new(config),
        db,
        "test-token".to_string(),
        Timestamp::from_millis(T0),
        dir.path().to_path_buf(),
    ));
    Ctx { _dir: dir, state }
}

/// 造一条活动（`verdict` 决定它是否被隐私拦截），返回活动 id。
fn add_activity(ctx: &Ctx, index: usize, title: &str, verdict: &str) -> String {
    let at = T0 + (index as i64) * 45 * 60_000;
    let observation = format!("obs-{index}");

    ctx.state
        .db
        .insert_observation(&NewObservation {
            id: observation.clone(),
            ts: Timestamp::from_millis(at),
            source_id: "screen".to_string(),
            kind: "screen".to_string(),
            app_name: Some("Visual Studio Code".to_string()),
            app_bundle_id: None,
            window_title: Some(title.to_string()),
            domain: None,
            display_id: None,
            scale_factor: None,
            image: None,
            text_content: None,
            text_origin: None,
            change_kind: "new".to_string(),
            privacy_verdict: verdict.to_string(),
            phash: None,
            idempotency: format!("key-{index}"),
        })
        .expect("观测必须能落库");

    mc_storage::projectors::activities::replay(
        ctx.state.db.as_ref(),
        &mc_domain::rules::RuleSet::default(),
        mc_domain::projector::ProjectionOptions::default(),
        Timestamp::from_millis(at + 60_000),
    )
    .expect("投影必须成功");

    mc_storage::projectors::activities::read_all(&ctx.state.db)
        .unwrap()
        .into_iter()
        .find(|activity| activity.evidence.contains(&observation))
        .map(|activity| activity.id)
        .expect("刚写入的活动必须能查到")
}

// 没有规则时活动标题退化为应用名，因此实体要靠「活动标题」这条链
// 就得先落一条带实体的标题 —— 用 `store` 直接放一条（夹具，与旧测试一致）。
fn set_title(ctx: &Ctx, activity_id: &str, title: &str) {
    let stored = mc_storage::projectors::activities::read_all(&ctx.state.db)
        .unwrap()
        .into_iter()
        .find(|activity| activity.id == activity_id)
        .expect("活动必须存在");

    let projection = mc_domain::projector::Projection {
        activities: vec![mc_domain::activity::ActivityView {
            id: stored.id.clone(),
            start: stored.start,
            end: stored.end,
            title: title.to_string(),
            original_title: title.to_string(),
            category: stored.category.clone(),
            observations: stored
                .observations
                .iter()
                .map(|id| mc_domain::activity::ObservationRef {
                    id: id.clone(),
                    at: stored.start,
                })
                .collect(),
            origin: stored.origin.clone(),
            confidence: stored.confidence,
            is_user_modified: false,
        }],
        noise: Vec::new(),
        ai_requests: 0,
        unknown_event_kinds: 0,
    };
    mc_storage::projectors::activities::store(
        ctx.state.db.as_ref(),
        &projection,
        0,
        Timestamp::from_millis(T0 + 3_600_000),
    )
    .expect("覆盖标题");
}

fn entity_count(ctx: &Ctx) -> usize {
    ctx.state.db.activity_entity_count().unwrap()
}

// ---------------------------------------------------------------- 同步

#[tokio::test]
async fn sync_persists_entities_extracted_from_activity_titles() {
    let ctx = ctx();
    let activity = add_activity(&ctx, 0, "修复 APEX-389", "allowed");
    set_title(&ctx, &activity, "修复 APEX-389 的验收标准");

    let report = retrieval::sync_activity_entities(&ctx.state.db).expect("同步必须成功");
    assert_eq!(report.linked, 1);
    assert_eq!(entity_count(&ctx), 1);

    let stored = ctx.state.db.entities_for_activity(&activity).unwrap();
    assert_eq!(stored[0].key, "APEX-389");
    assert_eq!(stored[0].display, "APEX-389");

    // 反查也能用（这是线索的数据来源）
    let activities = ctx
        .state
        .db
        .activities_for_entity("issue", "APEX-389")
        .unwrap();
    assert_eq!(activities.len(), 1);
    assert_eq!(activities[0].activity_id, activity);
}

#[tokio::test]
async fn sync_is_incremental_and_idempotent() {
    let ctx = ctx();
    let activity = add_activity(&ctx, 0, "修复 APEX-389", "allowed");
    set_title(&ctx, &activity, "修复 APEX-389");

    let first = retrieval::sync_activity_entities(&ctx.state.db).unwrap();
    assert_eq!(first.linked, 1);
    assert_eq!(first.unchanged, 0);

    let second = retrieval::sync_activity_entities(&ctx.state.db).unwrap();
    assert_eq!(second.linked, 0, "没有变化时不该重写");
    assert_eq!(second.unchanged, 1);
    assert_eq!(entity_count(&ctx), 1);
}

#[tokio::test]
async fn sync_removes_entities_that_are_no_longer_mentioned() {
    let ctx = ctx();
    let activity = add_activity(&ctx, 0, "修复 APEX-389", "allowed");
    set_title(&ctx, &activity, "修复 APEX-389");
    retrieval::sync_activity_entities(&ctx.state.db).unwrap();
    assert_eq!(entity_count(&ctx), 1);

    // 用户把标题改成了不含实体的内容
    set_title(&ctx, &activity, "上午在改代码");
    let report = retrieval::sync_activity_entities(&ctx.state.db).unwrap();

    assert_eq!(report.linked, 0);
    assert_eq!(report.cleared, 1, "不再提及的实体应当被记为「清理」");
    assert_eq!(entity_count(&ctx), 0, "不再提及的实体必须消失");
    assert_eq!(ctx.state.db.entity_count().unwrap(), 0, "孤儿实体也要清掉");
}

// ---------------------------------------------------------------- 隐私

/// 一条活动被判为拦截之后，它的实体关联必须被清掉 ——
/// 否则用户还能从「相关记录」里看到被拦截的内容。
#[tokio::test]
async fn sync_prunes_entities_of_blocked_activities() {
    let ctx = ctx();
    let activity = add_activity(&ctx, 0, "钱包助记词 APEX-389", "allowed");
    set_title(&ctx, &activity, "钱包助记词 APEX-389");
    retrieval::sync_activity_entities(&ctx.state.db).unwrap();
    assert_eq!(entity_count(&ctx), 1);

    // 隐私规则变化：这条活动的证据被标记为拦截
    ctx.state
        .db
        .with_write(|conn| {
            conn.execute(
                "UPDATE observations SET privacy_verdict = 'blocked' WHERE window_title LIKE '%助记词%'",
                [],
            )?;
            Ok(())
        })
        .expect("改隐私判定");

    let report = retrieval::sync_activity_entities(&ctx.state.db).unwrap();
    assert_eq!(report.cleared, 1, "被拦截活动的关联必须被清理");
    assert_eq!(entity_count(&ctx), 0);
    assert_eq!(ctx.state.db.entity_count().unwrap(), 0);
    assert!(
        ctx.state
            .db
            .activities_for_entity("issue", "APEX-389")
            .unwrap()
            .is_empty(),
        "被拦截的内容不得通过实体反查出来"
    );
}

// ---------------------------------------------------------------- 线索走落库

#[tokio::test]
async fn threads_are_built_from_persisted_links() {
    let ctx = ctx();
    let first = add_activity(&ctx, 0, "修复 APEX-389", "allowed");
    set_title(&ctx, &first, "修复 APEX-389");
    retrieval::sync_activity_entities(&ctx.state.db).unwrap();

    let threads = retrieval::threads(&ctx.state.db, "Asia/Shanghai", 10).unwrap();
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].entity.canonical, "APEX-389");
    assert_eq!(threads[0].activities.len(), 1);
}

/// 线索接口自己保证同步：即使 daemon 还没跑过一轮同步，也能拿到结果
#[tokio::test]
async fn threads_endpoint_syncs_entities_lazily() {
    let ctx = ctx();
    let activity = add_activity(&ctx, 0, "修复 APEX-389", "allowed");
    set_title(&ctx, &activity, "修复 APEX-389");

    assert_eq!(entity_count(&ctx), 0, "前置条件：还没有同步过");
    let threads = retrieval::threads(&ctx.state.db, "UTC", 10).unwrap();

    assert_eq!(threads.len(), 1);
    assert_eq!(entity_count(&ctx), 1, "查询时应当顺手把关联落库");
}

/// 被拦截的活动既不在文档集里、也没有关联行 → 线索里不会出现
#[tokio::test]
async fn threads_never_include_blocked_activities() {
    let ctx = ctx();
    let activity = add_activity(&ctx, 0, "私密 APEX-999", "blocked");
    set_title(&ctx, &activity, "私密 APEX-999");

    let threads = retrieval::threads(&ctx.state.db, "UTC", 10).unwrap();
    assert!(
        threads
            .iter()
            .all(|thread| thread.entity.canonical != "APEX-999"),
        "被拦截的活动不得出现在线索里"
    );
    assert_eq!(entity_count(&ctx), 0);
}
