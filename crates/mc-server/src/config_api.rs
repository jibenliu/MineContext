//! 采集设置的写回：把 UI 的改动落到用户配置层并热重载。
//!
//! 这里只做两件事：把请求体翻译成配置补丁、调用 `mc_config::write::apply_patch`；
//! 校验与原子落盘都在配置层。
//!
//! 两条语义：**选择是替换**（累积会让取消勾选永不生效）、
//! **没给的字段不动**（补丁只含请求里出现的键，PATCH 时段不会顺手重置间隔）。

use std::sync::Arc;

use mc_common::error::{AppError, ErrorCode};
use mc_config::load::LoadedConfig;
use serde_json::Value;

use crate::state::ServerState;

/// 把补丁写进用户配置并热重载。
pub fn apply_patch(state: &ServerState, patch: Value) -> Result<Arc<LoadedConfig>, AppError> {
    let Some((request, path)) = state.config_write() else {
        return Err(AppError::new(
            ErrorCode::ConfigUnreadable,
            "本实例未挂载可写配置（只读运行或未提供用户配置文件）",
        ));
    };

    let toml_patch = json_to_toml(&patch)?;
    mc_config::write::apply_patch(request, path, &state.config, &toml_patch)
}

/// `target_ids` 补丁。空数组 = 不限制（全部可见目标）。
pub fn selection_patch(target_ids: &[String]) -> Value {
    serde_json::json!({ "capture": { "target_ids": target_ids } })
}

/// 从 `CaptureSource[]` / `string[]` / `{targets: [...]}` 里取出目标 id。
///
/// 三种形状都真实存在：`channel-map.ts` 发的是数组，
/// 界面可能发 `{targets: [...]}`，自动化脚本会发 `string[]`。
pub fn targets_from_body(body: &Value) -> Result<Vec<String>, AppError> {
    let items = match body {
        Value::Array(items) => items.as_slice(),
        Value::Object(map) => map
            .get("targets")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .ok_or_else(|| invalid("请求体缺少 targets 数组"))?,
        _ => return Err(invalid("请求体必须是数组或 {targets: [...]}")),
    };

    let mut ids: Vec<String> = Vec::with_capacity(items.len());
    for item in items {
        let id = match item {
            Value::String(id) => id.clone(),
            Value::Object(map) => map
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("数组元素缺少 id"))?
                .to_string(),
            _ => return Err(invalid("数组元素必须是字符串或含 id 的对象")),
        };
        if id.is_empty() {
            return Err(invalid("目标 id 不能为空"));
        }
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    Ok(ids)
}

/// 兼容 UI 的 `ScreenSettings` → 配置补丁。
///
/// 只翻译**出现过的**键：没给就不动，避免一次改时间把间隔重置回默认值。
pub fn capture_settings_patch(body: &Value) -> Result<Value, AppError> {
    let map = body
        .as_object()
        .ok_or_else(|| invalid("请求体必须是对象"))?;

    let mut capture = serde_json::Map::new();

    if let Some(interval) = map.get("recordInterval") {
        let seconds = interval
            .as_u64()
            .ok_or_else(|| invalid("recordInterval 必须是秒数"))?;
        if seconds == 0 {
            return Err(invalid("recordInterval 必须大于 0"));
        }
        capture.insert("interval_secs".to_string(), Value::from(seconds));
    }

    if let Some(enabled) = map.get("enableRecordingHours") {
        capture.insert(
            "enable_recording_hours".to_string(),
            Value::Bool(
                enabled
                    .as_bool()
                    .ok_or_else(|| invalid("enableRecordingHours 必须是布尔值"))?,
            ),
        );
    }

    if let Some(days) = map.get("applyToDays") {
        let text = days
            .as_str()
            .ok_or_else(|| invalid("applyToDays 必须是字符串"))?;
        match text {
            "weekday" | "everyday" => {
                capture.insert("apply_to_days".to_string(), Value::from(text));
            }
            other => {
                return Err(invalid(&format!(
                    "applyToDays 只接受 weekday / everyday，收到 {other:?}"
                )));
            }
        }
    }

    if let Some(region) = map.get("region") {
        match region {
            // null = 清除区域限制
            Value::Null => {
                capture.insert("region".to_string(), Value::Null);
            }
            Value::Array(items) if items.len() == 4 => {
                let mut values = [0i64; 4];
                for (index, item) in items.iter().enumerate() {
                    values[index] = item
                        .as_i64()
                        .ok_or_else(|| invalid("region 的元素必须是整数"))?;
                }
                if mc_capture::geometry::Rect::from_array(values).is_none() {
                    return Err(invalid("region 必须是 left<right 且 top<bottom 的矩形"));
                }
                capture.insert("region".to_string(), serde_json::json!(values.to_vec()));
            }
            // 兼容对象形式的区域 {left, top, width, height}
            Value::Object(region_map) => {
                let number = |key: &str| region_map.get(key).and_then(Value::as_i64);
                let (left, top, width, height) = match (
                    number("left"),
                    number("top"),
                    number("width"),
                    number("height"),
                ) {
                    (Some(left), Some(top), Some(width), Some(height)) => {
                        (left, top, width, height)
                    }
                    _ => return Err(invalid("region 对象需要 left/top/width/height")),
                };
                let values = [left, top, left + width, top + height];
                if mc_capture::geometry::Rect::from_array(values).is_none() {
                    return Err(invalid("region 的宽高必须为正"));
                }
                capture.insert("region".to_string(), serde_json::json!(values.to_vec()));
            }
            _ => {
                return Err(invalid(
                    "region 必须是 [left, top, right, bottom]、{left,top,width,height} 或 null",
                ))
            }
        }
    }

    if let Some(hours) = map.get("recordingHours") {
        if !hours.is_null() {
            let items = hours
                .as_array()
                .filter(|items| items.len() == 2)
                .ok_or_else(|| invalid("recordingHours 必须是两个时间的数组"))?;
            let start = items[0]
                .as_str()
                .ok_or_else(|| invalid("recordingHours 的元素必须是字符串"))?;
            let end = items[1]
                .as_str()
                .ok_or_else(|| invalid("recordingHours 的元素必须是字符串"))?;
            // 语法在配置层校验（读取时也会再判一次），这里只保证形状
            capture.insert(
                "recording_hours".to_string(),
                serde_json::json!([start, end]),
            );
        }
    }

    if capture.is_empty() {
        return Err(invalid("请求体里没有可识别的采集设置字段"));
    }

    Ok(serde_json::json!({ "capture": capture }))
}

/// JSON → TOML。JSON 的 `null` 在 TOML 里没有对应值，因此直接拒绝。
fn json_to_toml(value: &Value) -> Result<toml::Value, AppError> {
    toml::Value::try_from(value)
        .map_err(|error| invalid(&format!("补丁无法转换为 TOML（不支持的字段值）：{error}")))
}

fn invalid(reason: &str) -> AppError {
    AppError::new(ErrorCode::ConfigInvalid, reason.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_accepts_all_three_shapes() {
        let objects = serde_json::json!([{ "id": "display-1" }, { "id": "display-2" }]);
        assert_eq!(
            targets_from_body(&objects).unwrap(),
            vec!["display-1".to_string(), "display-2".to_string()]
        );

        let strings = serde_json::json!(["display-1"]);
        assert_eq!(
            targets_from_body(&strings).unwrap(),
            vec!["display-1".to_string()]
        );

        let wrapped = serde_json::json!({ "targets": [{ "id": "display-3" }] });
        assert_eq!(
            targets_from_body(&wrapped).unwrap(),
            vec!["display-3".to_string()]
        );
    }

    #[test]
    fn selection_deduplicates_and_rejects_empty_ids() {
        let duplicated = serde_json::json!(["display-1", "display-1"]);
        assert_eq!(
            targets_from_body(&duplicated).unwrap(),
            vec!["display-1".to_string()]
        );
        assert!(targets_from_body(&serde_json::json!([{ "name": "x" }])).is_err());
        assert!(targets_from_body(&serde_json::json!([""])).is_err());
    }

    #[test]
    fn settings_patch_only_contains_provided_keys() {
        let patch = capture_settings_patch(&serde_json::json!({ "recordInterval": 30 })).unwrap();
        let capture = patch["capture"].as_object().unwrap();
        assert_eq!(capture.len(), 1, "只翻译出现过的字段：{capture:?}");
        assert_eq!(capture["interval_secs"], 30);
    }

    #[test]
    fn settings_patch_rejects_bad_values() {
        assert!(capture_settings_patch(&serde_json::json!({ "recordInterval": 0 })).is_err());
        assert!(capture_settings_patch(&serde_json::json!({ "applyToDays": "someday" })).is_err());
        assert!(
            capture_settings_patch(&serde_json::json!({ "recordingHours": ["08:00:00"] })).is_err()
        );
        assert!(capture_settings_patch(&serde_json::json!({})).is_err());
    }
}
