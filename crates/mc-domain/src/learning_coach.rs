//! AI 学习轨迹与知识教练（lite）：从本地活动推断学习主题与反复卡住的模式，
//! 并生成间隔复习计划。全部本地，不依赖云端课程平台。

use mc_common::time::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearningObservation {
    pub id: String,
    pub text: String,
    pub at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearningTopic {
    pub topic_id: String,
    pub label: String,
    pub hit_count: usize,
    pub last_at: Timestamp,
    pub source_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StuckPattern {
    pub pattern_id: String,
    pub label: String,
    pub repeats: usize,
    pub last_at: Timestamp,
    pub hint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewItem {
    pub topic_id: String,
    pub label: String,
    pub due_at: Timestamp,
    pub interval_days: u32,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpacedReviewPlan {
    pub generated_at: Timestamp,
    pub items: Vec<ReviewItem>,
    pub summary: String,
}

/// 检测最近学习主题（文档 / 教程 / 课程 / 技术关键词）。
pub fn detect_topics(obs: &[LearningObservation]) -> Vec<LearningTopic> {
    let mut buckets: std::collections::BTreeMap<String, LearningTopic> =
        std::collections::BTreeMap::new();
    for item in obs {
        if let Some((topic_id, label)) = classify_topic(&item.text) {
            let entry = buckets.entry(topic_id.clone()).or_insert_with(|| LearningTopic {
                topic_id: topic_id.clone(),
                label: label.clone(),
                hit_count: 0,
                last_at: item.at,
                source_ids: Vec::new(),
            });
            entry.hit_count += 1;
            entry.source_ids.push(item.id.clone());
            if item.at.as_millis() >= entry.last_at.as_millis() {
                entry.last_at = item.at;
                entry.label = label;
            }
        }
    }
    let mut out: Vec<_> = buckets.into_values().collect();
    out.sort_by(|a, b| {
        b.hit_count
            .cmp(&a.hit_count)
            .then_with(|| b.last_at.as_millis().cmp(&a.last_at.as_millis()))
    });
    out
}

/// 检测反复卡住的模式（同一错误 / 同一文档反复打开）。
pub fn detect_stuck_patterns(obs: &[LearningObservation]) -> Vec<StuckPattern> {
    let mut counts: std::collections::BTreeMap<String, (String, usize, Timestamp, Vec<String>)> =
        std::collections::BTreeMap::new();
    for item in obs {
        if let Some((pattern_id, label)) = classify_stuck(&item.text) {
            let entry = counts.entry(pattern_id.clone()).or_insert_with(|| {
                (label.clone(), 0, item.at, Vec::new())
            });
            entry.1 += 1;
            entry.3.push(item.id.clone());
            if item.at.as_millis() >= entry.2.as_millis() {
                entry.2 = item.at;
                entry.0 = label;
            }
        }
    }
    let mut out = Vec::new();
    for (pattern_id, (label, repeats, last_at, _ids)) in counts {
        if repeats < 2 {
            continue;
        }
        out.push(StuckPattern {
            pattern_id,
            label: label.clone(),
            repeats,
            last_at,
            hint: format!("反复遇到「{label}」{repeats} 次，建议专题复盘"),
        });
    }
    out.sort_by(|a, b| b.repeats.cmp(&a.repeats));
    out
}

/// 生成间隔复习计划（1 / 3 / 7 天节奏，本地确定性）。
pub fn build_spaced_review_plan(
    topics: &[LearningTopic],
    stuck: &[StuckPattern],
    now: Timestamp,
) -> SpacedReviewPlan {
    let day_ms = 24 * 60 * 60 * 1000i64;
    let mut items = Vec::new();
    for topic in topics.iter().take(8) {
        let intervals = if topic.hit_count >= 3 {
            [1u32, 3, 7]
        } else {
            [1, 2, 5]
        };
        for interval in intervals {
            items.push(ReviewItem {
                topic_id: topic.topic_id.clone(),
                label: topic.label.clone(),
                due_at: Timestamp::from_millis(now.as_millis() + i64::from(interval) * day_ms),
                interval_days: interval,
                reason: format!("hit_count={}", topic.hit_count),
            });
        }
    }
    for pattern in stuck.iter().take(4) {
        items.push(ReviewItem {
            topic_id: pattern.pattern_id.clone(),
            label: format!("卡住复盘：{}", pattern.label),
            due_at: Timestamp::from_millis(now.as_millis() + day_ms),
            interval_days: 1,
            reason: format!("stuck_repeats={}", pattern.repeats),
        });
    }
    items.sort_by(|a, b| a.due_at.as_millis().cmp(&b.due_at.as_millis()));
    let summary = if items.is_empty() {
        "暂无学习主题，继续记录本地活动后可生成复习计划。".to_string()
    } else {
        format!(
            "共 {} 个复习项（{} 个主题，{} 个卡住模式）。",
            items.len(),
            topics.len().min(8),
            stuck.len().min(4)
        )
    };
    SpacedReviewPlan {
        generated_at: now,
        items,
        summary,
    }
}

fn classify_topic(text: &str) -> Option<(String, String)> {
    let lower = text.to_lowercase();
    const RULES: &[(&str, &str, &str)] = &[
        ("rust", "rust", "Rust"),
        ("typescript", "typescript", "TypeScript"),
        ("react", "react", "React"),
        ("kubernetes", "kubernetes", "Kubernetes"),
        ("k8s", "kubernetes", "Kubernetes"),
        ("llm", "llm", "LLM"),
        ("prompt", "prompt-engineering", "Prompt engineering"),
        ("教程", "tutorial", "教程"),
        ("tutorial", "tutorial", "Tutorial"),
        ("course", "course", "Course"),
        ("文档", "docs", "文档"),
        ("docs.", "docs", "Docs"),
        ("stackoverflow", "stackoverflow", "Stack Overflow"),
        ("leetcode", "leetcode", "LeetCode"),
    ];
    for (needle, id, label) in RULES {
        if lower.contains(needle) {
            return Some(((*id).into(), (*label).into()));
        }
    }
    None
}

fn classify_stuck(text: &str) -> Option<(String, String)> {
    let lower = text.to_lowercase();
    const RULES: &[(&str, &str, &str)] = &[
        ("error:", "error-generic", "编译/运行错误"),
        ("panic", "rust-panic", "Rust panic"),
        ("typeerror", "ts-typeerror", "TypeError"),
        ("undefined is not", "js-undefined", "undefined 错误"),
        ("permission denied", "permission", "权限问题"),
        ("timeout", "timeout", "超时"),
        ("卡住", "stuck-zh", "卡住"),
        ("不会", "stuck-zh", "不会"),
        ("debug", "debug-loop", "反复调试"),
    ];
    for (needle, id, label) in RULES {
        if lower.contains(needle) {
            return Some(((*id).into(), (*label).into()));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(id: &str, text: &str, at: i64) -> LearningObservation {
        LearningObservation {
            id: id.into(),
            text: text.into(),
            at: Timestamp::from_millis(at),
        }
    }

    #[test]
    fn detects_topics_stuck_patterns_and_builds_spaced_plan() {
        let observations = vec![
            obs("1", "Reading Rust async tutorial", 1_000),
            obs("2", "Rust Book — ownership", 2_000),
            obs("3", "error: borrow checker panic in main", 3_000),
            obs("4", "error: borrow checker again", 4_000),
            obs("5", "Watching React course hooks", 5_000),
            obs("6", "Browsing weather", 6_000),
        ];
        let topics = detect_topics(&observations);
        assert!(topics.iter().any(|t| t.topic_id == "rust"), "{topics:?}");
        assert!(topics.iter().any(|t| t.topic_id == "react"));
        let stuck = detect_stuck_patterns(&observations);
        assert!(
            stuck.iter().any(|p| p.repeats >= 2),
            "expected repeated stuck pattern: {stuck:?}"
        );
        let plan = build_spaced_review_plan(&topics, &stuck, Timestamp::from_millis(10_000));
        assert!(!plan.items.is_empty());
        assert!(plan.summary.contains("复习"));
        // 间隔应递增（命中 <3 时用 1/2/5）
        let rust_intervals: Vec<_> = plan
            .items
            .iter()
            .filter(|i| i.topic_id == "rust")
            .map(|i| i.interval_days)
            .collect();
        assert!(
            rust_intervals.contains(&1) && rust_intervals.contains(&2),
            "{rust_intervals:?}"
        );
    }
}
