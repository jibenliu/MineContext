//! embedding 批量上限来自**配置**，不是常量。
//!
//! 上限写死的代价在里出现过两次：一次是维度写死 1536，
//! 一次是把整天的活动拼成一个巨大 `input` 数组。
//! 各家端点限制不同（64 / 100 / 2048），只有配置能同时满足它们。

use mc_config::load::{load, LayerSource, LoadRequest};

fn load_with(toml: &str) -> mc_config::LoadedConfig {
    load(&LoadRequest {
        layers: vec![LayerSource::Inline {
            name: "test".to_string(),
            toml: toml.to_string(),
        }],
        env: Vec::new(),
        read_process_env: false,
    })
    .expect("配置必须可加载")
}

#[test]
fn embedding_batch_limit_defaults_without_being_required() {
    let loaded = load_with("");
    assert_eq!(
        loaded.config.ai.embedding.batch_limit, 64,
        "默认 64 是常见端点的安全上限；用户不配也能工作"
    );
}

#[test]
fn embedding_batch_limit_is_configurable() {
    let loaded = load_with(
        r#"
        [ai.embedding]
        batch_limit = 8
        "#,
    );
    assert_eq!(loaded.config.ai.embedding.batch_limit, 8);
}

#[test]
fn embedding_batch_limit_must_not_be_zero() {
    // 0 会让切批失败（见 `mc_search::plan_batches`）。配置校验必须在这里拦住，
    // 而不是等运行到索引时才报错。
    let error = load(&LoadRequest {
        layers: vec![LayerSource::Inline {
            name: "test".to_string(),
            toml: "[ai.embedding]\nbatch_limit = 0\n".to_string(),
        }],
        env: Vec::new(),
        read_process_env: false,
    })
    .expect_err("batch_limit = 0 必须被拒绝");

    assert_eq!(error.code(), mc_common::error::ErrorCode::ConfigInvalid);
    assert!(
        error.detail().contains("batch_limit"),
        "错误必须指出是哪个字段：{}",
        error.detail()
    );
}
