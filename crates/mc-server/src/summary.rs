//! 总结 Provider 的组装（与 vision 的组装同构）。
//!
//! 没配 chat 模型时返回 `Ok(None)` —— 调用方改用确定性兜底，
//! **而不是跳过总结**：用户没配模型不等于「这段时间没有总结」。

use mc_common::error::AppError;
use mc_common::time::Timestamp;
use mc_providers::credentials::{resolve_secret, SecretStore};
use mc_providers::openai::{EndpointConfig, OpenAiCompatibleProvider};
use mc_providers::transport::ReqwestTransport;
use mc_providers::{ChatProvider, Role};
use mc_storage::provider_calls::{CallResult, ProviderCall, Purpose};
use mc_storage::Database;
use mc_summary::generator::SummaryGenerator;
use mc_summary::model::SummaryLocale;
use mc_summary::template::SummaryTemplate;

use std::sync::Arc;

pub fn build_summary_generator(
    config: &mc_config::Config,
    secrets: &dyn SecretStore,
) -> Result<Option<SummaryGenerator>, AppError> {
    // 未同意出网时不组装 provider：总结退化为确定性兜底
    // （`build_summary_generator` 返回 None 时调用方会使用 fallback generator）
    if !crate::activities::upload_allowed(config) {
        return Ok(None);
    }

    if !config.ai.enabled {
        return Ok(None);
    }

    // 总结是长文本任务，用 chat 端点（vision 端点通常更贵也更慢）
    let endpoint = &config.ai.chat;
    if endpoint.base_url.trim().is_empty() || endpoint.model.trim().is_empty() {
        return Ok(None);
    }

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
        Role::Chat,
    )?;

    let locale = SummaryLocale::from_config(&config.general.locale);

    // 脱敏规则非法时**不组装 provider**：静默放行等于用户以为自己脱敏了、
    // 实际把活动标题与正文原文发了出去。
    let redactor = match mc_common::redact::Redactor::new(&config.privacy.redact_patterns) {
        Ok(redactor) => redactor,
        Err(_) => return Ok(None),
    };
    Ok(Some(
        SummaryGenerator::new(
            Arc::new(provider) as Arc<dyn ChatProvider>,
            SummaryTemplate::default_work_stage(),
            locale,
        )
        .with_redactor(redactor),
    ))
}

/// 请求路径使用的总结来源：配了模型就用模型，否则用确定性兜底。
///
/// 与后台循环里的那份是**两个实例**（配置在运行中可能变化），
/// 但语义完全一致：任何情况下都拿得到一份总结。
pub fn source_for(state: &crate::state::ServerState) -> Box<dyn mc_summary::source::SummarySource> {
    source_for_with_template(state, SummaryTemplate::default_work_stage())
}

/// 用指定模板构造生成源：配置或请求解析出的模板从这里真正影响产出。
pub fn source_for_with_template(
    state: &crate::state::ServerState,
    template: SummaryTemplate,
) -> Box<dyn mc_summary::source::SummarySource> {
    let secrets = mc_providers::credentials::KeychainCommand::default();
    let inner: Box<dyn mc_summary::source::SummarySource> =
        match build_summary_generator(&state.config.current().config, &secrets) {
            Ok(Some(generator)) => Box::new(generator.with_template(template.clone())),
            _ => Box::new(crate::stages::fallback_generator_with(template)),
        };
    Box::new(AccountedSummarySource::new(inner, Arc::clone(&state.db)))
}

/// 把一次模型总结的开销写进 `provider_calls`。
///
/// 兜底总结（`model` 为空）**不写**：它没有花模型的钱，写进去会让报表
/// 出现「有产出就有成本」的假象。`latency_ms` 由调用方**实测**传入：记 0 会让
/// 诊断里 Chat 组的平均延迟永远是 0，排查「为什么这么慢」时没有依据。
pub fn record_summary_usage(
    db: &Database,
    model: Option<&str>,
    prompt_tokens: u32,
    completion_tokens: u32,
    latency_ms: u64,
    at: Timestamp,
) -> Result<(), AppError> {
    let Some(model) = model.filter(|value| !value.is_empty()) else {
        return Ok(());
    };
    db.record_provider_call(&ProviderCall {
        at,
        provider_id: model.to_string(),
        model: model.to_string(),
        purpose: Purpose::Chat,
        observation_id: None,
        stage_id: None,
        prompt_tokens,
        completion_tokens,
        latency_ms,
        result: CallResult::Ok,
        error_code: None,
    })?;
    Ok(())
}

/// 给总结来源套一层记账：所有调用点（阶段收尾、日报、任意时段、巡检补写）
/// 都走 [`SummarySource::generate`]，因此包一层就全覆盖。
pub struct AccountedSummarySource {
    inner: Box<dyn mc_summary::source::SummarySource>,
    db: Arc<Database>,
}

impl AccountedSummarySource {
    pub fn new(inner: Box<dyn mc_summary::source::SummarySource>, db: Arc<Database>) -> Self {
        Self { inner, db }
    }
}

#[async_trait::async_trait]
impl mc_summary::source::SummarySource for AccountedSummarySource {
    async fn generate(
        &self,
        input: &mc_summary::model::StageSummaryInput,
    ) -> mc_summary::generator::GenerateOutcome {
        // 实测耗时：不记录的话 Chat 组的平均延迟永远是 0 ——
        // 排查「为什么这么慢」时最缺的就是这个数。
        let started = std::time::Instant::now();
        let outcome = self.inner.generate(input).await;
        let elapsed_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        if let mc_summary::generator::GenerateOutcome::Model { summary } = &outcome {
            let _ = record_summary_usage(
                &self.db,
                summary.model.as_deref(),
                summary.prompt_tokens,
                summary.completion_tokens,
                elapsed_ms,
                mc_common::time::Clock::now(&mc_common::time::SystemClock),
            );
        }
        outcome
    }

    fn template_id(&self) -> String {
        self.inner.template_id()
    }
}
