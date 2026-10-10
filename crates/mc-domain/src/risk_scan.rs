//! 项目风险与遗漏发现：从本地活动 / 阶段 / 总结文本抽取承诺、待确认、等待与未决项。
//!
//! 纯启发式、可纠正；不改写 Observation。误报可 dismiss，重放后仍被压制。

use mc_common::time::Timestamp;
use serde::{Deserialize, Serialize};

pub const KIND_RISK_DISMISSED: &str = "risk.dismissed";

/// 风险类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskKind {
    Commitment,
    OpenQuestion,
    WaitingOn,
    Unresolved,
}

impl RiskKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Commitment => "commitment",
            Self::OpenQuestion => "open_question",
            Self::WaitingOn => "waiting_on",
            Self::Unresolved => "unresolved",
        }
    }

    pub fn label_zh(self) -> &'static str {
        match self {
            Self::Commitment => "承诺",
            Self::OpenQuestion => "待确认",
            Self::WaitingOn => "等待中",
            Self::Unresolved => "未决/遗漏",
        }
    }
}

/// 供抽取的本地文本源（活动标题、阶段摘要、总结正文）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskTextSource {
    pub id: String,
    pub kind: String,
    pub text: String,
    pub at: Timestamp,
}

/// 一条风险/遗漏发现。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RiskFinding {
    pub id: String,
    pub kind: RiskKind,
    pub text: String,
    pub source_id: String,
    pub source_kind: String,
    pub at: Timestamp,
    /// 0..=100；质量门会丢掉过低分。
    pub score: u8,
}

/// 用户纠正：压制误报（事件追加，可重放）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RiskDismissed {
    pub finding_id: String,
    pub at: Timestamp,
}

/// 从文本源抽取风险；过滤过短 / 纯噪声，避免显然误报。
pub fn extract_risks(sources: &[RiskTextSource]) -> Vec<RiskFinding> {
    let mut out = Vec::new();
    for source in sources {
        for (idx, line) in split_lines(&source.text).into_iter().enumerate() {
            if let Some(finding) = classify_line(source, &line, idx) {
                if passes_quality_gate(&finding) {
                    out.push(finding);
                }
            }
        }
    }
    out.sort_by(|a, b| {
        b.at.as_millis()
            .cmp(&a.at.as_millis())
            .then_with(|| a.kind.as_str().cmp(b.kind.as_str()))
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

/// 应用 dismiss 纠正：被压制的 finding_id 不再出现。
pub fn apply_dismissals(findings: &[RiskFinding], dismissed: &[RiskDismissed]) -> Vec<RiskFinding> {
    let suppressed: std::collections::HashSet<&str> =
        dismissed.iter().map(|d| d.finding_id.as_str()).collect();
    findings
        .iter()
        .filter(|f| !suppressed.contains(f.id.as_str()))
        .cloned()
        .collect()
}

/// 进度 / 阻塞风格报告（本地 Markdown，不出网）。
pub fn render_progress_report(findings: &[RiskFinding]) -> String {
    let mut md = String::from("# 项目风险与遗漏\n\n");
    if findings.is_empty() {
        md.push_str("当前未发现待处理的承诺、待确认、等待或未决项。\n");
        return md;
    }
    for kind in [
        RiskKind::Commitment,
        RiskKind::WaitingOn,
        RiskKind::OpenQuestion,
        RiskKind::Unresolved,
    ] {
        let items: Vec<_> = findings.iter().filter(|f| f.kind == kind).collect();
        if items.is_empty() {
            continue;
        }
        md.push_str(&format!("## {}\n\n", kind.label_zh()));
        for item in items {
            md.push_str(&format!("- {} （来源 `{}`）\n", item.text, item.source_id));
        }
        md.push('\n');
    }
    md
}

fn split_lines(text: &str) -> Vec<String> {
    text.split(['\n', ';', '；'])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn classify_line(source: &RiskTextSource, line: &str, idx: usize) -> Option<RiskFinding> {
    let lower = line.to_lowercase();
    let kind = if matches_waiting(&lower) {
        RiskKind::WaitingOn
    } else if matches_open_question(line, &lower) {
        RiskKind::OpenQuestion
    } else if matches_unresolved(&lower) {
        RiskKind::Unresolved
    } else if matches_commitment(&lower) {
        RiskKind::Commitment
    } else {
        return None;
    };
    let score = score_line(line, kind);
    let id = format!("{}:{}:{}", source.id, kind.as_str(), idx);
    Some(RiskFinding {
        id,
        kind,
        text: line.to_string(),
        source_id: source.id.clone(),
        source_kind: source.kind.clone(),
        at: source.at,
        score,
    })
}

fn matches_commitment(lower: &str) -> bool {
    const KEYS: &[&str] = &[
        "will ",
        "need to",
        "todo",
        "deadline",
        "by friday",
        "commit to",
        "承诺",
        "要做",
        "截止",
        "交付",
        "下周完成",
        "必须",
    ];
    KEYS.iter().any(|k| lower.contains(k))
}

fn matches_open_question(line: &str, lower: &str) -> bool {
    if line.contains('?') || line.contains('？') {
        return true;
    }
    const KEYS: &[&str] = &[
        "tbd",
        "unclear",
        "open question",
        "待确认",
        "待定",
        "是否",
        "怎么",
        "如何",
        "不清楚",
    ];
    KEYS.iter().any(|k| lower.contains(k))
}

fn matches_waiting(lower: &str) -> bool {
    const KEYS: &[&str] = &[
        "waiting on",
        "waiting for",
        "blocked by",
        "blocked on",
        "依赖",
        "等待",
        "等对方",
        "卡在",
        "blocked",
    ];
    KEYS.iter().any(|k| lower.contains(k))
}

fn matches_unresolved(lower: &str) -> bool {
    const KEYS: &[&str] = &[
        "fixme",
        "unresolved",
        "未解决",
        "遗漏",
        "forgot",
        "missed",
        "漏了",
        "还没",
        "未闭环",
    ];
    KEYS.iter().any(|k| lower.contains(k))
}

fn score_line(line: &str, kind: RiskKind) -> u8 {
    let len = line.chars().count();
    let mut score: u16 = 40;
    if len >= 12 {
        score += 20;
    }
    if len >= 24 {
        score += 15;
    }
    match kind {
        RiskKind::WaitingOn | RiskKind::Unresolved => score += 10,
        RiskKind::Commitment => score += 5,
        RiskKind::OpenQuestion => {}
    }
    score.min(100) as u8
}

/// 质量门：丢掉过短、纯符号、或分数过低的候选。
pub fn passes_quality_gate(finding: &RiskFinding) -> bool {
    let text = finding.text.trim();
    if text.chars().count() < 6 {
        return false;
    }
    if finding.score < 45 {
        return false;
    }
    // 纯问号 / 省略号不算遗漏
    let meaningful: String = text
        .chars()
        .filter(|c| c.is_alphanumeric() || ('\u{4e00}'..='\u{9fff}').contains(c))
        .collect();
    if meaningful.chars().count() < 3 {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(id: &str, text: &str, at: i64) -> RiskTextSource {
        RiskTextSource {
            id: id.into(),
            kind: "activity".into(),
            text: text.into(),
            at: Timestamp::from_millis(at),
        }
    }

    #[test]
    fn extracts_commitments_questions_waiting_and_unresolved() {
        let sources = vec![
            src(
                "a1",
                "Need to finish auth refresh by Friday; waiting on design review",
                1_000,
            ),
            src("a2", "Open question: 是否迁移到 vault v2？", 2_000),
            src("a3", "遗漏了错误重试；FIXME timeout path 未闭环", 3_000),
            src("noise", "??", 4_000),
            src("noise2", "ok", 5_000),
        ];
        let findings = extract_risks(&sources);
        assert!(
            findings
                .iter()
                .any(|f| f.kind == RiskKind::Commitment && f.text.to_lowercase().contains("friday")),
            "{findings:?}"
        );
        assert!(findings.iter().any(|f| f.kind == RiskKind::WaitingOn));
        assert!(findings.iter().any(|f| f.kind == RiskKind::OpenQuestion));
        assert!(findings.iter().any(|f| f.kind == RiskKind::Unresolved));
        assert!(
            !findings
                .iter()
                .any(|f| f.source_id == "noise" || f.source_id == "noise2"),
            "quality gate should drop noise: {findings:?}"
        );
    }

    #[test]
    fn dismiss_is_replay_safe_and_corrects_false_positives() {
        let sources = vec![src(
            "a1",
            "Need to ship release notes; waiting on legal",
            1_000,
        )];
        let findings = extract_risks(&sources);
        assert!(!findings.is_empty());
        let target = findings[0].id.clone();
        let dismissed = vec![RiskDismissed {
            finding_id: target.clone(),
            at: Timestamp::from_millis(2_000),
        }];
        let once = apply_dismissals(&findings, &dismissed);
        let twice = apply_dismissals(&findings, &dismissed);
        assert_eq!(once, twice);
        assert!(!once.iter().any(|f| f.id == target));
    }

    #[test]
    fn progress_report_lists_blockers_by_kind() {
        let findings = extract_risks(&[src(
            "s1",
            "承诺下周完成 MCP；waiting on API key；是否需要多租户？；遗漏了回滚说明",
            10,
        )]);
        let report = render_progress_report(&findings);
        assert!(report.contains("项目风险与遗漏"));
        assert!(report.contains("承诺") || report.contains("等待"));
    }
}
