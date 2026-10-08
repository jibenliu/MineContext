//! `privacy.blocked_domains`：域名规则必须真的拦得住。
//!
//! 用户写了 `blocked_domains = ["*.bank.com"]`，就必须真的拦住。语义：
//! - 规则写法归一化：`bank.com`、`*.bank.com`、`https://bank.com/login` 都指向同一个域名，**少写一个星号不该导致漏拦**（fail-closed）；
//! - 有 URL 时按 **host 后缀**匹配（`login.bank.com` 命中 `bank.com`），而不是字符串包含（`notbank.com` 不该命中）；
//! - 没有 URL 时退化为窗口标题匹配 —— 这是**启发式**：只覆盖标题里真的含有域名的情形，写进隐私说明，不假装是完整实现。

use mc_pipeline::domains::DomainRules;
use mc_pipeline::pump::PumpPolicy;

fn rules(patterns: &[&str]) -> DomainRules {
    DomainRules::new(
        &patterns
            .iter()
            .map(|p| (*p).to_string())
            .collect::<Vec<_>>(),
    )
}

#[test]
fn plain_domain_blocks_itself_and_subdomains() {
    let rules = rules(&["bank.com"]);

    assert!(rules.is_blocked(Some("https://bank.com/login"), ""));
    assert!(rules.is_blocked(Some("https://login.bank.com/"), ""));
    assert!(rules.is_blocked(Some("https://a.b.bank.com/x?y=1"), ""));
}

#[test]
fn suffix_matching_is_not_substring_matching() {
    let rules = rules(&["bank.com"]);

    // 反向探针：`notbank.com` / `bank.com.evil.io` 都不是它的子域。
    // 用字符串包含就会误伤第一个、放过第二个。
    assert!(!rules.is_blocked(Some("https://notbank.com/"), ""));
    assert!(!rules.is_blocked(Some("https://bank.com.evil.io/"), ""));
    assert!(!rules.is_blocked(Some("https://example.com/"), ""));
}

#[test]
fn rule_writing_variants_are_normalized() {
    for pattern in [
        "bank.com",
        "*.bank.com",
        ".bank.com",
        "BANK.com",
        "https://bank.com",
        "https://bank.com/login?next=/x",
        "bank.com:443",
    ] {
        let rules = rules(&[pattern]);
        assert!(
            rules.is_blocked(Some("https://login.bank.com/"), ""),
            "写法 {pattern} 应当命中 login.bank.com"
        );
    }
}

#[test]
fn title_is_used_when_there_is_no_url() {
    // 没有 URL（我们的窗口采集只有标题）时退化为标题匹配。
    // 这是启发式：能拦住标题里带域名的窗口，拦不住只有站点名的窗口。
    let rules = rules(&["*.bank.com"]);

    assert!(rules.is_blocked(None, "登录 - bank.com"));
    assert!(!rules.is_blocked(None, "银行 App 登录页"));
}

#[test]
fn empty_rules_block_nothing() {
    let rules = rules(&["", "   "]);

    assert!(!rules.is_blocked(Some("https://bank.com/"), "bank.com"));
}

#[test]
fn policy_carries_the_rules() {
    let policy = PumpPolicy {
        blocked_domains: rules(&["bank.com"]),
        ..PumpPolicy::default()
    };

    assert!(policy
        .blocked_domains
        .is_blocked(Some("https://bank.com/"), ""));
}
