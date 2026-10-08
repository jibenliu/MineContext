//! **「默认不出网」必须是代码事实，不是文档承诺**。
//!
//! `privacy.ai_upload` 默认 `false`，文档也写着「安装后只做本地采集与规则识别，不出网」。
//! 但只要用户填了模型端点，内容就可能被发出去 —— 这类缺陷不会让测试变红，也不会报错，
//! 只会在用户以为自己没开联网时把屏幕内容交给第三方。因此要求是：
//! 1. `ai_upload = false` → 三个出网路径（视觉推断 / 总结 / 对话）**都不组装 provider**；
//! 2. 只有显式 `true` 且端点配置齐全时才组装；
//! 3. 规则引擎异常时按最保守处理（fail-closed）。

use std::sync::Arc;

use mc_common::time::Timestamp;
use mc_providers::credentials::StaticSecretStore;

/// 测试用的密钥库：`ai_key_ref = "env:MC_TEST_KEY"` 会命中这里。
fn secrets() -> StaticSecretStore {
    StaticSecretStore::new(
        [("env:MC_TEST_KEY".to_string(), "sk-test".to_string())]
            .into_iter()
            .collect(),
    )
}
use mc_server::{activities, chat, summary, ServerState};
use mc_storage::Database;
use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;
use tower::ServiceExt;

const TOKEN: &str = "test-token";

/// 配置：模型端点齐全，`ai_upload` 由参数决定。
fn load_config(ai_upload: bool) -> mc_config::Config {
    let toml = format!(
        r#"
        [ai]
        enabled = true

        [ai.vision]
        base_url = "https://api.example.com/v1"
        model = "qwen3-vl"
        api_key_ref = "env:MC_TEST_KEY"

        [ai.chat]
        base_url = "https://api.example.com/v1"
        model = "qwen3-max"
        api_key_ref = "env:MC_TEST_KEY"

        [privacy]
        ai_upload = {ai_upload}
        "#
    );
    mc_config::load::load(&mc_config::load::LoadRequest {
        layers: vec![mc_config::load::LayerSource::Inline {
            name: "test".to_string(),
            toml,
        }],
        env: Vec::new(),
        read_process_env: false,
    })
    .expect("配置必须可加载")
    .config
}

fn state(config: mc_config::Config) -> (tempfile::TempDir, Arc<ServerState>) {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let state = Arc::new(ServerState::new(
        mc_config::ConfigHandle::new(mc_config::load::LoadedConfig {
            config,
            warnings: Vec::new(),
            sources: vec!["test".to_string()],
        }),
        db,
        TOKEN.to_string(),
        Timestamp::from_millis(T0),
        dir.path().to_path_buf(),
    ));
    (dir, state)
}

// ---------------------------------------------------------------- 默认不出网

/// **核心断言**：`ai_upload = false` 时，即使端点配置完整，也不组装任何 provider。
#[test]
fn no_provider_is_built_when_upload_is_disabled() {
    let config = load_config(false);
    let secrets = secrets();

    assert!(
        activities::build_vision_worker(&config, &secrets)
            .expect("组装不该报错")
            .is_none(),
        "视觉推断：未同意出网时不得组装 provider"
    );
    assert!(
        summary::build_summary_generator(&config, &secrets)
            .expect("组装不该报错")
            .is_none(),
        "总结：未同意出网时不得组装 provider"
    );
}

/// 对话引擎必须退化为「只列检索结果」的本地引擎，而不是带上 provider
#[tokio::test(flavor = "multi_thread")]
async fn chat_degrades_to_local_engine_when_upload_is_disabled() {
    let (_dir, state) = state(load_config(false));

    let engine = chat::engine_for(&state);
    let answer = engine
        .answer(&chat::ChatInput {
            query: "我今天做了什么？".to_string(),
            citations: Vec::new(),
            history: Vec::new(),
        })
        .await
        .expect("本地引擎必须能回答");

    assert!(
        answer.model.is_none(),
        "未同意出网时不该有模型参与：{:?}",
        answer.model
    );
}

/// 对照：显式同意后**且**端点齐全时才组装 provider
#[test]
fn provider_is_built_once_upload_is_allowed() {
    let config = load_config(true);
    let secrets = secrets();

    assert!(
        activities::build_vision_worker(&config, &secrets)
            .expect("组装不该报错")
            .is_some(),
        "同意出网 + 端点齐全 → 应当组装 provider"
    );
    assert!(summary::build_summary_generator(&config, &secrets)
        .expect("组装不该报错")
        .is_some());
}

/// 同意出网但端点没配 → 仍然不组装（不能凭「同意」就发到空端点）
#[test]
fn allowing_upload_without_an_endpoint_still_builds_nothing() {
    let mut config = load_config(true);
    config.ai.vision.base_url = String::new();
    config.ai.chat.model = String::new();
    let secrets = secrets();

    assert!(activities::build_vision_worker(&config, &secrets)
        .expect("组装不该报错")
        .is_none());
    assert!(summary::build_summary_generator(&config, &secrets)
        .expect("组装不该报错")
        .is_none());
}

/// 关掉 AI 时无论 `ai_upload` 是什么都不该出网
#[test]
fn disabled_ai_never_builds_a_provider() {
    let mut config = load_config(true);
    config.ai.enabled = false;
    let secrets = secrets();

    assert!(activities::build_vision_worker(&config, &secrets)
        .expect("组装不该报错")
        .is_none());
    assert!(summary::build_summary_generator(&config, &secrets)
        .expect("组装不该报错")
        .is_none());
}

/// 端到端：`ai_upload = false` 时，定时推断路径不发任何请求
/// （用「没有挂载任何 provider」这一物理事实来保证）
#[tokio::test]
async fn inference_does_not_run_without_upload_consent() {
    let (_dir, state) = state(load_config(false));

    let batch = activities::infer_pending(&state, Timestamp::from_millis(T0))
        .await
        .expect("未同意出网时推断应当是空操作而不是错误");

    assert_eq!(
        batch.suggestions.len(),
        0,
        "未同意出网时不该产生任何推断结果"
    );
}

/// 只读实例（无配置写回）也要能正常降级，不应 panic
#[tokio::test]
async fn router_still_works_with_upload_disabled() {
    let (_dir, state) = state(load_config(false));
    let request = axum::http::Request::builder()
        .method("GET")
        .uri("/api/diagnostics")
        .header("host", "127.0.0.1:12345")
        .header("x-mc-token", TOKEN)
        .body(axum::body::Body::empty())
        .unwrap();

    let response = mc_server::router(Arc::clone(&state))
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
}
