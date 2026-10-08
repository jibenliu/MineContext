//! 关键词打分。
//!
//! 刻意**不用模糊匹配**：用户搜 `APEX-389` 时期望的是精确命中，
//! 把 `APEX-390` 也排进来只会让人不再相信搜索。
//!
//! 打分规则（确定性、可解释）：
//! - 整串出现 → 强权重（这就是「精确 ID 一次命中」的来源）；
//! - 词元全中 → 中权重（按命中比例）；
//! - 部分命中 → 弱权重。

/// 把文本切成小写词元。中英文混排时按「字母数字串」与「单字」分别切。
pub fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        if ch.is_alphanumeric() {
            current.push(ch.to_ascii_lowercase());
        } else {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            // 中日韩单字本身有意义，单独成词
            if is_cjk(ch) {
                tokens.push(ch.to_string());
            }
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn is_cjk(ch: char) -> bool {
    matches!(ch as u32, 0x3040..=0x30FF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF)
}

/// 命中档次。**分档是必要的**：搜 `APEX-389` 时把 `APEX-390` 一起返回，
/// 用户很快就会不再相信搜索。有了档次，就能做到「有精确命中就只给精确命中」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchTier {
    /// 整串出现：精确 ID、路径、人名
    Verbatim,
    /// 查询的每个词元都出现（AND 语义）
    AllTokens,
    /// 只命中了一部分（OR 语义，兜底用）
    Partial,
    None,
}

impl PartialOrd for MatchTier {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MatchTier {
    /// **显式**写出「谁更好」，而不是依赖枚举声明顺序：
    /// `hybrid_search` 用 `.min()` 取最好档次，调整声明顺序会静默改变语义。
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        fn rank(tier: MatchTier) -> u8 {
            match tier {
                MatchTier::Verbatim => 0,
                MatchTier::AllTokens => 1,
                MatchTier::Partial => 2,
                MatchTier::None => 3,
            }
        }
        rank(*self).cmp(&rank(*other))
    }
}

pub fn classify(query: &str, text: &str) -> MatchTier {
    let query = query.trim();
    if query.is_empty() {
        return MatchTier::None;
    }

    let haystack = text.to_lowercase();
    let needle = query.to_lowercase();
    if haystack.contains(&needle) {
        return MatchTier::Verbatim;
    }

    let query_tokens = tokenize(query);
    if query_tokens.is_empty() {
        return MatchTier::None;
    }

    let text_tokens: std::collections::HashSet<String> = tokenize(text).into_iter().collect();
    let hits = query_tokens
        .iter()
        .filter(|token| text_tokens.contains(*token) || haystack.contains(token.as_str()))
        .count();

    match hits {
        0 => MatchTier::None,
        hits if hits == query_tokens.len() => MatchTier::AllTokens,
        _ => MatchTier::Partial,
    }
}

/// 关键词分。范围 `0.0..=1.0`；`0.0` 表示完全没命中。
pub fn score(query: &str, text: &str) -> f32 {
    match classify(query, text) {
        MatchTier::Verbatim => 1.0,
        MatchTier::AllTokens => 0.9,
        MatchTier::Partial => {
            let query_tokens = tokenize(query);
            let haystack = text.to_lowercase();
            let text_tokens: std::collections::HashSet<String> =
                tokenize(text).into_iter().collect();
            let hits = query_tokens
                .iter()
                .filter(|token| text_tokens.contains(*token) || haystack.contains(token.as_str()))
                .count();
            // 部分命中最高 0.5：明显低于「全中」，避免它挤掉更相关的结果
            0.5 * (hits as f32 / query_tokens.len().max(1) as f32)
        }
        MatchTier::None => 0.0,
    }
}

#[cfg(test)]
mod tier_order_tests {
    use super::MatchTier;

    /// 档次顺序是**显式**的：`hybrid_search` 用 `.min()` 取最好档次，
    /// 如果有人调整枚举声明顺序，这条测试会拦住语义漂移。
    #[test]
    fn tier_order_is_explicit() {
        assert!(MatchTier::Verbatim < MatchTier::AllTokens);
        assert!(MatchTier::AllTokens < MatchTier::Partial);
        assert!(MatchTier::Partial < MatchTier::None);
        assert_eq!(
            [MatchTier::None, MatchTier::Verbatim, MatchTier::Partial]
                .into_iter()
                .min(),
            Some(MatchTier::Verbatim)
        );
    }
}
