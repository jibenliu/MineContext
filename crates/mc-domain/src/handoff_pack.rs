//! 企业知识沉淀与交接（lite）：仅打包**用户确认过**的运维笔记 / 事故案例 / 决策。
//!
//! 导出为本地 pack（JSON/Markdown）。**不做** SSO、团队多租户云、计费。

use mc_common::time::Timestamp;
use serde::{Deserialize, Serialize};

pub const KIND_HANDOFF_CONFIRMED: &str = "handoff.confirmed";
pub const PACK_FORMAT: &str = "minecontext-handoff-pack";
pub const PACK_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffKind {
    OpsNote,
    Incident,
    Decision,
}

impl HandoffKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpsNote => "ops_note",
            Self::Incident => "incident",
            Self::Decision => "decision",
        }
    }

    pub fn label_zh(self) -> &'static str {
        match self {
            Self::OpsNote => "运维笔记",
            Self::Incident => "事故案例",
            Self::Decision => "决策",
        }
    }
}

/// 候选记忆（尚未确认则不可进交接包）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffCandidate {
    pub id: String,
    pub kind: HandoffKind,
    pub title: String,
    pub body: String,
    pub source_id: String,
    pub at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffConfirmation {
    pub candidate_id: String,
    pub at: Timestamp,
    pub by: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffItem {
    pub id: String,
    pub kind: HandoffKind,
    pub title: String,
    pub body: String,
    pub source_id: String,
    pub at: Timestamp,
    pub confirmed_at: Timestamp,
    pub confirmed_by: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffPackManifest {
    pub format: String,
    pub schema_version: u32,
    pub exported_at_ms: i64,
    pub item_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffPack {
    pub manifest: HandoffPackManifest,
    pub items: Vec<HandoffItem>,
    pub markdown: String,
}

/// 从本地文本启发式提出交接候选（需用户确认后才进包）。
pub fn propose_candidates(sources: &[(String, String, Timestamp)]) -> Vec<HandoffCandidate> {
    let mut out = Vec::new();
    for (id, text, at) in sources {
        let lower = text.to_lowercase();
        let kind = if matches_incident(&lower) {
            HandoffKind::Incident
        } else if matches_decision(&lower) {
            HandoffKind::Decision
        } else if matches_ops(&lower) {
            HandoffKind::OpsNote
        } else {
            continue;
        };
        let title = first_line(text);
        out.push(HandoffCandidate {
            id: format!("cand-{}", id),
            kind,
            title,
            body: text.trim().to_string(),
            source_id: id.clone(),
            at: *at,
        });
    }
    out
}

/// 只保留已确认候选。
pub fn confirmed_items(
    candidates: &[HandoffCandidate],
    confirmations: &[HandoffConfirmation],
) -> Vec<HandoffItem> {
    let mut by_id: std::collections::BTreeMap<&str, &HandoffConfirmation> =
        std::collections::BTreeMap::new();
    for c in confirmations {
        by_id.insert(c.candidate_id.as_str(), c);
    }
    let mut out = Vec::new();
    for cand in candidates {
        if let Some(conf) = by_id.get(cand.id.as_str()) {
            out.push(HandoffItem {
                id: cand.id.clone(),
                kind: cand.kind,
                title: cand.title.clone(),
                body: cand.body.clone(),
                source_id: cand.source_id.clone(),
                at: cand.at,
                confirmed_at: conf.at,
                confirmed_by: conf.by.clone(),
            });
        }
    }
    out.sort_by(|a, b| b.confirmed_at.as_millis().cmp(&a.confirmed_at.as_millis()));
    out
}

/// 导出本地交接包（Markdown + 结构化清单）。
pub fn export_pack(items: &[HandoffItem], at: Timestamp) -> HandoffPack {
    let manifest = HandoffPackManifest {
        format: PACK_FORMAT.into(),
        schema_version: PACK_SCHEMA_VERSION,
        exported_at_ms: at.as_millis(),
        item_count: items.len(),
    };
    let mut markdown = String::from("# 交接包（本地）\n\n");
    markdown.push_str("本包仅含用户确认过的记忆条目；无团队云、无 SSO。\n\n");
    for kind in [HandoffKind::Decision, HandoffKind::Incident, HandoffKind::OpsNote] {
        let section: Vec<_> = items.iter().filter(|i| i.kind == kind).collect();
        if section.is_empty() {
            continue;
        }
        markdown.push_str(&format!("## {}\n\n", kind.label_zh()));
        for item in section {
            markdown.push_str(&format!(
                "### {}\n\n{}\n\n_确认于 {}_ by {}\n\n",
                item.title,
                item.body,
                item.confirmed_at.as_millis(),
                item.confirmed_by
            ));
        }
    }
    if items.is_empty() {
        markdown.push_str("（空包：请先确认候选条目）\n");
    }
    HandoffPack {
        manifest,
        items: items.to_vec(),
        markdown,
    }
}

fn matches_incident(lower: &str) -> bool {
    const KEYS: &[&str] = &[
        "incident",
        "outage",
        "事故",
        "故障",
        "postmortem",
        "复盘",
        "sev",
    ];
    KEYS.iter().any(|k| lower.contains(k))
}

fn matches_decision(lower: &str) -> bool {
    const KEYS: &[&str] = &[
        "decision",
        "decided",
        "adr",
        "决策",
        "决议",
        "选定",
        "adopt",
    ];
    KEYS.iter().any(|k| lower.contains(k))
}

fn matches_ops(lower: &str) -> bool {
    const KEYS: &[&str] = &[
        "runbook",
        "ops note",
        "运维",
        "值班",
        "oncall",
        "playbook",
        "操作手册",
    ];
    KEYS.iter().any(|k| lower.contains(k))
}

fn first_line(text: &str) -> String {
    text.lines()
        .next()
        .unwrap_or(text)
        .chars()
        .take(80)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_confirmed_memory_enters_export_pack() {
        let at = Timestamp::from_millis(1_000);
        let candidates = propose_candidates(&[
            ("a1".into(), "事故：API 网关超时，回滚到 v1.0.6".into(), at),
            ("a2".into(), "决策：选定本地 SQLite 作为 vault".into(), at),
            ("a3".into(), "运维：值班 runbook 重启 daemon".into(), at),
            ("a4".into(), "随便看看天气".into(), at),
        ]);
        assert_eq!(candidates.len(), 3, "{candidates:?}");
        // 未确认 → 空包
        let empty = confirmed_items(&candidates, &[]);
        assert!(empty.is_empty());
        let pack_empty = export_pack(&empty, Timestamp::from_millis(2_000));
        assert_eq!(pack_empty.manifest.item_count, 0);
        assert_eq!(pack_empty.manifest.format, PACK_FORMAT);

        let confirmations = vec![
            HandoffConfirmation {
                candidate_id: candidates[0].id.clone(),
                at: Timestamp::from_millis(2_000),
                by: "user".into(),
            },
            HandoffConfirmation {
                candidate_id: candidates[1].id.clone(),
                at: Timestamp::from_millis(2_100),
                by: "user".into(),
            },
        ];
        let items = confirmed_items(&candidates, &confirmations);
        assert_eq!(items.len(), 2);
        let pack = export_pack(&items, Timestamp::from_millis(3_000));
        assert_eq!(pack.manifest.item_count, 2);
        assert!(pack.markdown.contains("交接包"));
        assert!(pack.markdown.contains("事故") || pack.markdown.contains("决策"));
        // 未确认的运维笔记不得出现
        assert!(!pack.items.iter().any(|i| i.kind == HandoffKind::OpsNote));
    }
}
