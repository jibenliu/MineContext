//! 显示器子集选择的配置面。
//!
//! 「要录哪块屏」若只存在内存里，重启即为空 —— 于是采集静默停摆，
//! 直到用户再打开一次设置页。因此这里把它落到配置，并明确规定
//! **空 = 全部可见目标**（没写就是没限制，而不是「什么都不采」）。

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
fn selection_defaults_to_empty_meaning_all_targets() {
    let loaded = load_with("");
    assert!(
        loaded.config.capture.target_ids.is_empty(),
        "缺省必须是空（= 全部），而不是写死某块屏"
    );
}

#[test]
fn selection_is_parsed_from_config() {
    let loaded = load_with(
        r#"
        [capture]
        target_ids = ["display-1", "display-2"]
        "#,
    );
    assert_eq!(
        loaded.config.capture.target_ids,
        vec!["display-1".to_string(), "display-2".to_string()]
    );
}

/// 显式给空数组与不写是同一件事：都是「不限制」。
#[test]
fn explicit_empty_list_is_the_same_as_absent() {
    let loaded = load_with("[capture]\ntarget_ids = []\n");
    assert!(loaded.config.capture.target_ids.is_empty());
}

// ---------------------------------------------------------------- 区域采集

#[test]
fn region_is_parsed_from_config() {
    let loaded = load_with("[capture]\nregion = [100, 200, 500, 500]\n");
    assert_eq!(loaded.config.capture.region, Some([100, 200, 500, 500]));
}

/// 顺序写反的矩形必须是**配置错误**，而不是被静默接受成一张空图。
#[test]
fn inverted_region_is_rejected_at_load_time() {
    let error = load(&LoadRequest {
        layers: vec![LayerSource::Inline {
            name: "test".to_string(),
            toml: "[capture]\nregion = [500, 500, 100, 200]\n".to_string(),
        }],
        env: Vec::new(),
        read_process_env: false,
    })
    .expect_err("写反的矩形必须被拒绝");
    assert_eq!(error.code(), mc_common::error::ErrorCode::ConfigInvalid);
}

/// 旧配置里是 `{left, top, width, height}` 字典，迁移时要换算成
/// `[left, top, right, bottom]` —— 换算错了区域就会偏。
#[test]
fn legacy_region_object_is_migrated() {
    let legacy = "capture:\n  screenshot:\n    screenshot_region:\n      left: 100\n      top: 200\n      width: 400\n      height: 300\n";
    let result = mc_config::legacy::migrate_legacy(&mc_config::legacy::LegacyInput {
        config_yaml: Some(legacy),
        user_setting_yaml: None,
    })
    .expect("迁移必须成功");

    assert_eq!(
        result.config.capture.region,
        Some([100, 200, 500, 500]),
        "right/bottom 必须是 left+width / top+height"
    );
}

/// 旧区域缺少字段时给 warning 并忽略，而不是让整个迁移失败
#[test]
fn malformed_legacy_region_is_ignored_with_a_warning() {
    let legacy = "capture:\n  screenshot:\n    screenshot_region:\n      left: 100\n";
    let result = mc_config::legacy::migrate_legacy(&mc_config::legacy::LegacyInput {
        config_yaml: Some(legacy),
        user_setting_yaml: None,
    })
    .expect("迁移不该因此失败");

    assert_eq!(result.config.capture.region, None);
    assert!(
        result
            .warnings
            .iter()
            .any(|warning| warning.path.contains("screenshot_region")),
        "必须给出 warning：{:?}",
        result.warnings
    );
}
