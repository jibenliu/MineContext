//! 域名规则的**可见性**提示。
//!
//! `privacy.blocked_domains` 的匹配需要 URL：拿到 URL 时按 host 后缀匹配，拿不到
//! 时只能退化为「窗口标题里出现域名」。当前采集路径暂无 URL 来源，所以配置了
//! 域名规则的用户实际拿到的是启发式匹配 —— 这件事必须说出来，否则用户会以为
//! URL 级规则在生效。

/// 配置了有效规则时返回提示语；没配置（或全是空白）时返回 `None`。
pub fn notice(patterns: &[String]) -> Option<&'static str> {
    let configured = patterns
        .iter()
        .filter(|pattern| !pattern.trim().is_empty())
        .count();
    (configured > 0).then_some(
        "privacy.blocked_domains 已配置，但当前采集路径没有 URL 来源：域名规则只能按窗口标题启发式匹配（见隐私说明里的局限）",
    )
}

#[cfg(test)]
mod tests {
    use super::notice;

    #[test]
    fn no_rules_means_no_notice() {
        assert!(notice(&[]).is_none());
        // 只有空白项也算没配置，不该刷一条无意义的提示
        assert!(notice(&["".to_string(), "   ".to_string()]).is_none());
    }

    #[test]
    fn configured_rules_produce_a_notice() {
        let message = notice(&["bank.com".to_string()]).expect("配置了规则就该有提示");
        assert!(message.contains("没有 URL 来源"), "{message}");
        assert!(message.contains("标题启发式"), "{message}");
    }
}
