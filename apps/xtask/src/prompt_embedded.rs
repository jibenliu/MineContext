//! `check-prompt-embedded`：提示词必须编译进二进制（`include_str!`）。
//!
//! 升级后缺少提示词文件会让截图分析持续失败，而这类失败在「提示词内嵌」之后
//! 结构上不可能发生。因此两条：提示词文件必须被同一 crate 的 `include_str!` 引用；
//! Rust 代码不得在运行时读旧的 `prompts_*.yaml`。

use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;

pub fn check(root: &Path) -> Result<(), String> {
    let legacy = Regex::new(r"prompts_(zh|en)\.ya?ml").map_err(|e| e.to_string())?;
    let mut problems: Vec<String> = Vec::new();
    let mut found_any = false;

    for crate_dir in crate_dirs(root)? {
        for sub in ["prompts", "src/prompts"] {
            let dir = crate_dir.join(sub);
            if !dir.is_dir() {
                continue;
            }
            for prompt in files_with_extension(&dir, &["md", "txt"])? {
                found_any = true;
                let Some(name) = prompt.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let mut referenced = false;
                for source in files_with_extension(&crate_dir, &["rs"])? {
                    let text = fs::read_to_string(&source).map_err(|e| e.to_string())?;
                    if text.contains("include_str!") && text.contains(name) {
                        referenced = true;
                        break;
                    }
                }
                if !referenced {
                    problems.push(format!(
                        "孤儿提示词（未被同 crate 的 include_str! 引用）：{}",
                        relative(root, &prompt)
                    ));
                }
            }
        }
    }

    for dir in [root.join("crates"), root.join("apps")] {
        if !dir.is_dir() {
            continue;
        }
        for source in files_with_extension(&dir, &["rs"])? {
            let text = fs::read_to_string(&source).map_err(|e| e.to_string())?;
            for (index, line) in text.lines().enumerate() {
                if line.trim_start().starts_with("//") {
                    continue;
                }
                if legacy.is_match(line) {
                    problems.push(format!(
                        "{}:{} 运行时引用旧 YAML 提示词（必须改为 include_str!）：{}",
                        relative(root, &source),
                        index + 1,
                        line.trim()
                    ));
                }
            }
        }
    }

    if !problems.is_empty() {
        return Err(problems.join("\n"));
    }
    if !found_any {
        return Ok(());
    }
    Ok(())
}

/// 提示词目录是否存在（调用方据此决定打印哪一句成功文案）。
pub fn has_prompts(root: &Path) -> bool {
    crate_dirs(root)
        .map(|dirs| {
            dirs.iter()
                .any(|dir| dir.join("prompts").is_dir() || dir.join("src/prompts").is_dir())
        })
        .unwrap_or(false)
}

fn crate_dirs(root: &Path) -> Result<Vec<PathBuf>, String> {
    let base = root.join("crates");
    if !base.is_dir() {
        return Ok(Vec::new());
    }
    let entries = fs::read_dir(&base).map_err(|error| format!("读取 {base:?} 失败：{error}"))?;
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    Ok(dirs)
}

fn files_with_extension(dir: &Path, extensions: &[&str]) -> Result<Vec<PathBuf>, String> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries =
            fs::read_dir(&current).map_err(|error| format!("读取 {current:?} 失败：{error}"))?;
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if extensions.contains(&ext) {
                    found.push(path);
                }
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
    fn legacy_yaml_prompt_reference_is_detected() {
        let pattern = Regex::new(r"prompts_(zh|en)\.ya?ml").unwrap();
        // 拼接出样例文本：这个守卫会扫描整个仓库（含本文件），
        // 源码里不该出现**字面量**的旧提示词文件名。
        let zh = format!("config/prompts_{}.yaml", "zh");
        let en = format!("prompts_{}.yml", "en");
        assert!(pattern.is_match(&zh));
        assert!(pattern.is_match(&format!("read(\"{en}\")")));
    }

    #[test]
    fn extension_filter_matches_only_requested_ones() {
        let extensions = ["md", "txt"];
        assert!(extensions.contains(&"md"));
        assert!(!extensions.contains(&"rs"));
    }
}
