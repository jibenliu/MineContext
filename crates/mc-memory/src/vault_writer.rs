//! 把报告归档进笔记树。
//!
//! 归档的目标是**前端的笔记树能直接看到**：
//! - 日报用 `document_type='DailyReport'`（现有枚举里的值）；
//! - 周报用 `'vaults'` + 标签 `weekly`（枚举里没有 WeeklyReport 类型，
//!   硬造一个值会让 UI 按类型过滤时漏掉它）；
//! - 两者都挂在 `Summary` 文件夹下。
//!
//! 形状对齐是这一节的全部意义：对不齐就是白做。

use mc_common::error::AppError;
use mc_common::observability::info;
use mc_common::time::Timestamp;
use mc_storage::projectors::summaries::StoredSummary;
use mc_storage::vaults::{
    VaultDocument, DOCUMENT_TYPE_DAILY_REPORT, DOCUMENT_TYPE_VAULTS, FOLDER_SUMMARY,
};
use mc_storage::Database;

/// 列表里显示的摘要长度。太长会把笔记树行撑爆，太短等于没有。
const SUMMARY_CHARS: usize = 120;

pub fn archive_daily_report(
    db: &Database,
    summary: &StoredSummary,
    at: Timestamp,
) -> Result<i64, AppError> {
    archive(db, summary, DOCUMENT_TYPE_DAILY_REPORT, &[], at)
}

pub fn archive_weekly_report(
    db: &Database,
    summary: &StoredSummary,
    at: Timestamp,
) -> Result<i64, AppError> {
    // 旧枚举没有 WeeklyReport：用标签区分，别造新类型
    let tags = vec!["weekly".to_string(), "report".to_string()];
    archive(db, summary, DOCUMENT_TYPE_VAULTS, &tags, at)
}

fn archive(
    db: &Database,
    summary: &StoredSummary,
    document_type: &str,
    tags: &[String],
    at: Timestamp,
) -> Result<i64, AppError> {
    let folder = db.ensure_folder(FOLDER_SUMMARY, at)?;
    let row = db.upsert_vault_document(
        document_type,
        &summary.title,
        &brief(&summary.body_markdown),
        &summary.body_markdown,
        tags,
        Some(folder),
        at,
    )?;
    // 只记 id 与类型：标题与正文是用户内容。
    info!(
        component = "memory",
        event = "report_archived",
        document_type,
        id = row,
        "报告已归档进笔记树"
    );
    Ok(row)
}

/// 摘要：取正文去掉 markdown 标题后的第一段，截断到 `SUMMARY_CHARS`。
fn brief(body: &str) -> String {
    let text = body
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect::<Vec<_>>()
        .join(" ");

    if text.chars().count() <= SUMMARY_CHARS {
        return text;
    }
    let mut brief: String = text.chars().take(SUMMARY_CHARS).collect();
    brief.push('…');
    brief
}

/// 笔记树里已归档的报告（按类型）。
pub fn archived_reports(
    db: &Database,
    document_type: &str,
) -> Result<Vec<VaultDocument>, AppError> {
    db.read_vault_documents(&[document_type, DOCUMENT_TYPE_VAULTS])
}
