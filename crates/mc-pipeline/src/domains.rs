//! 域名黑名单（`privacy.blocked_domains`）。
//!
//! 规则归一化：`bank.com` / `*.bank.com` / `.bank.com` / `BANK.com` /
//! `https://bank.com/x` / `bank.com:443` 等价 —— 少写一个星号不该漏拦，
//! 多写也不该失效。
//! 匹配按 **host 后缀**：`login.bank.com` 命中 `bank.com`，而
//! `notbank.com` 与 `bank.com.evil.io` 不命中（字符串包含会同时误伤前者、
//! 放过后者）。拿不到 URL 时退化为「标题包含域名」的启发式 ——
//! 匹配边界与误伤范围见 `docs/privacy.md`。

use serde::{Deserialize, Serialize};

/// 归一化后的域名规则集合。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DomainRules {
    rules: Vec<String>,
}

impl DomainRules {
    pub fn new(patterns: &[String]) -> Self {
        let mut rules: Vec<String> = patterns
            .iter()
            .filter_map(|pattern| normalize(pattern))
            .collect();
        rules.sort();
        rules.dedup();
        Self { rules }
    }

    /// 是否命中。`url` 为空时退化为标题匹配。
    pub fn is_blocked(&self, url: Option<&str>, title: &str) -> bool {
        if self.rules.is_empty() {
            return false;
        }

        match url.and_then(host_of) {
            Some(host) => self.matches_host(&host),
            // 没有 URL：标题里出现域名就算命中（启发式，见模块文档）
            None => {
                let title = title.to_lowercase();
                !title.is_empty() && self.rules.iter().any(|rule| title.contains(rule))
            }
        }
    }

    fn matches_host(&self, host: &str) -> bool {
        self.rules.iter().any(|rule| {
            // 后缀匹配必须在「点」边界上：`bank.com.evil.io` 不是 `bank.com` 的子域
            host == rule || host.ends_with(&format!(".{rule}"))
        })
    }
}

/// `https://user:pass@Login.Bank.com:443/path?q=1` → `login.bank.com`
fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .rsplit('@') // 去掉 userinfo
        .next()
        .unwrap_or_default();

    // IPv6 字面量：`[::1]:8080`
    let host = if let Some(stripped) = authority.strip_prefix('[') {
        stripped.split(']').next().unwrap_or_default().to_string()
    } else {
        authority.split(':').next().unwrap_or_default().to_string()
    };

    let host = host.trim().trim_end_matches('.').to_lowercase();
    (!host.is_empty()).then_some(host)
}

/// 把各种写法归一化成裸域名（host）。
fn normalize(pattern: &str) -> Option<String> {
    let trimmed = pattern.trim();
    if trimmed.is_empty() {
        return None;
    }

    // 带端口/路径的裸域名也按 URL 处理一次，复用同一套解析
    let host = host_of(trimmed)?;
    let host = host.trim_start_matches("*.").trim_start_matches('.');
    let host = host.trim_end_matches('.').to_string();

    (!host.is_empty()).then_some(host)
}
