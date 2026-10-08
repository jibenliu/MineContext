//! `check-version-consistency`：版本号只允许有一个事实来源。
//!
//! 版本号写在四处（workspace / 外壳 crate / 前端包 / Tauri 配置）时，漂移几乎是
//! 必然的：产物元数据、安装包名、前端包版本会各说各话，而发版时没人会对四遍。
//! 这里把它们读出来比对，不一致就报出「谁是多少」。

use std::fs;
use std::path::Path;

use regex::Regex;

/// 纯函数：给定（来源，版本）列表，判断是否一致。
pub fn evaluate(versions: &[(String, String)]) -> Result<(), String> {
    let Some((first_source, first_version)) = versions.first() else {
        return Err("没有读到任何版本号（检查器本身配错了）".to_string());
    };
    let mismatched: Vec<String> = versions
        .iter()
        .filter(|(_, version)| version != first_version)
        .map(|(source, version)| format!("{source} = {version}"))
        .collect();

    if mismatched.is_empty() {
        return Ok(());
    }
    Err(format!(
        "版本号不一致：{first_source} = {first_version}，但 {}",
        mismatched.join("、")
    ))
}

pub fn check(root: &Path) -> Result<(), String> {
    let versions = vec![
        (
            "Cargo.toml（workspace）".to_string(),
            read_cargo_version(&root.join("Cargo.toml"))?,
        ),
        (
            "src-tauri/Cargo.toml".to_string(),
            read_cargo_version(&root.join("src-tauri/Cargo.toml"))?,
        ),
        (
            "frontend/package.json".to_string(),
            read_json_version(&root.join("frontend/package.json"))?,
        ),
        (
            "src-tauri/tauri.conf.json".to_string(),
            read_json_version(&root.join("src-tauri/tauri.conf.json"))?,
        ),
    ];

    evaluate(&versions)
}

fn read_cargo_version(path: &Path) -> Result<String, String> {
    let text =
        fs::read_to_string(path).map_err(|error| format!("读不到 {}：{error}", path.display()))?;
    let pattern = Regex::new(r#"(?m)^version = "([^"]+)""#).expect("内置正则应当合法");
    pattern
        .captures(&text)
        .map(|capture| capture[1].to_string())
        .ok_or_else(|| format!("{} 里没有 version 字段", path.display()))
}

fn read_json_version(path: &Path) -> Result<String, String> {
    let text =
        fs::read_to_string(path).map_err(|error| format!("读不到 {}：{error}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| format!("{} 不是合法 JSON：{error}", path.display()))?;
    value
        .get("version")
        .and_then(|version| version.as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("{} 里没有 version 字段", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn versions(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(source, version)| (source.to_string(), version.to_string()))
            .collect()
    }

    #[test]
    fn consistent_versions_pass() {
        assert!(evaluate(&versions(&[
            ("Cargo.toml", "0.1.5"),
            ("frontend/package.json", "0.1.5"),
        ]))
        .is_ok());
    }

    #[test]
    fn mismatched_version_names_both_sides() {
        let error = evaluate(&versions(&[
            ("Cargo.toml", "0.1.5"),
            ("frontend/package.json", "0.1.0"),
        ]))
        .expect_err("必须报不一致");
        assert!(error.contains("0.1.5"), "{error}");
        assert!(error.contains("0.1.0"), "{error}");
        assert!(error.contains("frontend/package.json"), "{error}");
    }

    #[test]
    fn empty_input_is_a_checker_bug() {
        assert!(evaluate(&[]).is_err());
    }
}
