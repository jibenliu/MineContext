//! `/api/files*` —— 上传文件的列表 / 保存 / 读取 / 复制。
//!
//! 目录是 `<data_dir>/uploads`。
//!
//! **安全约束**（不改返回形状）：文件名必须落在上传目录内 —— 直接拼接文件名
//! 允许 `../../id_rsa`，而 token 只证明「是本应用」，不证明参数可信；
//! `copy` 的来源可以是任意绝对路径（那正是它的用途），但必须是常规文件且有大小上限。

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use mc_common::error::{AppError, ErrorCode};
use mc_common::time::{Clock, SystemClock};
use serde::Deserialize;
use serde_json::json;

use crate::envelope;
use crate::state::ServerState;

/// 上传目录名。
pub const UPLOADS_DIR: &str = "uploads";

/// 单个上传文件的大小上限。
pub const MAX_UPLOAD_BYTES: u64 = 64 * 1024 * 1024;

pub fn router() -> Router<Arc<ServerState>> {
    Router::new()
        .route("/api/files", get(list).post(save))
        .route("/api/files/copy", post(copy))
        .route("/api/files/{name}/data", get(read))
}

fn uploads_dir(state: &ServerState) -> Result<std::path::PathBuf, AppError> {
    let dir = state.data_dir.join(UPLOADS_DIR);
    if !dir.exists() {
        std::fs::create_dir_all(&dir).map_err(|error| {
            AppError::new(
                ErrorCode::StorageUnavailable,
                format!("无法创建上传目录 {}: {error}", dir.display()),
            )
        })?;
    }
    Ok(dir)
}

/// `source` 是给人看的一行：`<扩展名大写> · <MB>MB`。
fn describe_source(name: &str, size: u64) -> String {
    let extension = std::path::Path::new(name)
        .extension()
        .map(|value| value.to_string_lossy().to_ascii_uppercase())
        .unwrap_or_default();
    let megabytes = size as f64 / 1024.0 / 1024.0;
    format!("{extension} · {megabytes:.2}MB")
}

async fn list(State(state): State<Arc<ServerState>>) -> Response {
    let dir = match uploads_dir(&state) {
        Ok(dir) => dir,
        Err(error) => return envelope::compat_failure(&error),
    };

    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) => {
            return envelope::ok(json!({
                "success": false,
                "error": format!("无法读取上传目录：{error}"),
            }));
        }
    };

    let mut files: Vec<serde_json::Value> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        // 只列常规文件，目录与设备节点不进列表
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        files.push(json!({
            "name": name,
            "source": describe_source(&name, metadata.len()),
            "filePath": path.to_string_lossy(),
            "status": "Uploaded",
        }));
    }

    // 稳定顺序：目录遍历顺序是文件系统给的，两次请求可能不同
    files.sort_by(|left, right| {
        left["name"]
            .as_str()
            .unwrap_or_default()
            .cmp(right["name"].as_str().unwrap_or_default())
    });

    envelope::ok(json!({ "success": true, "files": files }))
}

#[derive(Debug, Deserialize)]
struct SaveBody {
    name: String,
    #[serde(default)]
    data: Option<serde_json::Value>,
}

/// 把 `data` 解析成字节。
///
/// 渲染层传的是 `Uint8Array`，`JSON.stringify` 之后是
/// `{"0":104,"1":105}`（对象，不是数组），因此三种形状都要支持。
fn decode_payload(value: Option<&serde_json::Value>) -> Result<Vec<u8>, AppError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };

    match value {
        serde_json::Value::Null => Ok(Vec::new()),
        serde_json::Value::Array(items) => items
            .iter()
            .map(|item| {
                item.as_u64()
                    .filter(|byte| *byte <= 255)
                    .map(|byte| byte as u8)
                    .ok_or_else(|| invalid("data 数组里必须是 0..=255 的整数"))
            })
            .collect(),
        serde_json::Value::Object(map) => {
            let mut bytes: Vec<(u64, u8)> = Vec::with_capacity(map.len());
            for (key, item) in map {
                let index: u64 = key.parse().map_err(|_| {
                    invalid("data 对象的键必须是数组下标（Uint8Array 序列化后的形状）")
                })?;
                let byte = item
                    .as_u64()
                    .filter(|byte| *byte <= 255)
                    .ok_or_else(|| invalid("data 对象的值必须是 0..=255 的整数"))?;
                bytes.push((index, byte as u8));
            }
            bytes.sort_by_key(|(index, _)| *index);
            Ok(bytes.into_iter().map(|(_, byte)| byte).collect())
        }
        serde_json::Value::String(text) => decode_base64(text),
        _ => Err(invalid(
            "data 必须是字节数组、Uint8Array 形状的对象或 base64 字符串",
        )),
    }
}

fn decode_base64(text: &str) -> Result<Vec<u8>, AppError> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .map_err(|error| invalid(&format!("base64 解码失败：{error}")))
}

fn invalid(reason: &str) -> AppError {
    AppError::new(ErrorCode::DomainInvalidRange, reason.to_string())
}

async fn save(State(state): State<Arc<ServerState>>, Json(body): Json<SaveBody>) -> Response {
    let dir = match uploads_dir(&state) {
        Ok(dir) => dir,
        Err(error) => return envelope::compat_failure(&error),
    };

    // 非法文件名是**调用方错误**（也可能是攻击尝试）：走失败信封，失败即关闭
    let path = match mc_common::fs::resolve_within(&dir, &body.name) {
        Ok(path) => path,
        Err(error) => return envelope::compat_failure(&error),
    };

    let bytes = match decode_payload(body.data.as_ref()) {
        Ok(bytes) => bytes,
        Err(error) => return envelope::compat_failure(&error),
    };

    if bytes.len() as u64 > MAX_UPLOAD_BYTES {
        return envelope::compat_failure(&AppError::new(
            ErrorCode::DomainInvalidRange,
            format!("文件过大（{} 字节，上限 {MAX_UPLOAD_BYTES}）", bytes.len()),
        ));
    }

    // IO 失败返回 `{success:false, error}`，不抛异常
    match std::fs::write(&path, &bytes) {
        Ok(()) => envelope::ok(json!({
            "success": true,
            "filePath": path.to_string_lossy(),
        })),
        Err(error) => envelope::ok(json!({
            "success": false,
            "error": error.to_string(),
        })),
    }
}

#[derive(Debug, Deserialize)]
struct ReadQuery {
    encoding: Option<String>,
}

async fn read(
    State(state): State<Arc<ServerState>>,
    Path(name): Path<String>,
    Query(query): Query<ReadQuery>,
) -> Response {
    let dir = match uploads_dir(&state) {
        Ok(dir) => dir,
        Err(error) => return envelope::compat_failure(&error),
    };
    let path = match mc_common::fs::resolve_within(&dir, &name) {
        Ok(path) => path,
        Err(error) => return envelope::compat_failure(&error),
    };

    if let Some(encoding) = query.encoding {
        if encoding != "base64" {
            return envelope::compat_failure(&invalid("encoding 只支持 base64"));
        }
        use base64::Engine;
        return match std::fs::read(&path) {
            Ok(bytes) if bytes.len() as u64 <= MAX_UPLOAD_BYTES => envelope::ok(json!({
                "success": true,
                "data": base64::engine::general_purpose::STANDARD.encode(bytes),
            })),
            Ok(_) => envelope::compat_failure(&invalid("文件超过读取上限")),
            Err(error) => envelope::ok(json!({ "success": false, "error": error.to_string() })),
        };
    }

    match std::fs::read_to_string(&path) {
        Ok(data) => envelope::ok(json!({ "success": true, "data": data })),
        Err(error) => envelope::ok(json!({
            "success": false,
            "error": error.to_string(),
        })),
    }
}

#[derive(Debug, Deserialize)]
struct CopyBody {
    path: String,
}

async fn copy(State(state): State<Arc<ServerState>>, Json(body): Json<CopyBody>) -> Response {
    let dir = match uploads_dir(&state) {
        Ok(dir) => dir,
        Err(error) => return envelope::compat_failure(&error),
    };

    let source = std::path::Path::new(&body.path);
    // 来源必须是绝对路径：相对路径的含义取决于进程 CWD，
    // 而「复制任意相对路径」正是误用与穿越的来源。
    if !source.is_absolute() {
        return envelope::compat_failure(&AppError::new(
            ErrorCode::DomainInvalidRange,
            format!("复制来源必须是绝对路径：{:?}", body.path),
        ));
    }

    let Some(file_name) = source
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
    else {
        return envelope::ok(json!({ "success": false, "error": "来源路径没有文件名" }));
    };

    let metadata = match std::fs::metadata(source) {
        Ok(metadata) => metadata,
        Err(error) => {
            return envelope::ok(json!({ "success": false, "error": error.to_string() }));
        }
    };
    if !metadata.is_file() {
        return envelope::ok(json!({
            "success": false,
            "error": "来源不是常规文件",
        }));
    }
    if metadata.len() > MAX_UPLOAD_BYTES {
        return envelope::compat_failure(&AppError::new(
            ErrorCode::DomainInvalidRange,
            format!("来源文件过大（{} 字节）", metadata.len()),
        ));
    }

    // 目标只用 basename，因此不可能逃出上传目录
    let destination = match mc_common::fs::resolve_within(&dir, &file_name) {
        Ok(path) => path,
        Err(error) => return envelope::compat_failure(&error),
    };

    let _ = Clock::now(&SystemClock);
    match std::fs::copy(source, &destination) {
        Ok(_) => envelope::ok(json!({ "success": true })),
        Err(error) => envelope::ok(json!({
            "success": false,
            "error": error.to_string(),
        })),
    }
}
