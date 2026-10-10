//! 本地文件夹导入与目录跟踪：把目录内支持的文件批量写入笔记树。
//!
//! 覆盖 README 中的「文件跟踪」、Obsidian 本地库与记忆库 markdown 目录导入。
//! 跟踪状态存在 settings：`context.tracked_folders` / `context.tracked_ingest_keys`。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use mc_common::error::{AppError, ErrorCode};
use mc_common::time::Timestamp;
use mc_storage::Database;
use serde_json::{json, Value};

use crate::file_ingest::{self, FileIngestResult};

const TRACKED_FOLDERS_KEY: &str = "context.tracked_folders";
const TRACKED_KEYS_KEY: &str = "context.tracked_ingest_keys";
const MAX_FILES_PER_SYNC: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderImportResult {
    pub path: String,
    pub imported: Vec<FileIngestResult>,
    pub skipped: usize,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedFolder {
    pub path: String,
}

/// 一次性导入目录中支持的文件（Obsidian / Memory Bank / 任意本地文档树）。
pub fn import_folder(
    db: &Database,
    data_dir: &Path,
    folder: &Path,
    parent_id: Option<i64>,
    recursive: bool,
    at: Timestamp,
) -> Result<FolderImportResult, AppError> {
    let folder = normalize_dir(folder)?;
    let files = list_candidate_files(&folder, recursive)?;
    let mut imported = Vec::new();
    let mut skipped = 0usize;
    let mut errors = Vec::new();

    for path in files.into_iter().take(MAX_FILES_PER_SYNC) {
        let name = match path.file_name().and_then(|v| v.to_str()) {
            Some(name) => name.to_string(),
            None => {
                skipped += 1;
                continue;
            }
        };
        if file_ingest::classify_file(&name).is_err() {
            skipped += 1;
            continue;
        }
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                errors.push(format!("{}: {error}", path.display()));
                continue;
            }
        };
        match file_ingest::ingest_file(db, data_dir, &name, &bytes, parent_id, at) {
            Ok(result) => imported.push(result),
            Err(error) => errors.push(format!("{name}: {}", error.detail())),
        }
    }

    if imported.is_empty() && errors.is_empty() {
        return Err(invalid("目录中没有可导入的支持文件"));
    }

    Ok(FolderImportResult {
        path: folder.to_string_lossy().into_owned(),
        imported,
        skipped,
        errors,
    })
}

/// 登记跟踪目录（幂等）。
pub fn track_folder(
    db: &Database,
    folder: &Path,
    at: Timestamp,
) -> Result<TrackedFolder, AppError> {
    let folder = normalize_dir(folder)?;
    let path = folder.to_string_lossy().into_owned();
    let mut folders = load_tracked(db)?;
    if !folders.iter().any(|item| item.path == path) {
        folders.push(TrackedFolder { path: path.clone() });
        save_tracked(db, &folders, at)?;
    }
    Ok(TrackedFolder { path })
}

pub fn untrack_folder(db: &Database, folder: &Path, at: Timestamp) -> Result<(), AppError> {
    let path = normalize_dir(folder)?.to_string_lossy().into_owned();
    let folders: Vec<TrackedFolder> = load_tracked(db)?
        .into_iter()
        .filter(|item| item.path != path)
        .collect();
    save_tracked(db, &folders, at)?;
    Ok(())
}

pub fn list_tracked(db: &Database) -> Result<Vec<TrackedFolder>, AppError> {
    load_tracked(db)
}

/// 扫描已跟踪目录，只导入尚未见过（路径+mtime+size）的支持文件。
pub fn sync_tracked(
    db: &Database,
    data_dir: &Path,
    only_path: Option<&Path>,
    parent_id: Option<i64>,
    at: Timestamp,
) -> Result<FolderImportResult, AppError> {
    let tracked = load_tracked(db)?;
    let targets: Vec<PathBuf> = match only_path {
        Some(path) => {
            let normalized = normalize_dir(path)?;
            let as_str = normalized.to_string_lossy().into_owned();
            if !tracked.iter().any(|item| item.path == as_str) {
                return Err(invalid("该目录尚未登记跟踪，请先 track"));
            }
            vec![normalized]
        }
        None => {
            if tracked.is_empty() {
                return Err(invalid("尚未登记任何跟踪目录"));
            }
            tracked
                .iter()
                .map(|item| PathBuf::from(&item.path))
                .collect()
        }
    };

    let mut seen = load_ingest_keys(db)?;
    let mut imported = Vec::new();
    let mut skipped = 0usize;
    let mut errors = Vec::new();
    let mut root_display = String::new();

    for folder in targets {
        if root_display.is_empty() {
            root_display = folder.to_string_lossy().into_owned();
        }
        let files = match list_candidate_files(&folder, true) {
            Ok(files) => files,
            Err(error) => {
                errors.push(format!("{}: {}", folder.display(), error.detail()));
                continue;
            }
        };
        for path in files {
            if imported.len() >= MAX_FILES_PER_SYNC {
                skipped += 1;
                continue;
            }
            let name = match path.file_name().and_then(|v| v.to_str()) {
                Some(name) => name.to_string(),
                None => {
                    skipped += 1;
                    continue;
                }
            };
            if file_ingest::classify_file(&name).is_err() {
                skipped += 1;
                continue;
            }
            let meta = match std::fs::metadata(&path) {
                Ok(meta) => meta,
                Err(error) => {
                    errors.push(format!("{}: {error}", path.display()));
                    continue;
                }
            };
            let key = format!(
                "{}|{}|{}",
                path.to_string_lossy(),
                meta.len(),
                meta.modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
            );
            if seen.contains(&key) {
                skipped += 1;
                continue;
            }
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    errors.push(format!("{}: {error}", path.display()));
                    continue;
                }
            };
            match file_ingest::ingest_file(db, data_dir, &name, &bytes, parent_id, at) {
                Ok(result) => {
                    seen.insert(key);
                    imported.push(result);
                }
                Err(error) => errors.push(format!("{name}: {}", error.detail())),
            }
        }
    }

    save_ingest_keys(db, &seen, at)?;

    if imported.is_empty() && errors.is_empty() {
        return Ok(FolderImportResult {
            path: root_display,
            imported,
            skipped,
            errors,
        });
    }

    Ok(FolderImportResult {
        path: root_display,
        imported,
        skipped,
        errors,
    })
}

fn normalize_dir(folder: &Path) -> Result<PathBuf, AppError> {
    let folder = if folder.as_os_str().is_empty() {
        return Err(invalid("目录路径不能为空"));
    } else {
        folder
    };
    let canonical = folder.canonicalize().map_err(|error| {
        AppError::new(
            ErrorCode::DomainInvalidRange,
            format!("无法解析目录 {}: {error}", folder.display()),
        )
    })?;
    if !canonical.is_dir() {
        return Err(invalid(&format!("{} 不是目录", canonical.display())));
    }
    Ok(canonical)
}

fn list_candidate_files(folder: &Path, recursive: bool) -> Result<Vec<PathBuf>, AppError> {
    let mut out = Vec::new();
    walk(folder, recursive, &mut out, 0)?;
    out.sort();
    Ok(out)
}

fn walk(dir: &Path, recursive: bool, out: &mut Vec<PathBuf>, depth: usize) -> Result<(), AppError> {
    if depth > 8 {
        return Ok(());
    }
    let entries = std::fs::read_dir(dir).map_err(|error| {
        AppError::new(
            ErrorCode::StorageUnavailable,
            format!("无法读取目录 {}: {error}", dir.display()),
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            AppError::new(
                ErrorCode::StorageUnavailable,
                format!("读取目录项失败：{error}"),
            )
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            AppError::new(
                ErrorCode::StorageUnavailable,
                format!("无法识别文件类型：{error}"),
            )
        })?;
        if file_type.is_dir() {
            let name = path.file_name().and_then(|v| v.to_str()).unwrap_or("");
            if name.starts_with('.') || name == "node_modules" || name == ".git" {
                continue;
            }
            if recursive {
                walk(&path, true, out, depth + 1)?;
            }
        } else if file_type.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

fn load_tracked(db: &Database) -> Result<Vec<TrackedFolder>, AppError> {
    let value = db.get_setting(TRACKED_FOLDERS_KEY)?.unwrap_or(json!([]));
    let mut out = Vec::new();
    if let Some(arr) = value.as_array() {
        for item in arr {
            if let Some(path) = item.get("path").and_then(|v| v.as_str()) {
                if !path.is_empty() {
                    out.push(TrackedFolder {
                        path: path.to_string(),
                    });
                }
            } else if let Some(path) = item.as_str() {
                if !path.is_empty() {
                    out.push(TrackedFolder {
                        path: path.to_string(),
                    });
                }
            }
        }
    }
    Ok(out)
}

fn save_tracked(db: &Database, folders: &[TrackedFolder], at: Timestamp) -> Result<(), AppError> {
    let value: Value = folders
        .iter()
        .map(|item| json!({ "path": item.path }))
        .collect::<Vec<_>>()
        .into();
    db.set_setting(TRACKED_FOLDERS_KEY, &value, at)
}

fn load_ingest_keys(db: &Database) -> Result<BTreeSet<String>, AppError> {
    let value = db.get_setting(TRACKED_KEYS_KEY)?.unwrap_or(json!([]));
    let mut out = BTreeSet::new();
    if let Some(arr) = value.as_array() {
        for item in arr {
            if let Some(key) = item.as_str() {
                out.insert(key.to_string());
            }
        }
    } else if let Some(obj) = value.as_object() {
        for key in obj.keys() {
            out.insert(key.clone());
        }
    }
    Ok(out)
}

fn save_ingest_keys(db: &Database, keys: &BTreeSet<String>, at: Timestamp) -> Result<(), AppError> {
    // 限制体积：只保留最近登记的键（按字典序截断尾部亦可；这里保全量到 5000）。
    let trimmed: Vec<String> = keys.iter().rev().take(5000).cloned().collect();
    let mut ordered: BTreeMap<String, bool> = BTreeMap::new();
    for key in trimmed {
        ordered.insert(key, true);
    }
    let value: Value = ordered.keys().cloned().collect::<Vec<_>>().into();
    db.set_setting(TRACKED_KEYS_KEY, &value, at)
}

fn invalid(reason: &str) -> AppError {
    AppError::new(ErrorCode::DomainInvalidRange, reason.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;
    use std::sync::Arc;

    #[test]
    fn import_folder_ingests_markdown_and_skips_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("obsidian");
        std::fs::create_dir_all(&vault).unwrap();
        std::fs::write(
            vault.join("note.md"),
            b"# Hello Obsidian\n\nLocal vault note.\n",
        )
        .unwrap();
        std::fs::write(vault.join("binary.bin"), b"\0\0\0").unwrap();

        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let db = Database::open(data.join("minecontext.db")).unwrap();
        let at = Timestamp::from_millis(T0);
        let result = import_folder(&db, &data, &vault, None, false, at).unwrap();
        assert_eq!(result.imported.len(), 1);
        assert!(result.skipped >= 1);
        assert!(result.imported[0].title.contains("note") || result.imported[0].name == "note.md");
    }

    #[test]
    fn track_and_sync_is_idempotent_for_same_file() {
        let dir = tempfile::tempdir().unwrap();
        let watched = dir.path().join("watched");
        std::fs::create_dir_all(&watched).unwrap();
        std::fs::write(watched.join("a.md"), b"tracked alpha content unique\n").unwrap();

        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let db = Arc::new(Database::open(data.join("minecontext.db")).unwrap());
        let at = Timestamp::from_millis(T0);

        track_folder(&db, &watched, at).unwrap();
        let first = sync_tracked(&db, &data, None, None, at).unwrap();
        assert_eq!(first.imported.len(), 1);
        let second = sync_tracked(&db, &data, None, None, at).unwrap();
        assert!(second.imported.is_empty(), "第二次不得重复导入：{second:?}");
        assert!(second.skipped >= 1);
    }
}
