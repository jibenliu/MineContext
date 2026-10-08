//! `check-log-redaction`：日志里**不许出现内容、密钥、路径**，也不许绕过统一出口。
//!
//! 三条规则：只能用 `mc_common::observability` 的宏（绕过出口就绕过了脱敏）；字段名与
//! 格式化参数里不许出现 content / prompt / token / path / dir 这类值，也不许
//! `.display()`（路径）与 `.detail()`（未脱敏原文）；每个 crate 要么有日志点，要么在
//! `scripts/log-coverage.txt` 里写明理由。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;

const MACROS: &[&str] = &["info", "warn", "error", "debug", "trace"];

pub fn check(root: &Path) -> Result<(), String> {
    // 字段名里出现这些词就说明写日志的人在传「值」而不是「事实」。
    let forbidden_fields = Regex::new(
        r"(?i)\b(api_key|apikey|token|secret|password|credential|authorization|content|prompt|body|ocr_text|window_title|transcript|file_path|filepath|path|dir|directory|data_dir|url|query)\b\s*=",
    )
    .expect("内置正则应当合法");
    // 直接在日志里求值路径 / 未脱敏错误。
    let forbidden_expr = Regex::new(r"\.display\(\)|\.detail\(\)").expect("内置正则应当合法");
    // 直接调 tracing（而不是走 mc-common 的再导出）。
    let direct_tracing =
        Regex::new(r"\btracing::(?:info|warn|error|debug|trace)!").expect("内置正则应当合法");
    // 格式化参数里的可疑标识符。`prompt_tokens` 这类计数不该被误伤。
    let suspect_arg = Regex::new(r"(?i)\b(path|dir|content|prompt|token|key|secret|body)\b")
        .expect("内置正则应当合法");
    let format_args = Regex::new(r#""[^"]*\{\}[^"]*"\s*,\s*([^;]+)"#).expect("内置正则应当合法");

    let mut roots = Vec::new();
    for dir in ["crates", "apps"] {
        roots.extend(subdirs(&root.join(dir), "src")?);
    }
    roots.sort();

    let mut problems = Vec::new();
    let mut crates_with_logs: BTreeSet<String> = BTreeSet::new();

    for root_dir in &roots {
        let crate_name = root_dir
            .parent()
            .map(|parent| display(root, parent))
            .unwrap_or_default();
        for path in rust_files(root_dir)? {
            let text = fs::read_to_string(&path)
                .map_err(|error| format!("无法读取 {}：{error}", path.display()))?;

            for found in direct_tracing.find_iter(&text) {
                let line = text[..found.start()].matches('\n').count() + 1;
                problems.push(format!(
                    "{}:{line}: 直接调用 tracing 宏 —— 日志必须走 mc_common::observability（否则绕过脱敏）",
                    display(root, &path)
                ));
            }

            let calls = calls(&text);
            if !calls.is_empty() {
                crates_with_logs.insert(crate_name.clone());
            }
            for (line, call) in calls {
                if let Some(hit) = forbidden_fields.captures(&call) {
                    let field = hit.get(1).map(|value| value.as_str()).unwrap_or_default();
                    problems.push(format!(
                        "{}:{line}: 日志字段 `{field}` 是内容/密钥/路径，不该进日志",
                        display(root, &path)
                    ));
                }
                if forbidden_expr.is_match(&call) {
                    problems.push(format!(
                        "{}:{line}: 日志里出现 `.display()` 或 `.detail()` —— 路径与未脱敏原文要用 observability 的脱敏函数",
                        display(root, &path)
                    ));
                }
                // 格式化参数：宏调用里 `"…{}"` 之后的实参。
                // 注意：这里**不能**写出「宏名 + 叹号 + 左括号」的字面量 —— 这条守卫
                // 扫的就是 `apps/*/src`，写出来它会把本文件的注释当成日志点，于是
                // `apps/xtask` 会被判定「有日志」，与 log-coverage 的登记冲突。
                for captures in format_args.captures_iter(&call) {
                    let args = captures
                        .get(1)
                        .map(|value| value.as_str())
                        .unwrap_or_default();
                    if let Some(suspect) = suspect_arg.captures(args) {
                        let name = suspect
                            .get(1)
                            .map(|value| value.as_str())
                            .unwrap_or_default();
                        if !args.contains("redact") && !args.contains("summary") {
                            problems.push(format!(
                                "{}:{line}: 格式化参数里出现 `{name}`，先过 redact_text / error_summary",
                                display(root, &path)
                            ));
                        }
                    }
                }
            }
        }
    }

    let coverage = root.join("scripts/log-coverage.txt");
    let mut registered: BTreeMap<String, String> = BTreeMap::new();
    if coverage.is_file() {
        let text = fs::read_to_string(&coverage)
            .map_err(|error| format!("无法读取 {}：{error}", coverage.display()))?;
        for (index, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, _, reason) = match line.split_once('#') {
                Some((key, reason)) => (key.trim(), "#", reason),
                None => (line, "", ""),
            };
            if key.is_empty() {
                continue;
            }
            if reason.trim().is_empty() {
                problems.push(format!(
                    "{}:{}: 「{key}」没有写明理由",
                    display(root, &coverage),
                    index + 1
                ));
                continue;
            }
            registered.insert(key.to_string(), reason.trim().to_string());
        }
    }

    let all_crates: BTreeSet<String> = roots
        .iter()
        .filter_map(|root_dir| root_dir.parent().map(|parent| display(root, parent)))
        .collect();
    for name in all_crates
        .difference(&crates_with_logs)
        .filter(|name| !registered.contains_key(*name))
    {
        problems.push(format!(
            "{name}: 没有任何日志点，也没有在 {} 里登记理由",
            display(root, &coverage)
        ));
    }
    for name in registered
        .keys()
        .filter(|name| crates_with_logs.contains(*name))
    {
        problems.push(format!(
            "{name}: 已经有日志点了，请从 {} 删除这条陈旧登记",
            display(root, &coverage)
        ));
    }

    if problems.is_empty() {
        println!(
            "日志卫生检查通过（{} 个 crate 有日志点，{} 个登记为「不直接记日志」）",
            crates_with_logs.len(),
            registered.len()
        );
        return Ok(());
    }

    let mut message = String::from("日志卫生检查未通过：\n");
    for problem in &problems {
        message.push_str(&format!("  - {problem}\n"));
    }
    Err(message)
}

/// 从 `name!(` 开始按括号配对取出整个调用（可能跨行）。
fn calls(text: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut found = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        for name in MACROS {
            let opening = format!(r"\b{name}!\(");
            let Ok(pattern) = Regex::new(&opening) else {
                continue;
            };
            if !pattern.is_match(line) {
                continue;
            }
            if let Some(call) = macro_call(&lines, index, name) {
                found.push((index + 1, call));
            }
            break;
        }
    }
    found
}

fn macro_call(lines: &[&str], index: usize, name: &str) -> Option<String> {
    let text = lines[index..].join("\n");
    let start = text.find(&format!("{name}!("))?;
    let mut depth = 0usize;
    for (offset, character) in text[start..].char_indices() {
        match character {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(text[start..start + offset + 1].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn subdirs(base: &Path, name: &str) -> Result<Vec<PathBuf>, String> {
    let entries =
        fs::read_dir(base).map_err(|error| format!("无法读取 {}：{error}", base.display()))?;
    let mut found: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path().join(name))
        .filter(|path| path.is_dir())
        .collect();
    found.sort();
    Ok(found)
}

fn rust_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = fs::read_dir(&current)
            .map_err(|error| format!("无法读取 {}：{error}", current.display()))?;
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                found.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}
