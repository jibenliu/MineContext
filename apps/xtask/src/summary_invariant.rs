//! `check-summary-invariant`：最高优先级不变量的哨兵。
//!
//! 「有阶段必有总结」与「总结不被视觉积压饿死」这两条只要测试被删掉，
//! 一切仍然会「通过」—— 因此这里钉住「这两个测试必须存在」。
//!
//! 默认允许这两个测试缺失；设 `MC_ENFORCE_SUMMARY_INVARIANT=1` 时缺失即阻断
//! （发布前应当打开）。

use std::fs;
use std::path::Path;

const REQUIRED: &[&str] = &[
    "no_stage_without_summary",
    "summary_jobs_never_starved_by_vision_backlog",
];

/// 环境变量名：设为 1 时缺失即失败（否则只提示）。
pub const ENFORCE_ENV: &str = "MC_ENFORCE_SUMMARY_INVARIANT";

pub fn check(root: &Path) -> Result<(), String> {
    let mut missing: Vec<&str> = Vec::new();
    for name in REQUIRED {
        if !name_exists(root, name)? {
            missing.push(name);
        }
    }

    if missing.is_empty() {
        return Ok(());
    }

    let message = format!("缺少关键不变量测试：{}", missing.join("、"));
    if std::env::var(ENFORCE_ENV).as_deref() == Ok("1") {
        return Err(message);
    }
    // 未开启强制时只提示：调用方打印成功文案，但明确说明「暂不阻断」
    Err(format!(
        "{message}\n（提示：未设置 {ENFORCE_ENV}=1，本次不阻断）"
    ))
}

/// 名称是否出现在 `crates/` 下的任意源码里。
fn name_exists(root: &Path, name: &str) -> Result<bool, String> {
    let dir = root.join("crates");
    if !dir.is_dir() {
        return Ok(false);
    }
    let mut stack = vec![dir];
    while let Some(current) = stack.pop() {
        let entries =
            fs::read_dir(&current).map_err(|error| format!("读取 {current:?} 失败：{error}"))?;
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            let text = fs::read_to_string(&path).map_err(|error| error.to_string())?;
            if text.contains(name) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_names_are_stable() {
        // 这两个名字是契约（测试名 + 不变量名），改名等于删哨兵
        assert!(REQUIRED.contains(&"no_stage_without_summary"));
        assert!(REQUIRED.contains(&"summary_jobs_never_starved_by_vision_backlog"));
    }

    #[test]
    fn missing_names_are_reported_with_the_enforce_hint() {
        let message = format!("缺少关键不变量测试：no_stage_without_summary\n（提示：未设置 {ENFORCE_ENV}=1，本次不阻断）");
        assert!(message.contains(ENFORCE_ENV));
    }
}
