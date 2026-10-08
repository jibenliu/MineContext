//! 旧配置迁移。
//!
//! 现有用户的配置是一个 252 行的 YAML（数据目录里的 `config.yaml`）外加一个
//! `user_setting.yaml`（旧版 `save_user_settings()` 写出，**明文包含 API Key**）。
//! 迁移必须做到三件事：
//!
//! 1. 已知字段**不丢**（0.12）
//! 2. 无法映射的字段**变成 warnings**，而不是静默消失（0.13）
//! 3. 明文密钥**不进新配置**，转为 Keychain 引用（0.14）

use mc_config::legacy::{migrate_legacy, LegacyInput};

const LEGACY_CONFIG: &str = include_str!("../../../fixtures/legacy/config.yaml");
const LEGACY_USER_SETTING: &str = include_str!("../../../fixtures/legacy/user_setting.yaml");

fn migrate() -> mc_config::legacy::MigrationResult {
    migrate_legacy(&LegacyInput {
        config_yaml: Some(LEGACY_CONFIG),
        user_setting_yaml: Some(LEGACY_USER_SETTING),
    })
    .expect("legacy fixtures must migrate")
}

#[test]
fn legacy_config_migration_preserves_known_fields() {
    let result = migrate();
    let c = &result.config;

    // user_setting 覆盖了 config.yaml → 以用户值为准
    assert_eq!(c.capture.interval_secs, 15);
    assert!(c.capture.enabled);

    assert_eq!(
        c.ai.vision.base_url,
        "https://ark.cn-beijing.volces.com/api/v3"
    );
    assert_eq!(c.ai.vision.model, "doubao-seed-1-6-vision-250815");
    assert_eq!(
        c.ai.embedding.base_url,
        "https://ark.cn-beijing.volces.com/api/v3"
    );
    assert_eq!(c.ai.embedding.model, "doubao-embedding-vision-250615");

    // prompts.language: "zh" → general.locale
    assert_eq!(c.general.locale, "zh-CN");

    // logging.level → observability.log_level
    assert_eq!(c.observability.log_level, "info");
}

// 0.12b — 旧 provider 值（doubao）不再合法，必须映射为 openai_compatible 并告知用户
#[test]
fn legacy_vendor_provider_is_mapped_to_openai_compatible_with_a_warning() {
    let result = migrate();

    assert_eq!(
        result.config.ai.vision.provider,
        mc_config::model::ProviderKind::OpenAiCompatible
    );
    assert_eq!(
        result.config.ai.embedding.provider,
        mc_config::model::ProviderKind::OpenAiCompatible
    );

    let w = result
        .warnings
        .iter()
        .find(|w| w.path.contains("provider") && w.message.contains("doubao"))
        .expect("必须告知用户 provider 已归一化为 openai_compatible");
    assert!(
        w.message.contains("openai_compatible"),
        "warning 应说明替换成了什么: {}",
        w.message
    );
}

#[test]
fn legacy_unmapped_fields_surface_as_warnings() {
    let result = migrate();

    assert!(
        !result.warnings.is_empty(),
        "旧配置里必然有无法映射的字段，必须逐条报出来"
    );

    // 这些是本次刻意不迁移的：向量库选型、合并策略、文档处理流水线
    for expected in [
        "storage.backends",
        "processing.context_merger.enabled",
        "processing.screenshot_processor.batch_size",
    ] {
        assert!(
            result.warnings.iter().any(|w| w.path == expected),
            "缺少对 {expected} 的迁移说明；实际 warnings: {:?}",
            result.warnings.iter().map(|w| &w.path).collect::<Vec<_>>()
        );
    }

    // 已知被映射的字段不应出现在 warnings 里
    assert!(
        !result
            .warnings
            .iter()
            .any(|w| w.path == "capture.screenshot.capture_interval"),
        "已映射字段不应被报为未映射"
    );
}

// 0.13b — 环境变量占位符必须被识别出来并提示用户填写
#[test]
fn unresolved_env_placeholders_are_reported() {
    let result = migrate();

    // config.yaml 里 vlm_model.base_url 是 "${LLM_BASE_URL}"，被 user_setting 覆盖了；
    // 但 embedding 的 output_dim 等字段仍应能识别。这里断言占位符检测本身可用。
    assert!(
        result
            .warnings
            .iter()
            .any(|w| w.message.contains("占位符") || w.message.contains("placeholder")),
        "未解析的 ${{VAR}} 占位符必须提示用户填写"
    );
}

// 0.13c — 迁移结果必须能落盘再读回
#[test]
fn migrated_config_roundtrips_through_toml() {
    let result = migrate();

    let text = toml::to_string_pretty(&result.config).expect("migrated config must serialize");
    let reloaded = mc_config::load::load(&mc_config::load::LoadRequest {
        layers: vec![mc_config::load::LayerSource::Inline {
            name: "migrated.toml".to_string(),
            toml: text,
        }],
        env: Vec::new(),
        read_process_env: false,
    })
    .expect("migrated config must load back");

    assert_eq!(reloaded.config, result.config);
}

// 0.14 — 明文密钥绝不进新配置
#[test]
fn api_key_ref_never_contains_plaintext() {
    let result = migrate();

    assert_eq!(
        result.config.ai.vision.api_key_ref.as_deref(),
        Some("keychain:provider:vision")
    );
    assert_eq!(
        result.config.ai.embedding.api_key_ref.as_deref(),
        Some("keychain:provider:embedding")
    );

    let text = toml::to_string_pretty(&result.config).expect("config must serialize");
    assert!(
        !text.contains("sk-SECRET"),
        "序列化后的配置里出现了明文密钥:\n{text}"
    );
    assert!(!text.contains("0123456789abcdef"));
    assert!(!text.contains("api_key ="), "不应再出现明文 api_key 字段");

    // 配置里也不该有任何字段长得像明文 key
    for value in [
        result.config.ai.vision.model.as_str(),
        &result.config.ai.vision.base_url,
    ] {
        assert!(!value.contains("sk-"));
    }
}

// 0.14b — 密钥被交给调用方去写 Keychain，而不是被丢弃
#[test]
fn legacy_keychain_imports_carry_the_secret() {
    let result = migrate();

    let accounts: Vec<&str> = result
        .keychain_imports
        .iter()
        .map(|k| k.account.as_str())
        .collect();
    assert!(accounts.contains(&"provider:vision"));
    assert!(accounts.contains(&"provider:embedding"));

    let vision = result
        .keychain_imports
        .iter()
        .find(|k| k.account == "provider:vision")
        .unwrap();
    assert_eq!(vision.secret, "sk-SECRET-VLM-KEY-0123456789abcdef");
}

// 0.14c — 空密钥不产生 Keychain 导入项（避免写入垃圾）
#[test]
fn empty_legacy_api_key_produces_no_keychain_import() {
    let result = migrate_legacy(&LegacyInput {
        config_yaml: Some(
            "vlm_model:\n  base_url: \"http://localhost:11434/v1\"\n  model: \"qwen3-vl\"\n  api_key: \"\"\n",
        ),
        user_setting_yaml: None,
    })
    .unwrap();

    assert!(result.keychain_imports.is_empty());
    assert_eq!(result.config.ai.vision.api_key_ref, None);
    assert_eq!(
        result.config.ai.vision.base_url,
        "http://localhost:11434/v1"
    );
}

// 0.14d — 完全没有旧配置时，迁移应当给出默认配置而不是失败
#[test]
fn migration_with_no_input_yields_defaults() {
    let result = migrate_legacy(&LegacyInput {
        config_yaml: None,
        user_setting_yaml: None,
    })
    .unwrap();

    assert_eq!(result.config, mc_config::model::Config::default());
    assert!(result.warnings.iter().any(|w| w.path == "general.timezone"));
}

// 0.14e — 旧配置本身语法坏掉时必须报错，而不是给出半截配置
#[test]
fn broken_legacy_yaml_is_rejected() {
    let err = migrate_legacy(&LegacyInput {
        config_yaml: Some("[capture\ninterval: 15\n"),
        user_setting_yaml: None,
    })
    .unwrap_err();

    assert_eq!(err.code(), mc_common::error::ErrorCode::ConfigInvalid);
}
