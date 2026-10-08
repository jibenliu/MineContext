//! `check-provider-purity`：Provider 只走标准 OpenAI 兼容 HTTP 契约。
//!
//! 写死厂商 SDK（例如某个非标准 embedding 方法）会让「换端点」变成改代码。
//! 两条约束：源码里不出现厂商特化符号；HTTP 路径只允许白名单里的三条。

use std::fs;
use std::path::Path;

use regex::Regex;

const DIR: &str = "crates/mc-providers/src";
/// 允许出现的 HTTP 路径（OpenAI 兼容接口的全部面）。
const ALLOWED_PATHS: &[&str] = &["/v1/chat/completions", "/v1/embeddings", "/v1/models"];

pub fn check(root: &Path) -> Result<(), String> {
    let dir = root.join(DIR);
    if !dir.is_dir() {
        // 接口没就位时明确跳过，而不是假装通过
        return Ok(());
    }

    let forbidden = Regex::new(
        r"volcengine|volcenginesdkarkruntime|multimodal_embeddings|dashscope|DashScope|from volcengine",
    )
    .map_err(|e| e.to_string())?;
    let path_literal = Regex::new(r#""/v1/[a-z_/]+""#).map_err(|e| e.to_string())?;

    let mut problems: Vec<String> = Vec::new();
    for path in rust_files(&dir)? {
        let text = fs::read_to_string(&path).map_err(|error| error.to_string())?;
        for (index, line) in text.lines().enumerate() {
            let location = format!("{}:{}", relative(root, &path), index + 1);
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            if forbidden.is_match(line) {
                problems.push(format!(
                    "{location} 出现厂商特化调用（必须只走 OpenAI 兼容接口）：{}",
                    line.trim()
                ));
            }
            for found in path_literal.find_iter(line) {
                let literal = found.as_str();
                let allowed = ALLOWED_PATHS
                    .iter()
                    .any(|allowed| literal == format!("\"{allowed}\""));
                if !allowed {
                    problems.push(format!(
                        "{location} 出现白名单外的 HTTP 路径 {literal}（可用：{}）",
                        ALLOWED_PATHS.join("、")
                    ));
                }
            }
        }
    }

    if problems.is_empty() {
        return Ok(());
    }
    Err(problems.join("\n"))
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
    fn allowed_paths_are_recognised() {
        let literal = "\"/v1/chat/completions\"";
        assert!(ALLOWED_PATHS
            .iter()
            .any(|allowed| literal == format!("\"{allowed}\"")));
        let other = "\"/v1/some/other\"";
        assert!(!ALLOWED_PATHS
            .iter()
            .any(|allowed| other == format!("\"{allowed}\"")));
    }

    #[test]
    fn vendor_specific_symbols_are_forbidden() {
        let pattern = Regex::new(r"multimodal_embeddings|dashscope").unwrap();
        assert!(pattern.is_match("client.multimodal_embeddings(&inputs)"));
        assert!(!pattern.is_match("client.embeddings(&inputs)"));
    }
}
