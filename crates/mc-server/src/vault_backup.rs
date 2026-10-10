//! 笔记树 + `uploads/` 备份包。
//!
//! 与诊断包不同：这里**刻意包含用户内容**，供重装 / 换机恢复；
//! 形状收敛在既有 `VaultRow` 与 uploads 目录，不另开索引管线。

use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use base64::Engine;
use mc_common::error::{AppError, ErrorCode};
use mc_common::time::Timestamp;
use mc_storage::vaults::{VaultQuery, VaultRow, VaultUpsert};
use mc_storage::Database;
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::routes::files::UPLOADS_DIR;

pub const FORMAT: &str = "minecontext-vault-backup";
pub const SCHEMA_VERSION: u32 = 1;
/// 导入 zip 的字节上限（含 base64 解码后）。
pub const MAX_ARCHIVE_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub format: String,
    pub schema_version: u32,
    pub exported_at_ms: i64,
    pub vault_count: usize,
    pub file_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportSummary {
    pub vault_count: usize,
    pub file_count: usize,
}

/// 打包当前 vault 行（非软删）与 uploads 文件为 zip 字节。
pub fn build_export_zip(
    db: &Database,
    data_dir: &Path,
    at: Timestamp,
) -> Result<(Vec<u8>, Manifest), AppError> {
    let vaults = db.query_vault_rows(&VaultQuery::default())?;
    let uploads = list_upload_files(data_dir)?;

    let manifest = Manifest {
        format: FORMAT.to_string(),
        schema_version: SCHEMA_VERSION,
        exported_at_ms: at.as_millis(),
        vault_count: vaults.len(),
        file_count: uploads.len(),
    };

    let mut cursor = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut cursor);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        zip.start_file("manifest.json", options)
            .map_err(zip_error)?;
        zip.write_all(
            serde_json::to_vec_pretty(&manifest)
                .map_err(json_error)?
                .as_slice(),
        )
        .map_err(io_error)?;

        zip.start_file("vaults.json", options).map_err(zip_error)?;
        zip.write_all(
            serde_json::to_vec_pretty(&vaults)
                .map_err(json_error)?
                .as_slice(),
        )
        .map_err(io_error)?;

        for (name, bytes) in &uploads {
            let entry = format!("{UPLOADS_DIR}/{name}");
            zip.start_file(&entry, options).map_err(zip_error)?;
            zip.write_all(bytes).map_err(io_error)?;
        }

        zip.finish().map_err(zip_error)?;
    }

    Ok((cursor.into_inner(), manifest))
}

/// 从 zip 字节恢复 vault 行与 uploads；`file://…/uploads/<name>` 改写到本机 data_dir。
pub fn import_export_zip(
    db: &Database,
    data_dir: &Path,
    zip_bytes: &[u8],
    at: Timestamp,
) -> Result<ImportSummary, AppError> {
    if zip_bytes.len() as u64 > MAX_ARCHIVE_BYTES {
        return Err(AppError::new(
            ErrorCode::DomainInvalidRange,
            format!("备份包超过上限（{} 字节）", MAX_ARCHIVE_BYTES),
        ));
    }

    let mut archive = ZipArchive::new(Cursor::new(zip_bytes)).map_err(|error| {
        AppError::new(
            ErrorCode::DomainInvalidRange,
            format!("无法打开备份 zip：{error}"),
        )
    })?;

    let manifest: Manifest = {
        let bytes = read_required_entry(&mut archive, "manifest.json")?;
        serde_json::from_slice(&bytes).map_err(|error| {
            AppError::new(
                ErrorCode::DomainInvalidRange,
                format!("manifest.json 无法解析：{error}"),
            )
        })?
    };
    if manifest.format != FORMAT || manifest.schema_version != SCHEMA_VERSION {
        return Err(AppError::new(
            ErrorCode::DomainInvalidRange,
            format!(
                "不支持的备份格式（format={}, schema_version={}）",
                manifest.format, manifest.schema_version
            ),
        ));
    }

    let vaults: Vec<VaultRow> = {
        let bytes = read_required_entry(&mut archive, "vaults.json")?;
        serde_json::from_slice(&bytes).map_err(|error| {
            AppError::new(
                ErrorCode::DomainInvalidRange,
                format!("vaults.json 无法解析：{error}"),
            )
        })?
    };

    let upload_names = restore_uploads(data_dir, &mut archive)?;
    let name_set: HashSet<String> = upload_names.iter().cloned().collect();
    let vault_count = restore_vaults(db, data_dir, &vaults, &name_set, at)?;

    Ok(ImportSummary {
        vault_count,
        file_count: upload_names.len(),
    })
}

pub fn decode_archive_base64(data: &str) -> Result<Vec<u8>, AppError> {
    base64::engine::general_purpose::STANDARD
        .decode(data.trim())
        .map_err(|error| {
            AppError::new(
                ErrorCode::DomainInvalidRange,
                format!("备份包 base64 无法解码：{error}"),
            )
        })
}

fn list_upload_files(data_dir: &Path) -> Result<Vec<(String, Vec<u8>)>, AppError> {
    let dir = data_dir.join(UPLOADS_DIR);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(&dir).map_err(|error| {
        AppError::new(
            ErrorCode::StorageUnavailable,
            format!("无法读取上传目录 {}: {error}", dir.display()),
        )
    })?;

    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        // 拒绝路径分隔符，避免 zip slip 形状的文件名
        if name.contains('/') || name.contains('\\') || name == ".." || name == "." {
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|error| {
            AppError::new(
                ErrorCode::StorageUnavailable,
                format!("无法读取上传文件 {}: {error}", path.display()),
            )
        })?;
        files.push((name, bytes));
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(files)
}

fn restore_uploads(
    data_dir: &Path,
    archive: &mut ZipArchive<Cursor<&[u8]>>,
) -> Result<Vec<String>, AppError> {
    let uploads_dir = data_dir.join(UPLOADS_DIR);
    std::fs::create_dir_all(&uploads_dir).map_err(|error| {
        AppError::new(
            ErrorCode::StorageUnavailable,
            format!("无法创建上传目录 {}: {error}", uploads_dir.display()),
        )
    })?;

    let mut names = Vec::new();
    let prefix = format!("{UPLOADS_DIR}/");
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).map_err(zip_error)?;
        let name = file.name().to_string();
        if file.is_dir() || !name.starts_with(&prefix) {
            continue;
        }
        let file_name = name[prefix.len()..].to_string();
        if file_name.is_empty()
            || file_name.contains('/')
            || file_name.contains('\\')
            || file_name == ".."
            || file_name == "."
        {
            return Err(AppError::new(
                ErrorCode::DomainInvalidRange,
                format!("备份包含非法上传文件名：{name}"),
            ));
        }
        let dest = mc_common::fs::resolve_within(&uploads_dir, &file_name)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(io_error)?;
        if bytes.len() as u64 > crate::routes::files::MAX_UPLOAD_BYTES {
            return Err(AppError::new(
                ErrorCode::DomainInvalidRange,
                format!("上传文件 {file_name} 超过单文件上限"),
            ));
        }
        std::fs::write(&dest, &bytes).map_err(|error| {
            AppError::new(
                ErrorCode::StorageUnavailable,
                format!("无法写入 {}: {error}", dest.display()),
            )
        })?;
        names.push(file_name);
    }
    names.sort();
    Ok(names)
}

fn restore_vaults(
    db: &Database,
    data_dir: &Path,
    vaults: &[VaultRow],
    upload_names: &HashSet<String>,
    at: Timestamp,
) -> Result<usize, AppError> {
    let mut pending: Vec<&VaultRow> = vaults.iter().collect();
    let mut id_map: HashMap<i64, i64> = HashMap::new();
    let mut inserted = 0usize;

    while !pending.is_empty() {
        let before = pending.len();
        let mut next = Vec::new();
        for row in pending {
            let parent_ready = match row.parent_id {
                None => true,
                Some(parent) => id_map.contains_key(&parent),
            };
            if !parent_ready {
                next.push(row);
                continue;
            }
            let parent_id = row.parent_id.and_then(|old| id_map.get(&old).copied());
            let content = rewrite_upload_file_urls(&row.content, data_dir, upload_names);
            let new_id = db.insert_vault_row(
                &VaultUpsert {
                    title: row.title.clone(),
                    summary: row.summary.clone(),
                    content,
                    tags: split_tags(&row.tags),
                    parent_id,
                    is_folder: row.is_folder == 1,
                    document_type: row.document_type.clone(),
                    sort_order: row.sort_order,
                },
                at,
            )?;
            id_map.insert(row.id, new_id);
            inserted += 1;
        }
        if next.len() == before {
            return Err(AppError::new(
                ErrorCode::DomainInvariantViolated,
                "备份包中的笔记树存在无法解析的 parent_id 环或悬空父节点",
            ));
        }
        pending = next;
    }

    Ok(inserted)
}

/// 把正文里指向 uploads 的 `file://` 改到本机 data_dir（按 basename 匹配）。
fn rewrite_upload_file_urls(
    content: &str,
    data_dir: &Path,
    upload_names: &HashSet<String>,
) -> String {
    let mut out = String::with_capacity(content.len());
    let mut rest = content;
    while let Some(start) = rest.find("file://") {
        out.push_str(&rest[..start]);
        let after_scheme = &rest[start + "file://".len()..];
        let end = after_scheme
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | '"' | '\'' | ']' | '>'))
            .unwrap_or(after_scheme.len());
        let path_part = &after_scheme[..end];
        if let Some(name) = Path::new(path_part).file_name().and_then(|n| n.to_str()) {
            if upload_names.contains(name) && path_part.contains(&format!("/{UPLOADS_DIR}/")) {
                out.push_str("file://");
                out.push_str(
                    &PathBuf::from(data_dir)
                        .join(UPLOADS_DIR)
                        .join(name)
                        .to_string_lossy(),
                );
            } else {
                out.push_str("file://");
                out.push_str(path_part);
            }
        } else {
            out.push_str("file://");
            out.push_str(path_part);
        }
        rest = &after_scheme[end..];
    }
    out.push_str(rest);
    out
}

fn split_tags(tags: &str) -> Vec<String> {
    tags.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

fn read_required_entry(
    archive: &mut ZipArchive<Cursor<&[u8]>>,
    name: &str,
) -> Result<Vec<u8>, AppError> {
    let mut file = archive
        .by_name(name)
        .map_err(|_| AppError::new(ErrorCode::DomainInvalidRange, format!("备份包缺少 {name}")))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(io_error)?;
    Ok(bytes)
}

fn zip_error(error: zip::result::ZipError) -> AppError {
    AppError::new(
        ErrorCode::StorageUnavailable,
        format!("zip 操作失败：{error}"),
    )
}

fn io_error(error: std::io::Error) -> AppError {
    AppError::new(
        ErrorCode::StorageUnavailable,
        format!("读写备份包失败：{error}"),
    )
}

fn json_error(error: serde_json::Error) -> AppError {
    AppError::new(
        ErrorCode::StorageUnavailable,
        format!("序列化备份包失败：{error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrite_replaces_upload_file_urls_by_basename() {
        let names = HashSet::from(["note-image-1.png".to_string()]);
        let data_dir = Path::new("/new/data");
        let content = "![x](file:///old/Machine/uploads/note-image-1.png) keep";
        let rewritten = rewrite_upload_file_urls(content, data_dir, &names);
        assert_eq!(
            rewritten,
            "![x](file:///new/data/uploads/note-image-1.png) keep"
        );
    }
}
