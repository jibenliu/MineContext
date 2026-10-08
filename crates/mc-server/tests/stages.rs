//! 阶段投影、崩溃恢复与强制 flush。
//!
//! 状态机是纯函数，但**只有接进投影循环它才会真的产出阶段**。
//! 这里覆盖三件事：
//! 1. 从活动事件重放出阶段（确定性：同一份历史重放两次结果一致）；
//! 2. 崩溃恢复：上次没关掉的阶段要被补记为 `interrupted`，并进入总结队列；
//! 3. 强制 flush：关机时**不等待模型**，直接用确定性兜底总结收尾，
//!    退出路径必须有时间上界，否则用户会遇到「关不掉」。
//!
//! 用真数据库 + 真投影链（这些行为只在「事件 → 活动 → 阶段 → 总结」串起来后才成立）。

use std::sync::Arc;

use mc_common::time::Timestamp;
use mc_server::{router, ServerState};
use mc_storage::projectors::stages::StageRow;
use mc_storage::Database;
use mc_summary::patrol::PatrolPolicy;

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

fn at(offset_secs: i64) -> Timestamp {
    Timestamp::from_millis(T0 + offset_secs * 1_000)
}

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
        at(0),
        dir.path().to_path_buf(),
    ));
    Ctx { _dir: dir, state }
}

/// 往事件日志里写一条观测（采集链路的真实形状），随后由投影产出活动。
fn observe(state: &ServerState, id: &str, offset_secs: i64, app: &str, title: &str) {
    state
        .db
        .insert_observation(&mc_storage::observations::NewObservation {
            id: id.to_string(),
            ts: at(offset_secs),
            source_id: "macos:screen".to_string(),
            kind: "screen".to_string(),
            app_name: Some(app.to_string()),
            app_bundle_id: None,
            window_title: Some(title.to_string()),
            domain: None,
            display_id: None,
            scale_factor: None,
            image: None,
            text_content: None,
            text_origin: None,
            change_kind: "pixel_major".to_string(),
            privacy_verdict: "allowed".to_string(),
            phash: None,
            idempotency: format!("idem-{id}"),
        })
        .expect("写入观测");
}

fn lock(state: &ServerState, offset_secs: i64) {
    state
        .db
        .append_events(&[mc_storage::NewEvent::new(
            mc_domain::projector::KIND_SCREEN_LOCKED,
            at(offset_secs),
            serde_json::json!({ "at": at(offset_secs).as_millis() }),
        )
        .by("system")])
        .expect("写入锁屏事件");
}

/// 测试用的短周期策略：稳定 10 秒、宽限 30 秒、空闲 60 秒、上限 30 分钟。
fn policy() -> mc_domain::stage::StagePolicy {
    mc_domain::stage::StagePolicy {
        min_duration_secs: 60,
        max_duration_secs: 1800,
        switch_grace_secs: 30,
        idle_threshold_secs: 60,
        min_activity_stable_secs: 10,
        summary_deadline_secs: 30,
        timezone: "UTC".to_string(),
    }
}

fn project_and_rebuild(state: &ServerState, now: Timestamp) -> Vec<StageRow> {
    mc_server::activities::project_once(state, now).expect("投影活动");
    mc_server::stages::rebuild(state, &policy(), now).expect("重建阶段");
    state.db.read_stages().expect("读阶段")
}

// 4.19 的前置：活动接上阶段
#[test]
fn stages_are_rebuilt_from_activities() {
    let ctx = ctx();
    for step in 0..6 {
        observe(
            &ctx.state,
            &format!("obs-{step}"),
            step * 20,
            "VSCode",
            "main.rs",
        );
    }

    // 收尾时刻只比最后一条观测晚 20 秒（< idle 60 秒）→ 阶段仍然开着
    let stages = project_and_rebuild(&ctx.state, at(120));

    assert_eq!(stages.len(), 1, "{stages:?}");
    assert_eq!(stages[0].id, "stage-act-obs-0");
    assert_eq!(stages[0].state, "open", "还在进行中的阶段不该被关闭");
    assert!(!stages[0].activities.is_empty());
}

// 4.14 在投影层的对应：重建必须确定性
#[test]
fn stage_rebuild_is_deterministic() {
    let ctx = ctx();
    observe(&ctx.state, "obs-1", 0, "VSCode", "main.rs");
    observe(&ctx.state, "obs-2", 20, "VSCode", "main.rs");
    observe(&ctx.state, "obs-3", 300, "Chrome", "Hacker News");
    observe(&ctx.state, "obs-4", 320, "Chrome", "Hacker News");

    let first = project_and_rebuild(&ctx.state, at(400));
    let second = project_and_rebuild(&ctx.state, at(400));

    assert_eq!(first, second, "同一份历史重放两次必须得到完全一样的阶段");
}

// 空闲与锁屏都会关阶段，且原因区分得开
#[test]
fn idle_and_lock_close_stages_with_distinct_reasons() {
    let ctx = ctx();
    // 第一段：写代码 → 空闲 5 分钟
    observe(&ctx.state, "obs-1", 0, "VSCode", "main.rs");
    observe(&ctx.state, "obs-2", 30, "VSCode", "main.rs");
    // 第二段：看网页 → 锁屏。
    // 锁屏时刻必须落在空闲阈值**之内**（否则空闲会先结束阶段，
    // 那是正确行为，但测不到锁屏这一条路径）。
    observe(&ctx.state, "obs-3", 600, "Chrome", "Hacker News");
    observe(&ctx.state, "obs-4", 630, "Chrome", "Hacker News");
    lock(&ctx.state, 660);

    let stages = project_and_rebuild(&ctx.state, at(900));

    assert_eq!(stages.len(), 2, "{stages:?}");
    assert_eq!(stages[0].state, "closed");
    assert_eq!(
        stages[0].end_reason.as_deref(),
        Some("idle"),
        "第一段是空闲结束：{stages:?}"
    );
    assert_eq!(
        stages[1].end_reason.as_deref(),
        Some("locked"),
        "第二段是锁屏结束 —— 与「空闲离开」必须能区分：{stages:?}"
    );
}

#[test]
fn restart_marks_the_previous_open_stage_as_interrupted() {
    let ctx = ctx();
    observe(&ctx.state, "obs-1", 0, "VSCode", "main.rs");
    observe(&ctx.state, "obs-2", 30, "VSCode", "main.rs");

    let stages = project_and_rebuild(&ctx.state, at(60));
    assert_eq!(stages[0].state, "open");

    // 模拟 kill -9 之后重启：进程重启时先把上次留下的 open 阶段补记为中断
    let recovered = mc_server::stages::recover_interrupted(&ctx.state, at(120)).expect("恢复");
    assert_eq!(recovered, 1, "上次留下的 open 阶段要被补记");

    let stages = ctx.state.db.read_stages().expect("读阶段");
    assert_eq!(stages[0].state, "closed");
    assert_eq!(stages[0].end_reason.as_deref(), Some("interrupted"));
    assert_eq!(stages[0].end, Some(at(120)));

    // 补记之后必须能被巡检补上总结（否则「有阶段必有总结」就漏了一个）
    let missing = ctx
        .state
        .db
        .stages_missing_summary(at(200), 0)
        .expect("查询");
    assert_eq!(missing.len(), 1, "{missing:?}");
}

#[test]
fn shutdown_flush_closes_the_stage_and_writes_a_summary_within_bounds() {
    let ctx = ctx();
    observe(&ctx.state, "obs-1", 0, "VSCode", "main.rs");
    observe(&ctx.state, "obs-2", 30, "VSCode", "main.rs");
    let stages = project_and_rebuild(&ctx.state, at(60));
    assert_eq!(stages[0].state, "open");

    let started = std::time::Instant::now();
    let summary_id = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(mc_server::stages::flush_on_shutdown(
            &ctx.state,
            &policy(),
            at(90),
        ))
        .expect("flush 不该失败")
        .expect("应当产出一条总结");

    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "关机路径必须有时间上界（4.43），实际 {:?}",
        started.elapsed()
    );

    let stages = ctx.state.db.read_stages().expect("读阶段");
    assert_eq!(stages[0].state, "closed");
    assert_eq!(stages[0].end_reason.as_deref(), Some("shutdown"));

    let summaries = ctx.state.db.read_summaries(None, None).expect("读总结");
    assert_eq!(summaries.len(), 1, "{summaries:?}");
    assert_eq!(summaries[0].id, summary_id);
    assert_eq!(
        summaries[0].quality, "fallback",
        "关机路径不等模型：确定性兜底是唯一有时间保证的做法"
    );
    assert!(!summaries[0].body_markdown.trim().is_empty());
    assert_eq!(
        summaries[0].stage_id.as_deref(),
        Some(stages[0].id.as_str())
    );
}

// 没有可用模型时 flush 同样必须立刻返回
#[test]
fn flush_is_bounded_even_without_a_model() {
    let ctx = ctx();
    // 只改配置：没有 base_url → 不会构造任何 Provider
    observe(&ctx.state, "obs-1", 0, "VSCode", "main.rs");
    observe(&ctx.state, "obs-2", 30, "VSCode", "main.rs");
    project_and_rebuild(&ctx.state, at(60));

    let started = std::time::Instant::now();
    let outcome = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(mc_server::stages::flush_on_shutdown(
            &ctx.state,
            &policy(),
            at(90),
        ))
        .expect("没有模型也要能收尾");
    let elapsed = started.elapsed();

    assert!(outcome.is_some(), "没有模型更要用兜底把总结写出来");
    assert!(elapsed < std::time::Duration::from_secs(2), "{elapsed:?}");
}

// flush 之后巡检不应该再补一条（幂等）
#[test]
fn flush_then_patrol_does_not_duplicate_summaries() {
    let ctx = ctx();
    observe(&ctx.state, "obs-1", 0, "VSCode", "main.rs");
    observe(&ctx.state, "obs-2", 30, "VSCode", "main.rs");
    project_and_rebuild(&ctx.state, at(60));

    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime
        .block_on(mc_server::stages::flush_on_shutdown(
            &ctx.state,
            &policy(),
            at(90),
        ))
        .expect("flush");

    // 巡检用同一个配置（无模型 → 兜底），deadline 设 0 表示立刻可补
    let report = runtime
        .block_on(mc_summary::patrol::patrol_once(
            &ctx.state.db,
            &mc_server::stages::fallback_generator(),
            &PatrolPolicy {
                deadline_secs: 0,
                min_stage_duration_secs: 0,
                timezone: "UTC".to_string(),
            },
            at(200),
        ))
        .expect("巡检");

    assert!(
        report.repaired.is_empty(),
        "已经有总结就不该再补：{report:?}"
    );
    assert_eq!(ctx.state.db.summary_count().expect("总结数"), 1);
}

/// 阶段里活动的顺序必须与时间一致（总结按这个顺序讲「做了什么」）
#[test]
fn stage_activity_order_follows_time() {
    let ctx = ctx();
    observe(&ctx.state, "obs-1", 0, "VSCode", "main.rs");
    observe(&ctx.state, "obs-2", 30, "VSCode", "main.rs");
    observe(&ctx.state, "obs-3", 120, "Chrome", "Hacker News");
    observe(&ctx.state, "obs-4", 150, "Chrome", "Hacker News");
    observe(&ctx.state, "obs-5", 240, "Chrome", "Hacker News");

    let stages = project_and_rebuild(&ctx.state, at(300));
    let closed: Vec<&StageRow> = stages.iter().filter(|stage| stage.is_closed()).collect();
    assert!(!closed.is_empty(), "{stages:?}");

    for stage in closed {
        assert!(
            !stage.activities.is_empty(),
            "每个阶段都要记下自己包含哪些活动：{stage:?}"
        );
    }
}

/// 接口层能读到阶段（4.48 的前置：路由还没做，但读路径要通）
#[test]
fn stages_are_readable_through_the_state() {
    let ctx = ctx();
    observe(&ctx.state, "obs-1", 0, "VSCode", "main.rs");
    observe(&ctx.state, "obs-2", 30, "VSCode", "main.rs");
    project_and_rebuild(&ctx.state, at(60));

    let _router = router(Arc::clone(&ctx.state));
    let stages = ctx.state.db.read_stages().expect("读阶段");
    assert_eq!(stages.len(), 1);
    assert_eq!(stages[0].day, "2026-09-30");
}

// ---------------------------------------------------------------- 合成入口

/// `tick` 必须同时做两件事：重建阶段 + 巡检补总结。
///
/// 合起来是因为「重建了阶段却没跑巡检」出现一次，
/// 就意味着一个阶段永远没有总结。
#[test]
fn stage_tick_rebuilds_and_repairs_in_one_call() {
    let ctx = ctx();
    // 第一段：写代码 30 分钟（配置里的 min_duration 是 15 分钟，因此够格产出总结）
    for step in 0..31 {
        observe(
            &ctx.state,
            &format!("obs-work-{step}"),
            step * 60,
            "VSCode",
            "main.rs",
        );
    }
    // 第二段：一小时后看网页
    observe(&ctx.state, "obs-web-1", 5400, "Chrome", "Hacker News");
    observe(&ctx.state, "obs-web-2", 5460, "Chrome", "Hacker News");

    // 阶段读的是**投影后的活动**，因此必须先投影（daemon 的顺序也是
    // 投影 → 推断 → 阶段；推断在中间是为了让阶段用上推断更新过的标题）
    mc_server::activities::project_once(&ctx.state, at(6000)).expect("投影活动");

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let source = mc_server::stages::fallback_generator();

    // 第一段早就关闭并且超过 deadline，第二段还在进行中
    let report = runtime
        .block_on(mc_server::stages::tick(&ctx.state, &source, at(6000)))
        .expect("tick");

    assert_eq!(report.stages, 2, "应当重建出两个阶段：{report:?}");
    assert!(
        report.repaired >= 1,
        "已关闭且超过 deadline 的阶段必须在这一轮里被补上总结：{report:?}"
    );
    assert_eq!(
        ctx.state
            .db
            .stages_without_summary_count(900)
            .expect("哨兵指标"),
        0,
        "tick 结束后「该有总结却没有」必须归零"
    );
    assert_eq!(ctx.state.db.summary_count().expect("总结数"), 1);
}

// ---------------------------------------------------------------- 日报归档（5.22 的接线）

/// 翻页时不只是产出日报，还要**归档进笔记树** ——
/// 否则日报只存在于 summaries 表里，用户在笔记树里根本看不到。
#[test]
fn day_rollover_archives_the_report_into_the_vault() {
    let ctx = ctx();
    // 本地（UTC）09-30 上午的一段活动
    observe(&ctx.state, "obs-1", 0, "VSCode", "main.rs");
    observe(&ctx.state, "obs-2", 30, "VSCode", "main.rs");
    mc_server::activities::project_once(&ctx.state, at(60)).expect("投影");

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let source = mc_server::stages::fallback_generator();
    let day = chrono::NaiveDate::from_ymd_opt(2026, 9, 30).unwrap();

    let id = runtime
        .block_on(mc_server::stages::roll_over_day(
            &ctx.state,
            &source,
            at(3600),
            day,
        ))
        .expect("翻页")
        .expect("有内容的一天要产出日报");
    assert_eq!(id, "sum-daily-2026-09-30");

    // 渲染层的查询：document_type IN ('DailyReport', 'vaults')
    let documents = ctx
        .state
        .db
        .read_vault_documents(&[
            mc_storage::vaults::DOCUMENT_TYPE_DAILY_REPORT,
            mc_storage::vaults::DOCUMENT_TYPE_VAULTS,
        ])
        .expect("读笔记树");

    let report = documents
        .iter()
        .find(|doc| doc.title == "2026-09-30 日报")
        .expect("日报必须出现在笔记树里");
    assert!(
        report
            .content
            .as_deref()
            .is_some_and(|content| !content.trim().is_empty()),
        "正文不能为空：{report:?}"
    );
    assert!(
        documents
            .iter()
            .any(|doc| doc.is_folder && doc.title == mc_storage::vaults::FOLDER_SUMMARY),
        "要挂在 Summary 文件夹下：{documents:?}"
    );
}
