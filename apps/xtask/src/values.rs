//! 两个给 shell 脚本用的小工具：
//! `json-field <file> <key>` 取 JSON 顶层字段（runtime.json 的 port/token），
//! `now-ms` 打印当前毫秒时间戳（benchmark 计时用）。

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// 取 JSON 字段；`key` 支持点号路径（`data.id`）。
pub fn json_field(path: &Path, key: &str) -> Result<String, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("无法读取 {}：{error}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| format!("{} 不是合法 JSON：{error}", path.display()))?;

    let mut current = &value;
    for part in key.split('.') {
        current = current
            .get(part)
            .ok_or_else(|| format!("{} 里没有字段 `{key}`", path.display()))?;
    }
    match current {
        serde_json::Value::String(text) => Ok(text.clone()),
        serde_json::Value::Number(number) => Ok(number.to_string()),
        other => Err(format!("字段 `{key}` 不是字符串或数字：{other}")),
    }
}

pub fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0)
}

/// 输出 p50 / p95 / max，格式与基准脚本一致。
///
/// 关键在 `round`：这里必须用**银行家舍入**（0.5 向偶数），Rust 的 `f64::round`
/// 是四舍五入 —— 直接换会让样本数落在边界时差一格，基准数据就不可比了。
pub fn percentiles(values: &[u64]) -> String {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let pick = |p: f64| -> u64 {
        let last = sorted.len().saturating_sub(1);
        let index = py_round(last as f64 * p).clamp(0, last as i64) as usize;
        sorted[index]
    };
    format!(
        "  p50 {}ms · p95 {}ms · max {}ms · n={}",
        pick(0.5),
        pick(0.95),
        sorted.last().copied().unwrap_or(0),
        sorted.len()
    )
}

fn py_round(value: f64) -> i64 {
    let floor = value.floor();
    if (value - floor - 0.5).abs() < 1e-9 {
        let candidate = floor as i64;
        if candidate % 2 == 0 {
            candidate
        } else {
            candidate + 1
        }
    } else {
        value.round() as i64
    }
}

/// 诊断接口的字段齐全性（可观测性回归）：列出缺哪些字段。
pub fn diag_fields(json: &str) -> Result<String, String> {
    const KEYS: &[&str] = &[
        "platform",
        "retention",
        "components",
        "queues",
        "invariants",
    ];
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| format!("不是可解析的 JSON：{error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "诊断响应不是 JSON 对象".to_string())?;
    let missing: Vec<String> = KEYS
        .iter()
        .filter(|key| !object.contains_key(**key))
        .map(|key| format!("'{key}'"))
        .collect();
    if missing.is_empty() {
        Ok("  诊断字段: 齐全".to_string())
    } else {
        Ok(format!("  诊断字段: 缺少 [{}]", missing.join(", ")))
    }
}
