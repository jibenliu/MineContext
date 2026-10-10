//! 轻量上下文组装（context-lite）。
//!
//! 把检索命中收成带 **token 预算** 的 ContextPack，供 Chat / MCP Server 共用。
//! 刻意留在 `mc-server`：完整 `mc-context` crate 要等 ≥2 个入口验证后再抽。

use mc_search::Hit;
use serde::{Deserialize, Serialize};

use crate::chat::Citation;

/// 默认预算：够塞几条带摘要的证据，又不会把提示词撑爆。
pub const DEFAULT_TOKEN_BUDGET: usize = 2_000;

/// 单条证据在包内的上限（字符估算前的硬截断）。
const MAX_SNIPPET_CHARS: usize = 800;

/// 粗估 token：按字符 / 4（中英混合够用；精确分词不是本切片目标）。
pub fn estimate_tokens(text: &str) -> usize {
    let chars = text.chars().count();
    chars.div_ceil(4).max(if chars == 0 { 0 } else { 1 })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextPackItem {
    pub document_id: String,
    pub title: String,
    pub kind: String,
    pub at: i64,
    pub snippet: String,
    pub score: f32,
    pub estimated_tokens: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextPack {
    pub query: String,
    pub items: Vec<ContextPackItem>,
    pub total_tokens: usize,
    pub budget: usize,
    /// 是否因预算或条数上限丢掉了后续命中。
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackOptions {
    pub token_budget: usize,
    pub max_items: usize,
}

impl Default for PackOptions {
    fn default() -> Self {
        Self {
            token_budget: DEFAULT_TOKEN_BUDGET,
            max_items: 12,
        }
    }
}

/// 从混合检索命中组装可用的上下文包。
///
/// 契约：按检索分排序贪心装入；每条至少保留标题；正文按剩余预算截断；
/// 产出的 citations 与 items 一一对应，可供 Chat 直接引用。
pub fn pack_from_hits(query: &str, hits: &[Hit], options: PackOptions) -> ContextPack {
    let budget = options.token_budget.max(1);
    let max_items = options.max_items.max(1);
    let mut items = Vec::new();
    let mut used = 0usize;
    let mut truncated = false;

    for hit in hits {
        if items.len() >= max_items {
            truncated = true;
            break;
        }

        let title = first_line(&hit.document.text);
        let title_tokens = estimate_tokens(&title);
        if used + title_tokens > budget && !items.is_empty() {
            truncated = true;
            break;
        }

        let remaining = budget.saturating_sub(used + title_tokens);
        // 标题本身就超预算时：仍放一条截断标题，保证「有依据」而不是空包。
        let (title, title_tokens, snippet, snippet_tokens) =
            if items.is_empty() && used + title_tokens > budget {
                let t = truncate_to_tokens(&title, budget);
                let tt = estimate_tokens(&t);
                (t, tt, String::new(), 0)
            } else {
                let body = body_after_title(&hit.document.text);
                let snippet = if remaining == 0 || body.is_empty() {
                    String::new()
                } else {
                    truncate_to_tokens(&body, remaining)
                };
                let st = estimate_tokens(&snippet);
                (title, title_tokens, snippet, st)
            };

        let estimated = title_tokens + snippet_tokens;
        used += estimated;
        items.push(ContextPackItem {
            document_id: hit.document.id.clone(),
            title,
            kind: hit.document.kind.as_str().to_string(),
            at: hit.document.at.as_millis(),
            snippet,
            score: hit.score,
            estimated_tokens: estimated,
        });
    }

    if hits.len() > items.len() {
        truncated = true;
    }

    ContextPack {
        query: query.to_string(),
        items,
        total_tokens: used,
        budget,
        truncated,
    }
}

/// ContextPack → 对话引用（含 snippet，供提示词组装）。
pub fn citations_from_pack(pack: &ContextPack) -> Vec<Citation> {
    pack.items
        .iter()
        .map(|item| Citation {
            document_id: item.document_id.clone(),
            title: item.title.clone(),
            kind: item.kind.clone(),
            at: item.at,
            snippet: item.snippet.clone(),
        })
        .collect()
}

/// 把包渲染成可直接塞进提示词 / MCP 工具结果的文本。
pub fn render_pack_text(pack: &ContextPack) -> String {
    if pack.items.is_empty() {
        return format!("查询「{}」没有可用的本地上下文。", pack.query);
    }
    let mut out = format!(
        "查询：{}\n证据（约 {} / {} tokens{}）：\n",
        pack.query,
        pack.total_tokens,
        pack.budget,
        if pack.truncated { "，已截断" } else { "" }
    );
    for (index, item) in pack.items.iter().enumerate() {
        out.push_str(&format!(
            "\n[{}] {}（{}）\n",
            index + 1,
            item.title,
            item.kind
        ));
        if !item.snippet.is_empty() {
            out.push_str(&item.snippet);
            out.push('\n');
        }
    }
    out
}

fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    line.chars().take(60).collect()
}

fn body_after_title(text: &str) -> String {
    let mut lines = text.lines();
    let _ = lines.next();
    let rest: String = lines.collect::<Vec<_>>().join("\n");
    let rest = rest.trim();
    if rest.is_empty() {
        // 单行文档：用标题后的剩余字符当摘要。
        let chars: Vec<char> = text.chars().collect();
        if chars.len() <= 60 {
            return String::new();
        }
        chars
            .into_iter()
            .skip(60)
            .take(MAX_SNIPPET_CHARS)
            .collect::<String>()
            .trim()
            .to_string()
    } else {
        rest.chars().take(MAX_SNIPPET_CHARS).collect()
    }
}

fn truncate_to_tokens(text: &str, token_budget: usize) -> String {
    if token_budget == 0 {
        return String::new();
    }
    // 预留省略号的字符，避免截断后再加「…」把预算撑破。
    let max_chars = token_budget.saturating_mul(4);
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_chars {
        return chars.into_iter().collect();
    }
    let keep = max_chars.saturating_sub(1).max(1);
    let mut out: String = chars.into_iter().take(keep).collect();
    out.push('…');
    // 若估算仍略超（中英混合边界），再削到预算内。
    while estimate_tokens(&out) > token_budget && out.chars().count() > 1 {
        let mut tmp: Vec<char> = out.chars().collect();
        tmp.pop();
        if tmp.last() == Some(&'…') {
            tmp.pop();
        }
        out = tmp.into_iter().collect();
        if !out.is_empty() {
            out.push('…');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_common::time::Timestamp;
    use mc_domain::activity::Provenance;
    use mc_search::{Document, DocumentKind, Hit};

    fn hit(id: &str, text: &str, score: f32) -> Hit {
        Hit {
            document: Document {
                id: id.into(),
                kind: DocumentKind::Activity,
                text: text.into(),
                at: Timestamp::from_millis(1_000),
                provenance: Provenance::Observed,
                blocked: false,
            },
            score,
            keyword_score: score,
            semantic_score: 0.0,
        }
    }

    #[test]
    fn packs_retrieval_hits_into_usable_context_with_citations() {
        let hits = vec![
            hit(
                "a1",
                "Fix auth token refresh\nOpened src/auth.rs and updated refresh flow before lunch.",
                0.9,
            ),
            hit(
                "a2",
                "Review PR #42\nLeft comments on error handling and asked for tests.",
                0.8,
            ),
        ];
        let pack = pack_from_hits("what was I doing on auth", &hits, PackOptions::default());
        assert_eq!(pack.items.len(), 2);
        assert!(!pack.items[0].snippet.is_empty());
        assert!(pack.total_tokens <= pack.budget);
        let citations = citations_from_pack(&pack);
        assert_eq!(citations.len(), 2);
        assert_eq!(citations[0].document_id, "a1");
        assert!(citations[0].snippet.contains("auth.rs"));
        let rendered = render_pack_text(&pack);
        assert!(rendered.contains("Fix auth token refresh"));
        assert!(rendered.contains("auth.rs"));
    }

    #[test]
    fn respects_token_budget_and_marks_truncated() {
        let long = "x".repeat(400);
        let hits: Vec<Hit> = (0..8)
            .map(|i| {
                hit(
                    &format!("id-{i}"),
                    &format!("Title {i}\n{long}"),
                    1.0 - (i as f32) * 0.01,
                )
            })
            .collect();
        let pack = pack_from_hits(
            "budget",
            &hits,
            PackOptions {
                token_budget: 120,
                max_items: 12,
            },
        );
        assert!(pack.total_tokens <= 120);
        assert!(pack.truncated);
        assert!(!pack.items.is_empty());
        assert!(pack.items.len() < hits.len());
    }
}
