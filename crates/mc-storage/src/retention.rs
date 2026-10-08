//! 保留策略的执行：轮转删除 + 引用清理。
//!
//! 删除逻辑本身在 [`crate::blob`]（`enforce_retention`）里，这一层补上
//! 只有「一起做」才能成立的那半件事：**删文件的同时清掉数据库引用** ——
//! 只删文件会留下悬空 `image_path`（界面上是一片破图），只清引用则磁盘泄漏。
//!
//! 因此这是一个整体操作：先按策略删文件，再用一次目录遍历的结果把
//! 「引用还在、文件已无」的行清理掉。后者同时覆盖外部删除（手工删、
//! `deleteScreenshot`）留下的破图。

use std::collections::HashSet;

use mc_common::error::AppError;
use mc_common::time::Timestamp;

use crate::blob::{FileSystemBlobStore, RetentionPolicy};
use crate::db::Database;

/// 一次轮转的结果。数字要能解释「为什么磁盘变小了」。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RetentionOutcome {
    pub deleted_files: u64,
    pub freed_bytes: u64,
    pub kept_files: u64,
    /// 被清掉的悬空引用条数（含本次删除产生的与历史遗留的）
    pub cleared_references: u64,
}

/// 执行一次保留策略。
pub fn run_retention(
    db: &Database,
    blobs: &FileSystemBlobStore,
    policy: &RetentionPolicy,
    now: Timestamp,
) -> Result<RetentionOutcome, AppError> {
    let deleted = blobs.enforce_retention(now, policy)?;

    // 目录遍历一次拿到当前存在的文件全集；引用清理据此判断
    let existing: HashSet<String> = blobs.list_relative()?.into_iter().collect();
    let cleared_references = clear_dangling_references(db, &existing)?;

    Ok(RetentionOutcome {
        deleted_files: deleted.deleted,
        freed_bytes: deleted.freed_bytes,
        kept_files: deleted.kept,
        cleared_references,
    })
}

/// 清掉「引用指向的文件已不存在」的观测字段，返回清理条数。
///
/// 保留观测行本身：那是一条事实（当时确实截了图），只是图片不在了。
/// 清掉的是四个图片列，避免界面拿到一个必然 404 的路径。
pub fn clear_dangling_references(
    db: &Database,
    existing: &HashSet<String>,
) -> Result<u64, AppError> {
    let rows: Vec<(String, Option<String>, Option<String>)> = db.with_read(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, image_path, thumbnail_path FROM observations
              WHERE image_path IS NOT NULL OR thumbnail_path IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        rows.collect::<Result<Vec<_>, _>>()
    })?;

    // 先算出要清理什么，再一次性写库（避免在遍历里做写操作）
    let mut clear_all: Vec<String> = Vec::new();
    let mut clear_thumbnail: Vec<String> = Vec::new();
    for (id, image_path, thumbnail_path) in rows {
        let image_missing = image_path
            .as_ref()
            .is_some_and(|path| !path.is_empty() && !existing.contains(path));
        if image_missing {
            clear_all.push(id);
            continue;
        }
        let thumbnail_missing = thumbnail_path
            .as_ref()
            .is_some_and(|path| !path.is_empty() && !existing.contains(path));
        if thumbnail_missing {
            clear_thumbnail.push(id);
        }
    }

    if clear_all.is_empty() && clear_thumbnail.is_empty() {
        return Ok(0);
    }

    let cleared = clear_all.len() + clear_thumbnail.len();
    db.with_write(|conn| {
        let tx = conn.transaction()?;
        {
            let mut clear_image = tx.prepare(
                "UPDATE observations
                    SET image_path = NULL, image_blob_hash = NULL, thumbnail_path = NULL,
                        image_bytes = NULL, image_w = NULL, image_h = NULL
                  WHERE id = ?1",
            )?;
            for id in &clear_all {
                clear_image.execute([id])?;
            }

            let mut clear_thumb =
                tx.prepare("UPDATE observations SET thumbnail_path = NULL WHERE id = ?1")?;
            for id in &clear_thumbnail {
                clear_thumb.execute([id])?;
            }
        }
        tx.commit()?;
        Ok(())
    })?;

    Ok(cleared as u64)
}

/// 主动删除一张截图的执行结果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeleteOutcome {
    pub removed_files: u64,
    pub freed_bytes: u64,
    pub cleared_references: u64,
}

/// 删除一张截图（或它的缩略图）并同步清理数据库引用。
///
/// `relative` 必须是 blob 内的相对路径 —— 校验由 [`FileSystemBlobStore::resolve_relative`]
/// 完成（拒绝绝对路径与 `..`），安全属性只允许有一处实现。
///
/// **幂等**：文件已经不存在时返回成功且不清理多余的引用。
pub fn delete_screenshot(
    db: &Database,
    blobs: &FileSystemBlobStore,
    relative: &str,
) -> Result<DeleteOutcome, AppError> {
    // 先校验路径（非法路径在这里就失败，不会走到文件系统）
    let path = blobs.resolve_relative(relative)?;

    // 找出引用这一行的观测（原图与缩略图可能是不同行）
    let rows: Vec<(String, Option<String>, Option<String>)> = db.with_read(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, image_path, thumbnail_path FROM observations
              WHERE image_path = ?1 OR thumbnail_path = ?1",
        )?;
        let rows = stmt.query_map([relative], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>()
    })?;

    let mut outcome = DeleteOutcome::default();
    if path.is_file() {
        outcome.freed_bytes = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        match std::fs::remove_file(&path) {
            Ok(()) => outcome.removed_files = 1,
            // 竞态：校验与删除之间文件被清掉了，不算失败
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(AppError::new(
                    mc_common::error::ErrorCode::StorageUnavailable,
                    format!("删除截图失败（{}）：{error}", path.display()),
                ));
            }
        }
    }

    // 同步引用：删的是原图 → 清掉整组图片列；删的是缩略图 → 只清缩略图列
    db.with_write(|conn| {
        let tx = conn.transaction()?;
        {
            let mut clear_image = tx.prepare(
                "UPDATE observations
                    SET image_path = NULL, image_blob_hash = NULL, thumbnail_path = NULL,
                        image_bytes = NULL, image_w = NULL, image_h = NULL
                  WHERE id = ?1",
            )?;
            let mut clear_thumb =
                tx.prepare("UPDATE observations SET thumbnail_path = NULL WHERE id = ?1")?;
            for (id, image_path, thumbnail_path) in &rows {
                if image_path.as_deref() == Some(relative) {
                    clear_image.execute([id])?;
                } else if thumbnail_path.as_deref() == Some(relative) {
                    clear_thumb.execute([id])?;
                }
            }

            // 活动行里冗余存了一份自己的截图列表（`activity.resources`），
            // 只清观测的话那一行仍旧列出这个路径，界面上就是一张取不到的图。
            // 按路径剔除该元素；剔空后写 `[]`，不要留 NULL 让下游再判一次。
            tx.execute(
                "UPDATE activity
                    SET resources = COALESCE((
                        SELECT json_group_array(json(value))
                          FROM json_each(activity.resources)
                         WHERE json_extract(value, '$.path') IS NOT ?1
                    ), '[]')
                  WHERE json_valid(resources)
                    AND EXISTS (
                        SELECT 1 FROM json_each(activity.resources)
                         WHERE json_extract(value, '$.path') = ?1
                    )",
                [relative],
            )?;
        }
        tx.commit()?;
        Ok(())
    })?;
    outcome.cleared_references = rows.len() as u64;

    Ok(outcome)
}
