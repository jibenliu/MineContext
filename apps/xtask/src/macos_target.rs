//! `check-macos-target`：最低支持 macOS 13、同时覆盖 14，三处必须一致 ——
//! 构建目标（`.cargo/config.toml`）、运行时常量（Rust 测试覆盖）、CI 矩阵。
//! 少一处就会出现「文档说支持、二进制不支持」。

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

    // ---- 2) CI 矩阵 ----
    let workflow = root.join(".github/workflows/rust.yml");
    match fs::read_to_string(&workflow) {
        Err(_) => problems.push("缺少 .github/workflows/rust.yml".to_string()),
        Ok(text) => {
            for runner in ["macos-13", "macos-14"] {
                if !text.contains(runner) {
                    problems.push(format!(
                        "CI 测试矩阵里没有 {runner}（用户要求 13 与 14 都要覆盖）"
                    ));
                }
            }
        }
    }

    if problems.is_empty() {
        let version = target
            .map(|(major, minor)| format!("{major}.{minor}"))
            .unwrap_or_else(|| "未设置".to_string());
        println!(
            "macOS 支持范围检查通过（deployment target {version}，CI 覆盖 macos-13 / macos-14）"
        );
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
        let entries =
            fs::read_dir(&base).map_err(|error| format!("无法读取 {}：{error}", base.display()))?;
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path().join("Cargo.toml");
            if path.is_file() {
                found.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}
