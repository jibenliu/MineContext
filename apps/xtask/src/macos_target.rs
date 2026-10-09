//! `check-macos-target`：产品最低支持 macOS 13，CI 托管 runner 用 14+。
//!
//! 三处必须一致（少一处就会出现「文档说支持、二进制不支持」）：
//!
//! 1. 构建：`MACOSX_DEPLOYMENT_TARGET` ≥ 13.0（`.cargo/config.toml`）
//! 2. 运行：`mc_common::platform::MIN_SUPPORTED_MACOS`（由 Rust 测试覆盖）
//! 3. CI：test 矩阵覆盖 `macos-14`（或更高），且**不得**再依赖已退场的 `macos-13` 托管标签

use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;

const MIN_MAJOR: u64 = 13;
const MIN_MINOR: u64 = 0;

pub fn check(root: &Path) -> Result<(), String> {
    let pattern = Regex::new(r#"MACOSX_DEPLOYMENT_TARGET\s*=\s*"([0-9]+)\.([0-9]+)""#)
        .expect("内置正则应当合法");
    let mut problems = Vec::new();

    // ---- 1) 构建目标 ----
    let config = root.join(".cargo/config.toml");
    let mut target: Option<(u64, u64)> = None;
    match fs::read_to_string(&config) {
        Err(_) => problems
            .push("缺少 .cargo/config.toml：没有显式声明 MACOSX_DEPLOYMENT_TARGET".to_string()),
        Ok(text) => match pattern.captures(&text) {
            None => problems.push(".cargo/config.toml 里没有 MACOSX_DEPLOYMENT_TARGET".to_string()),
            Some(captures) => {
                let value = pair(&captures);
                if value < (MIN_MAJOR, MIN_MINOR) {
                    problems.push(format!(
                        "MACOSX_DEPLOYMENT_TARGET={}.{} 低于最低要求 {MIN_MAJOR}.{MIN_MINOR}",
                        value.0, value.1
                    ));
                }
                target = Some(value);
            }
        },
    }

    // crate 不得私自调低（[env] 是全局的，但 Cargo.toml 里也可能出现 env 段落）
    for manifest in manifests(root)? {
        let text = fs::read_to_string(&manifest)
            .map_err(|error| format!("无法读取 {}：{error}", manifest.display()))?;
        if let Some(captures) = pattern.captures(&text) {
            let value = pair(&captures);
            if value < (MIN_MAJOR, MIN_MINOR) {
                problems.push(format!(
                    "{} 把部署目标调低到 {}.{}",
                    manifest.display(),
                    value.0,
                    value.1
                ));
            }
        }
    }

    // ---- 2) CI 矩阵：托管 runner ≥ 14；禁止再写死 macos-13 ----
    let workflow = root.join(".github/workflows/rust.yml");
    match fs::read_to_string(&workflow) {
        Err(_) => problems.push("缺少 .github/workflows/rust.yml".to_string()),
        Ok(text) => {
            // 只看非注释行：注释里可以提到旧标签，矩阵里不能再写。
            let code_lines: String = text
                .lines()
                .filter(|line| !line.trim_start().starts_with('#'))
                .collect::<Vec<_>>()
                .join("\n");
            if code_lines.contains("macos-13") {
                problems.push(
                    "CI 测试矩阵仍依赖 macos-13（该托管标签已退场，会永久排队）；请改为 macos-14 或更高"
                        .to_string(),
                );
            }
            let has_modern = ["macos-14", "macos-15", "macos-latest"]
                .iter()
                .any(|runner| code_lines.contains(runner));
            if !has_modern {
                problems.push(
                    "CI 测试矩阵里没有 macos-14 / macos-15 / macos-latest（托管 CI 须覆盖 14+）"
                        .to_string(),
                );
            }
        }
    }

    if problems.is_empty() {
        let version = target
            .map(|(major, minor)| format!("{major}.{minor}"))
            .unwrap_or_else(|| "未设置".to_string());
        println!("macOS 支持范围检查通过（deployment target {version}，CI runner ≥ macos-14）");
        return Ok(());
    }

    let mut message = String::from("macOS 支持范围检查失败：\n");
    for problem in &problems {
        message.push_str(&format!("  - {problem}\n"));
    }
    Err(message)
}

fn pair(captures: &regex::Captures<'_>) -> (u64, u64) {
    let value = |index: usize| {
        captures
            .get(index)
            .and_then(|part| part.as_str().parse().ok())
            .unwrap_or(0)
    };
    (value(1), value(2))
}

fn manifests(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut found = Vec::new();
    for dir in ["crates", "apps"] {
        let base = root.join(dir);
        if !base.is_dir() {
            continue;
        }
        for entry in fs::read_dir(&base).map_err(|error| format!("无法读 {dir}/：{error}"))? {
            let entry = entry.map_err(|error| format!("无法读目录项：{error}"))?;
            let manifest = entry.path().join("Cargo.toml");
            if manifest.is_file() {
                found.push(manifest);
            }
        }
    }
    Ok(found)
}
