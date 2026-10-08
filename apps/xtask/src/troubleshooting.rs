//! `check-troubleshooting-freshness`：故障排查文档里的错误码与文案必须与
//! `ErrorCode` 一致 —— 新增错误码却忘了写进用户文档时，用户只会看到一句人话文案，
//! 这类漂移不会让任何测试变红。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use regex::Regex;

pub fn check(root: &Path) -> Result<(), String> {
    let source_path = root.join("crates/mc-common/src/error.rs");
    let doc_path = root.join("docs/troubleshooting.md");
    let source = read(&source_path)?;
    let doc = read(&doc_path)?;

    let variants = first_group_all(
        &source,
        r"Self::(\w+),",
        "pub const ALL: &'static [ErrorCode] = &[",
    )?;
    let codes = pairs(
        section(
            &source,
            "pub const fn as_str(self)",
            "pub const fn component",
        )?,
        r#"Self::(\w+) => "([a-z_]+)","#,
    )?;
    let messages = pairs(
        section(
            &source,
            "pub const fn user_message(self)",
            "pub const fn remediation",
        )?,
        r#"Self::(\w+) => \{?\s*"([^"]+)""#,
    )?;

    let mut problems = Vec::new();
    for variant in &variants {
        let (Some(code), Some(message)) = (codes.get(variant), messages.get(variant)) else {
            problems.push(format!("{variant}: 源码里缺少 as_str 或 user_message"));
            continue;
        };
        if !doc.contains(&format!("`{code}`")) {
            problems.push(format!("{code}（{variant}）没有出现在故障排查文档里"));
        } else if !doc.contains(message.as_str()) {
            problems.push(format!(
                "{code}: 文档里的文案与源码不一致（应为「{message}」）"
            ));
        }
    }

    // 反向：文档里有、源码里已经没有的码（改名后的残留）
    let documented = first_group_all(&doc, r"(?m)^\| `([a-z_]+)` \|", "")?;
    for code in documented.iter().collect::<BTreeSet<_>>() {
        if !codes.values().any(|value| value == code) {
            problems.push(format!(
                "{code}: 文档里有，但源码里已经没有这个错误码（改名残留？）"
            ));
        }
    }

    if problems.is_empty() {
        println!(
            "故障排查文档检查通过（{} 个错误码全部一致）",
            variants.len()
        );
        return Ok(());
    }

    let mut message = String::from("故障排查文档与错误码不一致：\n");
    for problem in &problems {
        message.push_str(&format!("  - {problem}\n"));
    }
    message.push_str("\n改完 error.rs 后请同步 docs/troubleshooting.md。");
    Err(message)
}

fn read(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|error| format!("无法读取 {}：{error}", path.display()))
}

/// 取 `start` 之后、`end` 之前的一段。
fn section<'a>(source: &'a str, start: &str, end: &str) -> Result<&'a str, String> {
    let rest = source
        .split_once(start)
        .map(|(_, rest)| rest)
        .ok_or_else(|| format!("源码里找不到 `{start}`"))?;
    Ok(rest.split_once(end).map(|(head, _)| head).unwrap_or(rest))
}

/// 在 `block`（为空表示整段文本）里按正则取每个匹配的第一个捕获组。
fn first_group_all(text: &str, pattern: &str, block_start: &str) -> Result<Vec<String>, String> {
    let scope = if block_start.is_empty() {
        text
    } else {
        let rest = text
            .split_once(block_start)
            .map(|(_, rest)| rest)
            .ok_or_else(|| format!("源码里找不到 `{block_start}`"))?;
        rest.split_once("];").map(|(head, _)| head).unwrap_or(rest)
    };
    let regex = Regex::new(pattern).map_err(|error| format!("内置正则非法：{error}"))?;
    Ok(regex
        .captures_iter(scope)
        .filter_map(|captures| captures.get(1).map(|value| value.as_str().to_string()))
        .collect())
}

/// `Self::变体 => "值",` 形态的映射。
fn pairs(text: &str, pattern: &str) -> Result<BTreeMap<String, String>, String> {
    let regex = Regex::new(pattern).map_err(|error| format!("内置正则非法：{error}"))?;
    Ok(regex
        .captures_iter(text)
        .filter_map(|captures| {
            Some((
                captures.get(1)?.as_str().to_string(),
                captures.get(2)?.as_str().to_string(),
            ))
        })
        .collect())
}
