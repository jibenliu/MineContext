//! 配置写回：UI 改设置 → 用户配置层 → 热重载。
//!
//! 三条硬约束，每条都对应一类真实事故：**先校验后落盘**（写坏配置会直接让
//! daemon 起不来，校验跑的是与启动时同一套分层加载）；**合并不是覆盖**
//! （用户文件里可能有本次不打算动的层，整体覆盖会把它们清掉）；
//! **原子落盘**（先写同目录临时文件再 `rename`，中途失败留下的是完整旧文件）。
//!
//! 校验失败时**不动磁盘**：这是「保存设置」这类操作的底线。

use std::path::{Path, PathBuf};

use mc_common::error::{AppError, ErrorCode};

use crate::load::{merge_value, ConfigHandle, LayerSource, LoadRequest, ReloadOutcome};

/// 把一段补丁合并进用户配置文件，校验通过后原子落盘并热重载。
///
/// `patch` 是 TOML 表（路由层把 JSON 请求体转成它）。
/// 返回重载后的配置，调用方可以直接用它响应本次请求。
pub fn apply_patch(
    request: &LoadRequest,
    user_path: &Path,
    handle: &ConfigHandle,
    patch: &toml::Value,
) -> Result<std::sync::Arc<crate::load::LoadedConfig>, AppError> {
    let mut merged = read_user_config(user_path)?;
    merge_value(&mut merged, patch.clone());

    let text = toml::to_string(&merged).map_err(|error| {
        AppError::new(ErrorCode::ConfigInvalid, format!("配置无法序列化：{error}"))
    })?;

    // 校验：把用户层替换成「合并后的内容」，其余层保持不变，
    // 因此判定结果与下一次启动完全一致。
    let probe = replace_user_layer(request, user_path, &text);
    crate::load::load(&probe)?;

    write_atomic(user_path, &text)?;

    match handle.reload(request) {
        ReloadOutcome::Applied(loaded) => Ok(loaded),
        // 校验已经通过，这里失败说明磁盘上的东西与刚写的不一致
        ReloadOutcome::Rejected(error) => Err(error),
    }
}

/// 读取用户配置；文件不存在时给出空表（而不是报错）。
///
/// 文件存在但解析不了时**报错**：宁可不写，也不能把用户手写的配置覆盖掉。
pub fn read_user_config(user_path: &Path) -> Result<toml::Value, AppError> {
    if !user_path.exists() {
        return Ok(toml::Value::Table(toml::map::Map::new()));
    }

    let text = std::fs::read_to_string(user_path).map_err(|error| {
        AppError::new(
            ErrorCode::ConfigUnreadable,
            format!("无法读取用户配置 {}: {error}", user_path.display()),
        )
    })?;

    if text.trim().is_empty() {
        return Ok(toml::Value::Table(toml::map::Map::new()));
    }

    toml::from_str(&text).map_err(|error| {
        AppError::new(
            ErrorCode::ConfigInvalid,
            format!(
                "用户配置 {} 无法解析，已拒绝覆盖：{error}",
                user_path.display()
            ),
        )
    })
}

/// 把请求里的用户层换成给定内容，其余层原样保留。
fn replace_user_layer(request: &LoadRequest, user_path: &Path, text: &str) -> LoadRequest {
    let mut layers: Vec<LayerSource> = Vec::with_capacity(request.layers.len() + 1);
    let mut replaced = false;

    for layer in &request.layers {
        match layer {
            LayerSource::File(path) if path == user_path => {
                layers.push(LayerSource::Inline {
                    name: user_path.display().to_string(),
                    toml: text.to_string(),
                });
                replaced = true;
            }
            other => layers.push(other.clone()),
        }
    }

    if !replaced {
        // 用户层还没在请求里（首次写入）→ 追加到最高优先级处
        layers.push(LayerSource::Inline {
            name: user_path.display().to_string(),
            toml: text.to_string(),
        });
    }

    LoadRequest {
        layers,
        env: request.env.clone(),
        read_process_env: request.read_process_env,
    }
}

/// 原子写：同目录临时文件 + `rename`。
///
/// 临时文件必须与目标同目录，否则 `rename` 可能跨设备失败。
pub fn write_atomic(path: &Path, text: &str) -> Result<(), AppError> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|error| {
            AppError::new(
                ErrorCode::ConfigUnreadable,
                format!("无法创建配置目录 {}: {error}", parent.display()),
            )
        })?;
    }

    let temp = temp_path(path);
    std::fs::write(&temp, text).map_err(|error| {
        AppError::new(
            ErrorCode::ConfigUnreadable,
            format!("无法写入临时配置 {}: {error}", temp.display()),
        )
    })?;

    std::fs::rename(&temp, path).map_err(|error| {
        // 失败时清理临时文件，避免留下半截内容误导下一次排查
        let _ = std::fs::remove_file(&temp);
        AppError::new(
            ErrorCode::ConfigUnreadable,
            format!("无法替换配置文件 {}: {error}", path.display()),
        )
    })
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}
