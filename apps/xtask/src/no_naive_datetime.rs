//! `check-no-naive-datetime`：领域层只允许一种时间表示（`mc_common::time::Timestamp`）。
//!
//! naive 与 timezone-aware 混比会抛 `TypeError`，导致上下文被静默丢弃；把这条
//! 约束做成门禁，比写在文档里可靠。注释里出现这些类型名是允许的（要解释为什么禁止），
//! 因此**整行注释**先被排除，只检查真正的代码。

use std::fs;
use std::path::Path;

use regex::Regex;

/// 领域层与公共层：这两处一旦引入 naive 时间，问题会扩散到所有上层。
const TARGETS: &[&str] = &["crates/mc-domain/src", "crates/mc-common/src"];

pub fn check(root: &Path) -> Result<(), String> {
    let pattern = Regex::new(r"\bNaiveDateTime\b|\bSystemTime::now\b|\bLocal::now\b")
        .map_err(|error| format!("正则编译失败：{error}"))?;
    let mut offenders: Vec<String> = Vec::new();

    for target in TARGETS {
        let dir = root.join(target);
        if !dir.is_dir() {
            continue;
        }
        for path in rust_files(&dir)? {
            let text = fs::read_to_string(&path).map_err(|error| error.to_string())?;
            for (index, line) in text.lines().enumerate() {
                if line.trim_start().starts_with("//") {
                    continue;
                }
                if pattern.is_match(line) {
                    offenders.push(format!(
                        "{}:{}: {}",
                        relative(root, &path),
                        index + 1,
                        line.trim()
                    ));
                }
            }
        }
    }

    if offenders.is_empty() {
        return Ok(());
    }
    Err(format!(
        "发现被禁止的时间用法（领域层只能用 mc_common::time::Timestamp）：\n  {}",
        offenders.join("\n  ")
    ))
}

fn rust_files(dir: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries =
            fs::read_dir(&current).map_err(|error| format!("读取 {current:?} 失败：{error}"))?;
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                found.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn naive_datetime_in_code_is_an_offender_but_not_in_a_comment() {
        let pattern = Regex::new(r"\bNaiveDateTime\b").unwrap();
        assert!(pattern.is_match("let x: NaiveDateTime = now();"));
        // 整行注释不参与匹配（守卫先过滤注释行）
        assert!("// NaiveDateTime 是禁止的".trim_start().starts_with("//"));
    }
}
