//! 从本地活动 / 总结收集只读文本，供商业方向 3–6 的启发式抽取共用。
//!
//! **不改写 Observation**；不出网。

use mc_common::error::AppError;
use mc_common::time::Timestamp;
use mc_storage::Database;

#[derive(Debug, Clone)]
pub struct LocalText {
    pub id: String,
    pub kind: String,
    pub text: String,
    pub at: Timestamp,
}

/// 活动标题 + 总结正文（本地派生数据）。
pub fn collect_local_texts(db: &Database) -> Result<Vec<LocalText>, AppError> {
    let mut out = Vec::new();
    let activities = mc_storage::projectors::activities::read_all(db)?;
    for activity in activities {
        out.push(LocalText {
            id: activity.id,
            kind: "activity".into(),
            text: activity.title,
            at: activity.end,
        });
    }
    let summaries = db.read_summaries(None, None)?;
    for summary in summaries {
        let text = if summary.body_markdown.trim().is_empty() {
            summary.title.clone()
        } else {
            format!("{}\n{}", summary.title, summary.body_markdown)
        };
        out.push(LocalText {
            id: summary.id,
            kind: "summary".into(),
            text,
            at: summary.end,
        });
    }
    Ok(out)
}
