//! 活动投影的后台任务。
//!
//! 采集链路只负责把观测写进事件日志；「这些观测合起来是在干什么」由这里算。
//! 之所以不做成「每次写观测顺手算一次」：
//! 聚合是**全量**纯函数（简单、不会留脏行），每来一条观测就重放整段历史
//! 会随着运行时间越跑越贵。定时 + 变更检测把这件事摊平成常数成本。

use std::path::Path;
use std::sync::Arc;

use mc_common::error::AppError;
use mc_common::observability::warn;
use mc_common::time::Timestamp;
use mc_domain::projector::{Projection, ProjectionOptions};
use mc_domain::rules::RuleSet;
use mc_pipeline::activity_ai::{
    infer_activities, ActivityAiPolicy, ActivityAiWorker, AiCall, AiCallResult, InferenceBatch,
};
use mc_pipeline::budget::{BudgetAction, BudgetPolicy, BudgetTracker};
use mc_pipeline::pools::{PoolConfig, Priority, TaskClass};
use mc_pipeline::supervisor::PoolSupervisor;
use mc_providers::credentials::{resolve_secret, KeychainCommand, SecretStore};
use mc_providers::openai::{EndpointConfig, OpenAiCompatibleProvider};
use mc_providers::transport::ReqwestTransport;
use mc_providers::Role;
use mc_storage::provider_calls::{CallResult, ProviderCall, Purpose};

use crate::state::ServerState;

/// 规则文件的约定位置。
///
/// 约定优于配置：用户把 `activities.yaml` 放进数据目录的 `rules/` 就会被加载，
/// 文件不存在则视为「没有规则」——系统仍然可用，只是活动退化成进程名兜底。
pub fn rules_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join("rules").join("activities.yaml")
}

/// 读取规则。文件不存在返回空规则集；文件**存在但写坏了**要报错 ——
/// 静默忽略会让用户以为自己写的规则生效了。
pub fn load_rules(data_dir: &Path) -> Result<RuleSet, AppError> {
    let path = rules_path(data_dir);
    if !path.exists() {
        return Ok(RuleSet::default());
    }

    let text = std::fs::read_to_string(&path).map_err(|error| {
        AppError::new(
            mc_common::error::ErrorCode::ConfigUnreadable,
            format!("无法读取规则文件 {}: {error}", path.display()),
        )
    })?;

    RuleSet::parse_yaml(&text).map_err(|message| {
        AppError::new(
            mc_common::error::ErrorCode::ConfigInvalid,
            format!("规则文件 {} 无法解析：{message}", path.display()),
        )
    })
}

/// 按配置组装 vision Provider。
///
/// 返回 `Ok(None)` 表示「不推断」而不是「出错」：没配模型、关了 AI、
/// 或者密钥还没填，都属于正常状态 —— 系统照常用规则和元数据工作。
pub fn build_vision_worker(
    config: &mc_config::Config,
    secrets: &dyn SecretStore,
) -> Result<Option<ActivityAiWorker>, AppError> {
    if !upload_allowed(config) {
        return Ok(None);
    }

    if !config.ai.enabled {
        return Ok(None);
    }

    let endpoint = &config.ai.vision;
    if endpoint.base_url.trim().is_empty() || endpoint.model.trim().is_empty() {
        return Ok(None);
    }

    // 密钥缺失只影响「能不能推断」，不该让 daemon 起不来
    let api_key = resolve_secret(secrets, endpoint.api_key_ref.as_deref())?;

    let provider = OpenAiCompatibleProvider::new(
        EndpointConfig {
            base_url: endpoint.base_url.clone(),
            model: endpoint.model.clone(),
            api_key,
            timeout: std::time::Duration::from_secs(endpoint.timeout_secs),
            max_image_edge: None,
            max_concurrency: endpoint.max_concurrency,
        },
        Arc::new(ReqwestTransport::default()),
        Role::Vision,
    )?;

    let policy = ActivityAiPolicy {
        ai_enabled: true,
        max_calls_per_window: config.ai.budget.max_vlm_calls_per_hour,
        window_secs: 3600,
        // 活动最长 2 小时，超过就该重新判断
        reanalyze_after_secs: config.activity.max_duration_secs,
    };

    Ok(Some(ActivityAiWorker::new(Arc::new(provider), policy)))
}

/// 单轮最多推断多少个活动。分批的意义见 `infer_once_with_limit` 里的注释。
pub const MAX_INFERENCES_PER_ROUND: usize = 50;

/// 读一条观测对应的图片（推断的输入）。
///
/// store 由调用方传进来复用：构造它内含 `create_dir_all`，每张图建一次纯属浪费
/// （积压几百个活动就是几百次）。
fn read_observation_image(
    store: &mc_storage::blob::FileSystemBlobStore,
    relative: &str,
) -> Option<(Vec<u8>, String)> {
    let bytes = store.read_relative(relative).ok()?;
    Some((bytes, mime_for(relative)))
}

fn mime_for(relative: &str) -> String {
    let lower = relative.to_ascii_lowercase();
    if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg".to_string()
    } else if lower.ends_with(".webp") {
        "image/webp".to_string()
    } else {
        "image/png".to_string()
    }
}

/// 对「规则没判出来」的活动做一次推断，并把结论作为事件写入日志。
///
/// 顺序不能反：**先写事件，再重投影**。推断结论和用户修正一样是事件，
/// 重放时会重新生效；直接改派生表的话，下次重算就丢了。
pub async fn infer_once(
    state: &ServerState,
    worker: &mut ActivityAiWorker,
    at: Timestamp,
) -> Result<InferenceBatch, AppError> {
    infer_once_with_limit(state, worker, at, MAX_INFERENCES_PER_ROUND).await
}

/// 与 [`infer_once`] 相同，只是单轮上限可传 —— 测试用它验证「分批」确实发生
/// （默认上限是策略常量，测试里造几百条活动代价太高）。
pub async fn infer_once_with_limit(
    state: &ServerState,
    worker: &mut ActivityAiWorker,
    at: Timestamp,
    limit: usize,
) -> Result<InferenceBatch, AppError> {
    infer_pending_in_range(state, worker, at, limit, None).await
}

/// 只推断**起始时间落在给定范围内**的活动（补偿作业用它重跑某一段历史）。
///
/// 与全量推断共用同一条代码路径（筛选、记账、写事件都一致），
/// 差别只有范围过滤 —— 否则「重跑上周」会悄悄用另一套行为。
pub async fn infer_range(
    state: &ServerState,
    worker: &mut ActivityAiWorker,
    at: Timestamp,
    from: Timestamp,
    to: Timestamp,
    limit: usize,
) -> Result<InferenceBatch, AppError> {
    infer_pending_in_range(state, worker, at, limit, Some((from, to))).await
}

async fn infer_pending_in_range(
    state: &ServerState,
    worker: &mut ActivityAiWorker,
    at: Timestamp,
    limit: usize,
    range: Option<(Timestamp, Timestamp)>,
) -> Result<InferenceBatch, AppError> {
    let stored = mc_storage::projectors::activities::read_all(&state.db)?;
    if stored.is_empty() {
        return Ok(InferenceBatch::default());
    }

    // 先筛出真正需要推断的，再去查图片路径：
    // 大量活动是规则命中或用户改过的，给它们查图片纯属白费。
    let pending: Vec<mc_domain::activity::ActivityView> = stored
        .iter()
        .map(view_from_stored)
        .filter(needs_inference)
        .filter(|activity| match range {
            Some((from, to)) => activity.start >= from && activity.start <= to,
            None => true,
        })
        .collect();
    if pending.is_empty() {
        return Ok(InferenceBatch::default());
    }

    // 单轮上限：积压几百个时一轮全做完会把 projector 循环占住（期间新数据不处理、
    // 界面上只看到「一直卡着」）。分批做，剩下的下一轮继续 —— 积压量本身在
    // 诊断包的 counts.pending_inference 里可见。
    let activities: Vec<mc_domain::activity::ActivityView> =
        pending.iter().take(limit.max(1)).cloned().collect();
    if pending.len() > activities.len() {
        warn!(
            component = "ai",
            event = "inference_capped",
            pending = pending.len(),
            this_round = activities.len(),
            "待推断活动较多，本轮先处理一批，其余下一轮继续"
        );
    }

    let ids: Vec<String> = activities
        .iter()
        .flat_map(|activity| activity.observation_ids())
        .collect();
    let mut paths: std::collections::HashMap<String, String> =
        state.db.image_paths(&ids)?.into_iter().collect();

    // 图片 store 建一次就够
    let store = mc_storage::blob::FileSystemBlobStore::new(
        state.data_dir.join("blobs"),
        mc_storage::blob::ImageFormat::Png,
    )
    .ok();

    // 借用问题：闭包里要用 state，因此先把需要的路径取出来
    let batch = {
        let store_ref = &store;
        let paths_ref = &mut paths;
        infer_activities(
            worker,
            &activities,
            |observation_id| {
                let relative = paths_ref.remove(observation_id)?;
                read_observation_image(store_ref.as_ref()?, &relative)
            },
            at,
        )
        .await?
    };

    // 先记账再写事件：写事件失败也不该让已经花掉的钱从报表上消失
    record_inference_calls(&state.db, &batch.calls, at)?;

    if batch.suggestions.is_empty() {
        return Ok(batch);
    }

    let events: Vec<mc_storage::NewEvent> = batch
        .suggestions
        .iter()
        .map(|suggestion| {
            let event = mc_domain::projector::DomainEvent::ActivityInferred {
                suggestion: suggestion.clone(),
            };
            mc_storage::NewEvent::new(event.kind(), at, event.payload()).by("ai")
        })
        .collect();
    state.db.append_events(&events)?;

    // 立刻重算，让接口上马上能看到推断结果
    let _ = project_once(state, at)?;
    Ok(batch)
}

/// 把一次推断的每一笔开销写进 `provider_calls`。
///
/// 记的是「调用」而不是「产出」：解析失败、超时、被闸门拦下都要留下记录，
/// 否则报表会显示「没花钱」，而实际上要么花了、要么因为没钱没干活。
pub fn record_inference_calls(
    db: &mc_storage::Database,
    calls: &[AiCall],
    at: Timestamp,
) -> Result<(), AppError> {
    for call in calls {
        let result = match call.result {
            AiCallResult::Ok => CallResult::Ok,
            AiCallResult::Error => CallResult::Error,
            AiCallResult::RateLimited => CallResult::RateLimited,
            AiCallResult::Timeout => CallResult::Timeout,
            AiCallResult::InvalidResponse => CallResult::InvalidResponse,
        };
        db.record_provider_call(&ProviderCall {
            at,
            provider_id: call.model.clone(),
            model: call.model.clone(),
            purpose: Purpose::Vision,
            observation_id: None,
            stage_id: None,
            prompt_tokens: call.prompt_tokens,
            completion_tokens: call.completion_tokens,
            latency_ms: call.latency_ms,
            result,
            error_code: call.error_code.clone(),
        })?;
    }
    Ok(())
}

/// 把本轮推断的开销回灌进预算表。
///
/// 只喂 token、不喂次数：次数闸门由 `ActivityAiPolicy` 自己按滑动窗口算，
/// 两边都记会重复计数。
pub fn feed_budget(tracker: &mut BudgetTracker, calls: &[AiCall], _at: Timestamp) {
    for call in calls {
        if call.prompt_tokens == 0 && call.completion_tokens == 0 {
            continue;
        }
        tracker.record_call(
            u64::from(call.prompt_tokens) + u64::from(call.completion_tokens),
            _at,
        );
    }
}

/// 按预算状态开关推断：超限就降级（不再调用模型），窗口滑走后自动恢复。
///
/// 只处理 `Degrade` 这一档：`Pause` / `Ask` 需要界面参与，当前一律不生效 ——
/// 假装处理会让用户以为有个开关在起作用。
pub fn sync_ai_with_budget(worker: &mut ActivityAiWorker, tracker: &BudgetTracker, at: Timestamp) {
    let state = tracker.state(at);
    worker.set_ai_enabled(!state.exceeded);
    if state.exceeded {
        warn!(
            component = "budget",
            event = "gate_closed",
            reason = state.reason.as_deref().unwrap_or("unknown"),
            "模型预算已用尽，暂停推断（活动照常产出）"
        );
    }
}

/// 从配置里组装预算表。
pub async fn infer_pending(state: &ServerState, at: Timestamp) -> Result<InferenceBatch, AppError> {
    let secrets = KeychainCommand::default();
    let worker = build_vision_worker(&state.config.current().config, &secrets)?;
    let Some(mut worker) = worker else {
        return Ok(InferenceBatch::default());
    };
    infer_once(state, &mut worker, at).await
}

/// 只有「规则没判出来」且「用户没改过」的活动才值得问模型。
fn needs_inference(activity: &mc_domain::activity::ActivityView) -> bool {
    matches!(activity.origin, mc_domain::activity::Provenance::Observed)
        && !activity.is_user_modified
}

fn view_from_stored(
    stored: &mc_storage::projectors::activities::StoredActivity,
) -> mc_domain::activity::ActivityView {
    mc_domain::activity::ActivityView {
        id: stored.id.clone(),
        start: stored.start,
        end: stored.end,
        title: stored.title.clone(),
        original_title: stored
            .original_title
            .clone()
            .unwrap_or_else(|| stored.title.clone()),
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
        is_user_modified: stored.is_user_modified(),
    }
}

/// 配置变了（例如用户刚填了模型）时重建总结来源。
///
/// 只在真的补出过总结时才检查，避免每轮都读一遍配置。
fn rebuild_summary_source(
    state: &ServerState,
    secrets: &KeychainCommand,
    current: Box<dyn mc_summary::source::SummarySource>,
) -> Box<dyn mc_summary::source::SummarySource> {
    match crate::summary::build_summary_generator(&state.config.current().config, secrets) {
        Ok(Some(generator)) => Box::new(generator),
        _ => current,
    }
}

/// 投影一次（仅当日志有新内容）。
pub fn project_once(state: &ServerState, at: Timestamp) -> Result<Option<Projection>, AppError> {
    let config = state.config.current();
    let rules = load_rules(&state.data_dir)?;
    let options = ProjectionOptions {
        policy: config.config.activity.aggregation_policy(),
        // 推断调用属于 AI 路径，投影本身不发起任何模型调用
        allow_inference: false,
    };

    mc_storage::projectors::activities::project_if_dirty(&state.db, &rules, options, at)
}

/// 从配置里组装预算表（`ai.budget.*`）。
fn budget_tracker(state: &ServerState) -> BudgetTracker {
    let budget = &state.config.current().config.ai.budget;
    BudgetTracker::new(BudgetPolicy {
        max_vlm_calls_per_hour: budget.max_vlm_calls_per_hour,
        max_tokens_per_hour: budget.max_tokens_per_hour,
        max_tokens_per_day: budget.max_tokens_per_day,
        // 配置层的枚举与执行层的枚举分开：显式映射，避免新增档位时静默错配
        on_exceeded: match budget.on_exceeded {
            mc_config::model::BudgetAction::Degrade => BudgetAction::Degrade,
            mc_config::model::BudgetAction::Pause => BudgetAction::Pause,
            mc_config::model::BudgetAction::Ask => BudgetAction::Ask,
        },
    })
}

/// 把投影失败记进 `pipeline_failures`，让它出现在诊断页上。
pub(crate) fn record_failure(
    state: &ServerState,
    error: &AppError,
    at: Timestamp,
) -> Result<(), AppError> {
    state
        .db
        .record_failure(at, "activities", error, "warn")
        .map(|_| ())
}

/// 起一个后台循环：投影 + 对「规则没判出来」的活动做一次推断。
///
/// **Provider 只组装一次**，在循环外：滑动窗口限流与「同一活动窗口不重复分析」
/// 都依赖 worker 的跨轮次状态，每轮重建等于把两道闸门都废掉。
///
/// 返回 `JoinHandle`：调用方（daemon）可以在关闭时 abort。
pub fn spawn_projector(state: Arc<ServerState>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let secrets = KeychainCommand::default();

        // 未配置模型 / 关了 AI / 密钥没填 → 没有 worker，循环照常投影
        let worker = match build_vision_worker(&state.config.current().config, &secrets) {
            Ok(worker) => worker,
            Err(error) => {
                let at = mc_common::time::Clock::now(&mc_common::time::SystemClock);
                let _ = record_failure(&state, &error, at);
                None
            }
        };

        // 总结来源：配了 chat 模型就用模型，否则用确定性兜底。
        // **两者都不能是「不生成总结」** —— 那正是最坏的结果。
        let inner_summary: Box<dyn mc_summary::source::SummarySource> =
            match crate::summary::build_summary_generator(&state.config.current().config, &secrets)
            {
                Ok(Some(generator)) => Box::new(generator),
                Ok(None) => Box::new(crate::stages::fallback_generator()),
                Err(error) => {
                    let at = mc_common::time::Clock::now(&mc_common::time::SystemClock);
                    let _ = record_failure(&state, &error, at);
                    Box::new(crate::stages::fallback_generator())
                }
            };
        // 记账包一层：阶段收尾、日报、巡检补写都经这条路径
        let summary: Box<dyn mc_summary::source::SummarySource> = Box::new(
            crate::summary::AccountedSummarySource::new(inner_summary, Arc::clone(&state.db)),
        );

        // 两件有状态的执行体放在异步锁里：作业要跨越 await 修改它们
        // （推断 worker 的滑窗与缓存、总结来源的可替换性）
        let worker = Arc::new(tokio::sync::Mutex::new(worker));
        let summary = Arc::new(tokio::sync::Mutex::new(summary));
        // 预算也要跨轮次：滑窗状态每轮重建等于把闸门废掉
        let budget = Arc::new(tokio::sync::Mutex::new(budget_tracker(&state)));

        // 记住上一次 tick 时所在的本地日：翻页时要补一份「昨天」的日报
        let mut last_day = mc_common::time::Clock::now(&mc_common::time::SystemClock)
            .to_local_date(
                &state
                    .config
                    .current()
                    .config
                    .general
                    .timezone
                    .clone()
                    .unwrap_or_else(|| "UTC".to_string()),
            )
            .ok();

        // 两个池：vision 与 summary 各有专属容量，全局闸门共享。
        // 这样图片积压不会把总结饿死 —— 两个池各自限流是这条性质的落点。
        let config = state.config.current().config.clone();
        let mut supervisor = PoolSupervisor::new(PoolConfig {
            vision_capacity: config.ai.vision.max_concurrency as usize,
            summary_capacity: config.ai.chat.max_concurrency as usize,
            global_provider_limit: (config.ai.vision.max_concurrency
                + config.ai.chat.max_concurrency) as usize,
            global_reserved_for_summary: 1,
            interactive_reserved_summary_slots: 1,
            aging_after_secs: 30,
        });

        loop {
            let seconds = state
                .config
                .current()
                .config
                .activity
                .project_tick_secs
                .max(1);
            tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;

            let at = mc_common::time::Clock::now(&mc_common::time::SystemClock);

            match project_once(&state, at) {
                Ok(Some(_projection)) => {}
                Ok(None) => {}
                Err(error) => {
                    // 失败写进 `pipeline_failures` —— `/api/diagnostics` 会把它们列出来。
                    // 只打日志等于没人看得见，诊断页必须列出它们。
                    // 循环**不能**退出：一次规则文件写坏不该让活动永久停更。
                    let _ = record_failure(&state, &error, at);
                }
            }

            // 推断与总结提交给**两个池**，由同一套闸门限流。
            // 它们因此是并发的：总结不必等一轮图片推断跑完。
            let vision_state = Arc::clone(&worker);
            let vision_budget = Arc::clone(&budget);
            let vision_server = Arc::clone(&state);
            supervisor.submit(
                format!("infer-{}", at.as_millis()),
                TaskClass::Vision,
                Priority::Backfill,
                1,
                move || async move {
                    let mut guard = vision_state.lock().await;
                    if let Some(worker) = guard.as_mut() {
                        // 先按预算开关闸门，再跑推断；跑完把这一轮的花销记进预算
                        let mut tracker = vision_budget.lock().await;
                        sync_ai_with_budget(worker, &tracker, at);
                        match infer_once(&vision_server, worker, at).await {
                            Ok(batch) => feed_budget(&mut tracker, &batch.calls, at),
                            Err(error) => {
                                let _ = record_failure(&vision_server, &error, at);
                            }
                        }
                    }
                },
            );

            let summary_state = Arc::clone(&summary);
            let summary_secrets = KeychainCommand::default();
            let summary_server = Arc::clone(&state);
            supervisor.submit(
                format!("summary-{}", at.as_millis()),
                TaskClass::Summary,
                Priority::StageSummary,
                1,
                move || async move {
                    let mut guard = summary_state.lock().await;
                    match crate::stages::tick(&summary_server, guard.as_ref(), at).await {
                        Ok(report) => {
                            if report.repaired > 0 {
                                // 配置可能在运行中变了（用户刚填了模型）——
                                // 只有真的补出过总结时才重建，避免每轮读一遍配置。
                                let previous = std::mem::replace(
                                    &mut *guard,
                                    Box::new(crate::stages::fallback_generator()),
                                );
                                *guard = rebuild_summary_source(
                                    &summary_server,
                                    &summary_secrets,
                                    previous,
                                );
                            }
                        }
                        Err(error) => {
                            let _ = record_failure(&summary_server, &error, at);
                        }
                    }
                },
            );

            supervisor.run_until_idle().await;

            // 本地日翻页 → 给昨天补一份日报。
            // 放在作业跑完之后：日报要能看到这一轮刚补出来的总结。
            let timezone = state
                .config
                .current()
                .config
                .general
                .timezone
                .clone()
                .unwrap_or_else(|| "UTC".to_string());
            if let Ok(today) = at.to_local_date(&timezone) {
                if let Some(previous) = last_day.filter(|day| *day < today) {
                    let guard = summary.lock().await;
                    if let Err(error) =
                        crate::stages::roll_over_day(&state, guard.as_ref(), at, previous).await
                    {
                        let _ = record_failure(&state, &error, at);
                    }
                }
                last_day = Some(today);
            }
        }
    })
}

/// 是否允许把内容发到模型服务。
///
/// **默认拒绝**：`privacy.ai_upload` 默认 `false`，未显式同意时任何出网路径都
/// 不组装 provider —— 这样「默认不出网」是物理事实，而不是一句文档承诺。
pub fn upload_allowed(config: &mc_config::Config) -> bool {
    config.privacy.ai_upload
}
