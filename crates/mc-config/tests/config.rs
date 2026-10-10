//! 配置分层加载、校验与热重载。
//!
//! 设计要点：
//! - 分层：内置默认 < 系统配置 < 用户配置 < 环境变量
//! - **非法字段报错并指位置，绝不静默忽略**（静默忽略是「配置改了没生效」类问题的根源）
//! - 热重载失败时**保留旧配置**，不因为一次手滑把生效配置清空
//!
//! 这些测试全部使用内联 TOML，不碰文件系统，因此完全确定。

use mc_common::error::ErrorCode;
use mc_config::load::{load, LayerSource, LoadRequest};

fn inline(name: &str, toml: &str) -> LayerSource {
    LayerSource::Inline {
        name: name.to_string(),
        toml: toml.to_string(),
    }
}

/// 只加载内置默认，不读进程环境（否则测试会随环境漂移）。
fn load_layers(
    layers: Vec<LayerSource>,
) -> Result<mc_config::LoadedConfig, mc_common::error::AppError> {
    load(&LoadRequest {
        layers,
        env: Vec::new(),
        read_process_env: false,
    })
}

fn load_layers_with_env(
    layers: Vec<LayerSource>,
    env: Vec<(String, String)>,
) -> Result<mc_config::LoadedConfig, mc_common::error::AppError> {
    load(&LoadRequest {
        layers,
        env,
        read_process_env: false,
    })
}

#[test]
fn loads_defaults_when_no_file() {
    let loaded = load_layers(vec![]).expect("builtin defaults must always load");

    assert!(loaded.config.capture.enabled);
    assert_eq!(loaded.config.capture.interval_secs, 15);
    assert_eq!(loaded.config.capture.idle_threshold_secs, 300);
    assert_eq!(loaded.config.capture.idle_interval_secs, 60);
    assert!(
        loaded.config.capture.pause_on_lock,
        "锁屏暂停采集默认开启（安全默认；可在配置里关掉）"
    );
    assert_eq!(loaded.config.capture.retention_days, 7);
    assert_eq!(loaded.config.storage.max_total_gb, 10.0);
    assert_eq!(loaded.config.storage.max_screenshot_count, 200_000);
    assert_eq!(loaded.config.activity.debounce_secs, 45);
    assert_eq!(loaded.config.stage.min_duration_secs, 900);
    assert_eq!(loaded.config.ai.budget.max_vlm_calls_per_hour, 240);
    // 隐私默认：不出网（见 docs/privacy.md）
    assert!(!loaded.config.privacy.ai_upload);
    assert!(loaded.sources.is_empty());
}

// 0.8b — 时区未配置时不要假装知道
#[test]
fn timezone_is_unset_by_default_and_reported_as_a_warning() {
    let loaded = load_layers(vec![]).unwrap();

    assert_eq!(loaded.config.general.timezone, None);
    assert!(
        loaded.warnings.iter().any(|w| w.path == "general.timezone"),
        "未设置时区必须给出一条 warning，而不是静默用 UTC 算日边界"
    );
}

#[test]
fn layered_override_precedence_is_system_then_user_then_env() {
    let system = inline("system.toml", "capture_interval = 60\n");
    let _ = system;

    let loaded = load_layers(vec![
        inline(
            "system.toml",
            "[capture]\ninterval_secs = 30\nretention_days = 3\n",
        ),
        inline("user.toml", "[capture]\ninterval_secs = 45\n"),
    ])
    .unwrap();

    // 用户覆盖系统
    assert_eq!(loaded.config.capture.interval_secs, 45);
    // 未被覆盖的系统层值保留
    assert_eq!(loaded.config.capture.retention_days, 3);

    let with_env = load_layers_with_env(
        vec![
            inline("system.toml", "[capture]\ninterval_secs = 30\n"),
            inline("user.toml", "[capture]\ninterval_secs = 45\n"),
        ],
        vec![(
            "MC_CONFIG__capture__interval_secs".to_string(),
            "90".to_string(),
        )],
    )
    .unwrap();

    // 环境变量优先级最高
    assert_eq!(with_env.config.capture.interval_secs, 90);
}

// 0.9b — 环境变量按 TOML 解析类型，不要全都当字符串
#[test]
fn env_override_parses_typed_values() {
    let loaded = load_layers_with_env(
        vec![],
        vec![
            (
                "MC_CONFIG__capture__interval_secs".to_string(),
                "30".to_string(),
            ),
            (
                "MC_CONFIG__capture__enabled".to_string(),
                "false".to_string(),
            ),
            (
                "MC_CONFIG__privacy__ai_upload".to_string(),
                "true".to_string(),
            ),
        ],
    )
    .unwrap();

    assert_eq!(loaded.config.capture.interval_secs, 30);
    assert!(!loaded.config.capture.enabled);
    assert!(loaded.config.privacy.ai_upload);
}

// 0.9c — 类型错误的环境变量必须报错，不能被当成字符串硬塞
#[test]
fn invalid_env_override_is_rejected() {
    let err = load_layers_with_env(
        vec![],
        vec![(
            "MC_CONFIG__capture__interval_secs".to_string(),
            "\"not-a-number\"".to_string(),
        )],
    )
    .unwrap_err();

    assert_eq!(err.code(), ErrorCode::ConfigInvalid);
    assert!(
        err.detail().contains("capture.interval_secs"),
        "错误必须指出字段路径，实际: {}",
        err.detail()
    );
}

#[test]
fn invalid_syntax_reports_source_and_line() {
    let err = load_layers(vec![inline(
        "user.toml",
        "[capture]\ninterval_secs = = 30\n",
    )])
    .unwrap_err();

    assert_eq!(err.code(), ErrorCode::ConfigInvalid);
    let detail = err.detail();
    assert!(detail.contains("user.toml"), "必须指出哪个文件: {detail}");
    assert!(detail.contains("line"), "语法错误必须给出行号: {detail}");
}

// 0.10b
#[test]
fn unknown_field_is_rejected_not_ignored() {
    let err = load_layers(vec![inline(
        "user.toml",
        "[capture]\ninterval_seconds = 30\n",
    )])
    .unwrap_err();

    assert_eq!(err.code(), ErrorCode::ConfigInvalid);
    assert!(
        err.detail().contains("interval_seconds"),
        "拼错的字段必须报错而不是被忽略: {}",
        err.detail()
    );
}

// 0.10c — 类型错误必须指出完整字段路径
#[test]
fn invalid_value_reports_field_path() {
    let err = load_layers(vec![inline(
        "user.toml",
        "[capture]\ninterval_secs = \"thirty\"\n",
    )])
    .unwrap_err();

    assert_eq!(err.code(), ErrorCode::ConfigInvalid);
    assert!(
        err.detail().contains("capture.interval_secs"),
        "类型错误必须指出字段路径，实际: {}",
        err.detail()
    );
}

// 0.10d — 只有 OpenAI 兼容一种 Provider 合法（类型级防线）
#[test]
fn only_openai_compatible_provider_is_accepted() {
    let ok = load_layers(vec![inline(
        "user.toml",
        "[ai.vision]\nprovider = \"openai_compatible\"\nmodel = \"qwen3-vl\"\n",
    )])
    .unwrap();
    assert_eq!(ok.config.ai.vision.model, "qwen3-vl");

    let err = load_layers(vec![inline(
        "user.toml",
        "[ai.vision]\nprovider = \"doubao\"\n",
    )])
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigInvalid);
}

// 0.11 — 热重载：坏配置不能把生效配置清空
#[test]
fn hot_reload_keeps_old_config_on_parse_error() {
    let handle = mc_config::ConfigHandle::new(
        load_layers(vec![inline("user.toml", "[capture]\ninterval_secs = 45\n")]).unwrap(),
    );

    assert_eq!(handle.current().config.capture.interval_secs, 45);

    // 用户手滑写了坏配置
    let outcome = handle.reload(&LoadRequest {
        layers: vec![inline("user.toml", "[capture]\ninterval_secs = = 45\n")],
        env: Vec::new(),
        read_process_env: false,
    });

    assert!(matches!(outcome, mc_config::ReloadOutcome::Rejected(_)));
    assert_eq!(
        handle.current().config.capture.interval_secs,
        45,
        "重载失败必须保留旧配置"
    );
}

// 0.11b — 正常重载要生效
#[test]
fn hot_reload_applies_valid_config() {
    let handle = mc_config::ConfigHandle::new(load_layers(vec![]).unwrap());

    let outcome = handle.reload(&LoadRequest {
        layers: vec![inline("user.toml", "[capture]\ninterval_secs = 20\n")],
        env: Vec::new(),
        read_process_env: false,
    });

    assert!(matches!(outcome, mc_config::ReloadOutcome::Applied(_)));
    assert_eq!(handle.current().config.capture.interval_secs, 20);
}

// 0.11c — 并发读取时重载不能撕裂（读到的必须是完整的某一个版本）
#[test]
fn reload_is_atomic_for_readers() {
    use std::sync::Arc;

    let handle = Arc::new(mc_config::ConfigHandle::new(load_layers(vec![]).unwrap()));
    let reader_handle = Arc::clone(&handle);

    let reader = std::thread::spawn(move || {
        for _ in 0..2_000 {
            let cfg = reader_handle.current();
            // 两个字段必须来自同一个版本
            let a = cfg.config.capture.interval_secs;
            let b = cfg.config.capture.retention_days;
            assert!(
                (a, b) == (15, 7) || (a, b) == (20, 9),
                "读到撕裂的配置: interval={a} retention={b}"
            );
        }
    });

    for _ in 0..200 {
        handle.reload(&LoadRequest {
            layers: vec![inline(
                "user.toml",
                "[capture]\ninterval_secs = 20\nretention_days = 9\n",
            )],
            env: Vec::new(),
            read_process_env: false,
        });
        handle.reload(&LoadRequest {
            layers: vec![],
            env: Vec::new(),
            read_process_env: false,
        });
    }

    reader.join().expect("reader thread must not panic");
}

// 0.11d — 配置必须能落盘再读回（round-trip），否则「保存设置」会丢字段
#[test]
fn config_roundtrips_through_toml() {
    let loaded = load_layers(vec![inline(
        "user.toml",
        "[general]\ntimezone = \"Asia/Shanghai\"\nlocale = \"zh-CN\"\n\n[capture]\ninterval_secs = 30\n",
    )])
    .unwrap();

    let text = toml::to_string_pretty(&loaded.config).expect("config must serialize");
    let reloaded = load_layers(vec![inline("roundtrip.toml", &text)]).unwrap();

    assert_eq!(reloaded.config, loaded.config);
}

// 0.11e — 读文件失败要报可读错误，而不是 panic
#[test]
fn missing_file_is_reported_as_unreadable() {
    let err = load_layers(vec![LayerSource::File(std::path::PathBuf::from(
        "/nonexistent/minecontext/config.toml",
    ))])
    .unwrap_err();

    assert_eq!(err.code(), ErrorCode::ConfigUnreadable);
    assert!(err.detail().contains("config.toml"));
}

// 阶段策略必须与配置一致：重放与实时用两套参数，时间线就会对不上
#[test]
fn stage_config_maps_to_domain_policy() {
    let loaded = mc_config::load(&mc_config::load::LoadRequest::default()).expect("默认配置");
    let policy = loaded.config.stage.stage_policy("Asia/Shanghai");

    assert_eq!(
        policy.min_duration_secs,
        loaded.config.stage.min_duration_secs
    );
    assert_eq!(policy.max_duration_secs, 7200);
    assert_eq!(policy.switch_grace_secs, 300);
    assert_eq!(policy.idle_threshold_secs, 300);
    assert_eq!(policy.min_activity_stable_secs, 60);
    assert_eq!(policy.summary_deadline_secs, 600);
    assert_eq!(policy.timezone, "Asia/Shanghai", "日边界必须跟随用户时区");
}
