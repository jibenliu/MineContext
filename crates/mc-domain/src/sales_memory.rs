//! 销售与客户跟进记忆（lite）：仅从本地窗口标题 / 笔记 / 文件名推断。
//!
//! **不做**云 CRM OAuth / SaaS 同步。承诺与下次跟进提示来自本地启发式。

use mc_common::time::Timestamp;
use serde::{Deserialize, Serialize};

/// 一条本地观测线索。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SalesObservation {
    pub id: String,
    pub text: String,
    pub at: Timestamp,
}

/// 客户 / 联系人时间线条目。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactEvent {
    pub contact_id: String,
    pub display_name: String,
    pub summary: String,
    pub source_id: String,
    pub at: Timestamp,
}

/// 对客户的承诺。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SalesCommitment {
    pub contact_id: String,
    pub text: String,
    pub source_id: String,
    pub at: Timestamp,
}

/// 下次跟进提示。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowUpHint {
    pub contact_id: String,
    pub hint: String,
    pub reason: String,
    pub at: Timestamp,
}

/// 拜访准备包（本地拼装）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisitPrepPack {
    pub contact_id: String,
    pub display_name: String,
    pub recent: Vec<ContactEvent>,
    pub commitments: Vec<SalesCommitment>,
    pub follow_ups: Vec<FollowUpHint>,
    pub prep_notes: String,
}

/// 从本地观测构建客户时间线。
pub fn build_contact_timeline(obs: &[SalesObservation]) -> Vec<ContactEvent> {
    let mut out = Vec::new();
    for item in obs {
        if let Some((contact_id, display_name)) = detect_contact(&item.text) {
            out.push(ContactEvent {
                contact_id,
                display_name,
                summary: item.text.trim().to_string(),
                source_id: item.id.clone(),
                at: item.at,
            });
        }
    }
    out.sort_by(|a, b| {
        b.at.as_millis()
            .cmp(&a.at.as_millis())
            .then_with(|| a.contact_id.cmp(&b.contact_id))
    });
    out
}

/// 抽取对客户的承诺语句。
pub fn extract_commitments(obs: &[SalesObservation]) -> Vec<SalesCommitment> {
    let mut out = Vec::new();
    for item in obs {
        let Some((contact_id, _)) = detect_contact(&item.text) else {
            continue;
        };
        let lower = item.text.to_lowercase();
        if is_commitment(&lower) {
            out.push(SalesCommitment {
                contact_id,
                text: item.text.trim().to_string(),
                source_id: item.id.clone(),
                at: item.at,
            });
        }
    }
    out
}

/// 根据时间线与承诺生成下次跟进提示。
pub fn suggest_follow_ups(
    timeline: &[ContactEvent],
    commitments: &[SalesCommitment],
    now: Timestamp,
) -> Vec<FollowUpHint> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for c in commitments {
        if !seen.insert(c.contact_id.clone()) {
            continue;
        }
        out.push(FollowUpHint {
            contact_id: c.contact_id.clone(),
            hint: format!("跟进承诺：{}", truncate(&c.text, 80)),
            reason: "open_commitment".into(),
            at: now,
        });
    }
    // 超过 7 天未互动的客户给一条回访提示
    const WEEK_MS: i64 = 7 * 24 * 60 * 60 * 1000;
    let mut last_by: std::collections::BTreeMap<String, &ContactEvent> =
        std::collections::BTreeMap::new();
    for ev in timeline {
        last_by
            .entry(ev.contact_id.clone())
            .and_modify(|prev| {
                if ev.at.as_millis() > prev.at.as_millis() {
                    *prev = ev;
                }
            })
            .or_insert(ev);
    }
    for (contact_id, ev) in last_by {
        if now.as_millis() - ev.at.as_millis() < WEEK_MS {
            continue;
        }
        if seen.contains(&contact_id) {
            continue;
        }
        out.push(FollowUpHint {
            contact_id: contact_id.clone(),
            hint: format!("超过一周未联系 {}，建议回访", ev.display_name),
            reason: "stale_contact".into(),
            at: now,
        });
    }
    out
}

/// 为指定客户拼装拜访准备包。
pub fn build_visit_prep(
    contact_id: &str,
    timeline: &[ContactEvent],
    commitments: &[SalesCommitment],
    follow_ups: &[FollowUpHint],
) -> Option<VisitPrepPack> {
    let recent: Vec<_> = timeline
        .iter()
        .filter(|e| e.contact_id == contact_id)
        .take(8)
        .cloned()
        .collect();
    if recent.is_empty() {
        return None;
    }
    let display_name = recent[0].display_name.clone();
    let commitments: Vec<_> = commitments
        .iter()
        .filter(|c| c.contact_id == contact_id)
        .cloned()
        .collect();
    let follow_ups: Vec<_> = follow_ups
        .iter()
        .filter(|f| f.contact_id == contact_id)
        .cloned()
        .collect();
    let mut prep_notes = format!("拜访准备：{}\n\n最近互动：\n", display_name);
    for ev in &recent {
        prep_notes.push_str(&format!("- {}\n", ev.summary));
    }
    if !commitments.is_empty() {
        prep_notes.push_str("\n未完成承诺：\n");
        for c in &commitments {
            prep_notes.push_str(&format!("- {}\n", c.text));
        }
    }
    Some(VisitPrepPack {
        contact_id: contact_id.to_string(),
        display_name,
        recent,
        commitments,
        follow_ups,
        prep_notes,
    })
}

fn detect_contact(text: &str) -> Option<(String, String)> {
    // 显式标签：客户:Acme / Customer: Acme
    let lower = text.to_lowercase();
    for prefix in ["客户:", "客户：", "customer:", "client:", "account:"] {
        if let Some(idx) = lower.find(prefix) {
            let rest = lower[idx + prefix.len()..].trim();
            let name = rest
                .split(['|', '-', '—', '/'])
                .next()
                .unwrap_or(rest)
                .trim();
            if name.chars().count() >= 2 {
                return Some((slug(name), title_case(name)));
            }
        }
    }
    // 窗口标题常见形态：Mail - Jane@acme.com / Zoom Meeting with Contoso
    if let Some(email) = extract_email(text) {
        let domain = email.split('@').nth(1).unwrap_or("contact");
        let company = domain.split('.').next().unwrap_or(domain);
        if company.len() >= 2 && company != "gmail" && company != "outlook" {
            return Some((slug(company), title_case(company)));
        }
    }
    for marker in ["meeting with ", "call with ", "拜访 ", "会见 "] {
        if let Some(idx) = lower.find(marker) {
            let rest = text[idx + marker.len()..].trim();
            let name = rest.split(['-', '|', '—']).next().unwrap_or(rest).trim();
            if name.chars().count() >= 2 {
                return Some((slug(name), name.to_string()));
            }
        }
    }
    None
}

fn is_commitment(lower: &str) -> bool {
    const KEYS: &[&str] = &[
        "will send",
        "promise",
        "follow up",
        "send proposal",
        "承诺",
        "下周给",
        "发方案",
        "报价",
        "demo on",
    ];
    KEYS.iter().any(|k| lower.contains(k))
}

fn extract_email(text: &str) -> Option<String> {
    for token in text.split_whitespace() {
        let cleaned = token.trim_matches(|c: char| {
            !c.is_ascii_alphanumeric() && c != '@' && c != '.' && c != '_' && c != '-'
        });
        if cleaned.contains('@') && cleaned.contains('.') {
            return Some(cleaned.to_lowercase());
        }
    }
    None
}

fn slug(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else if ('\u{4e00}'..='\u{9fff}').contains(&c) {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn title_case(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return trimmed.to_string();
    }
    let mut chars = trimmed.chars();
    let first = chars.next().unwrap().to_uppercase().to_string();
    format!("{first}{}", chars.as_str())
}

fn truncate(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    format!(
        "{}…",
        s.chars().take(max.saturating_sub(1)).collect::<String>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(id: &str, text: &str, at: i64) -> SalesObservation {
        SalesObservation {
            id: id.into(),
            text: text.into(),
            at: Timestamp::from_millis(at),
        }
    }

    #[test]
    fn builds_timeline_commitments_and_visit_prep_from_local_signals() {
        let observations = vec![
            obs("1", "Zoom Meeting with Contoso — Q3 pipeline", 1_000),
            obs("2", "客户:Contoso | 承诺下周给报价方案 follow up", 2_000),
            obs("3", "Mail - jane@contoso.com — contract draft", 3_000),
            obs("4", "Browsing weather", 4_000),
        ];
        let timeline = build_contact_timeline(&observations);
        assert!(
            timeline.iter().any(|e| e.contact_id.contains("contoso")),
            "{timeline:?}"
        );
        let commitments = extract_commitments(&observations);
        assert!(!commitments.is_empty(), "{commitments:?}");
        let hints = suggest_follow_ups(&timeline, &commitments, Timestamp::from_millis(3_000));
        assert!(hints.iter().any(|h| h.contact_id.contains("contoso")));
        let pack =
            build_visit_prep("contoso", &timeline, &commitments, &hints).expect("visit prep");
        assert!(pack.prep_notes.contains("Contoso") || pack.prep_notes.contains("contoso"));
        assert!(!pack.recent.is_empty());
    }
}
