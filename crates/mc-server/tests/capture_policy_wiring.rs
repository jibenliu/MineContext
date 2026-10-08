//! 配置 → 采集策略的接线守卫。
//!
//! 这一片修的三个问题有一个共同形状：**配置项存在，代码从不读它**
//! （`capture.sources` 如此、`blocked_domains` 如此）。所以这里钉一条
//! 最直接的断言：`privacy` 段的每一条规则都必须出现在 `PumpPolicy` 里。
//!
//! 用了「先删掉一处接线、看它是否变红」的反向探针验证过它真的抓得住
//! 。

use mc_server::capture_loop::policy_from;

fn config_with_privacy(toml: &str) -> mc_config::Config {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, toml).unwrap();

    let request = mc_config::load::LoadRequest {
        layers: vec![mc_config::load::LayerSource::File(path)],
        env: Vec::new(),
        read_process_env: false,
    };
    mc_config::load::load(&request).unwrap().config
}

#[test]
fn every_privacy_rule_reaches_the_pump_policy() {
    let config = config_with_privacy(
        r#"
[privacy]
ai_upload = false
blocked_apps = ["1Password", "WeChat"]
blocked_window_patterns = ["(?i)password"]
blocked_domains = ["*.bank.com"]

[capture]
target_ids = ["display-1"]
"#,
    );

    let policy = policy_from(&config);

    assert_eq!(policy.blocked_apps, vec!["1Password", "WeChat"]);
    assert_eq!(policy.blocked_window_patterns, vec!["(?i)password"]);
    assert_eq!(policy.selected_targets, vec!["display-1"]);

    // 域名规则：断言行为而不是内部结构 —— 归一化后的规则要真的能拦
    assert!(policy
        .blocked_domains
        .is_blocked(Some("https://login.bank.com/"), ""));
    assert!(!policy
        .blocked_domains
        .is_blocked(Some("https://example.com/"), ""));
}

#[test]
fn default_config_has_no_privacy_rules() {
    let config = config_with_privacy("[capture]\nenabled = true\n");
    let policy = policy_from(&config);

    assert!(policy.blocked_apps.is_empty());
    assert!(policy.blocked_window_patterns.is_empty());
    assert!(!policy
        .blocked_domains
        .is_blocked(Some("https://bank.com/"), ""));
}
