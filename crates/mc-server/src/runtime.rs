//! `runtime.json`：把端口与一次性 token 交给前端。
//!
//! 文件含 token，因此**必须以 0600 创建**（创建即设权限，而不是先写后 chmod
//! —— 后者在一个短暂窗口内是 0644）。

use std::path::{Path, PathBuf};

use mc_common::error::{AppError, ErrorCode};
use mc_common::time::Timestamp;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeInfo {
    pub port: u16,
    pub token: String,
    pub pid: u32,
    pub version: String,
    #[serde(serialize_with = "serialize_timestamp")]
    pub started_at: Timestamp,
}

fn serialize_timestamp<S: serde::Serializer>(
    value: &Timestamp,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&value.to_rfc3339())
}

pub fn write_runtime_file(dir: &Path, info: &RuntimeInfo) -> Result<PathBuf, AppError> {
    std::fs::create_dir_all(dir).map_err(|e| {
        AppError::new(
            ErrorCode::StorageUnavailable,
            format!("无法创建运行目录 {}: {e}", dir.display()),
        )
    })?;

    let path = dir.join("runtime.json");
    let text = serde_json::to_string_pretty(info).map_err(|e| {
        AppError::new(
            ErrorCode::StorageUnavailable,
            format!("runtime.json 序列化失败: {e}"),
        )
    })?;

    mc_common::fs::write_private_str(&path, &text).map_err(|e| {
        AppError::new(
            ErrorCode::StorageUnavailable,
            format!("无法写入 {}: {e}", path.display()),
        )
    })?;

    Ok(path)
}
