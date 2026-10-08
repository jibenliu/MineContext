//! `check-theme-tokens`：前端样式里的颜色变量必须**能被解析**、并且**随主题走**。
//!
//! 两条规则拦的都是「静默失效」型缺陷 —— 浏览器不报错，只是这条声明不生效：
//!
//! 1. `var(--x)` 的 `--x` 必须在源码里有定义：名字写错、或引用别的设计系统的变量时，
//!    整条声明作废（底色变透明、文字色退回继承）。
//! 2. Arco 的调色板变量（`--primary-6` 等）存的是 `R, G, B` 三元组，**不是颜色**，
//!    必须写成 `rgb(var(--primary-6))`；裸用时实心按钮变透明、白字看不见。
//!
//! 只扫 `frontend/src`（含 `assets/theme/*.less` 主题源，变量定义从那里取）。

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;

/// Arco 调色板：这些前缀 + 档位数字在主题里都是 `R, G, B` 三元组。
const PALETTES: [&str; 20] = [
    "red",
    "orangered",
    "orange",
    "gold",
    "yellow",
    "lime",
    "green",
    "cyan",
    "blue",
    "arcoblue",
    "purple",
    "pinkpurple",
    "magenta",
    "gray",
    "success",
    "primary",
    "danger",
    "warning",
    "link",
    "data",
];

/// 运行时才注入的变量：Radix 把尺寸写在元素行内样式上，Tailwind 自己维护 `--tw-*`。
const RUNTIME_PREFIXES: [&str; 2] = ["--radix-", "--tw-"];

const SCAN_EXTENSIONS: [&str; 5] = ["tsx", "ts", "css", "less", "html"];

pub fn check(root: &Path) -> Result<(), String> {
    let files = files_with_extension(&root.join("frontend/src"), &SCAN_EXTENSIONS)?;

    let mut sources: Vec<(String, String)> = Vec::new();
    let mut defined: BTreeSet<String> = BTreeSet::new();
    for file in &files {
        let text = fs::read_to_string(file)
            .map_err(|error| format!("读取 {} 失败：{error}", file.display()))?;
        defined.extend(defined_names(&text, file));
        sources.push((relative(root, file), text));
    }

    let mut problems: Vec<String> = Vec::new();
    for (name, text) in &sources {
        problems.extend(violations(name, text, &defined));
    }

    if problems.is_empty() {
        return Ok(());
    }
    Err(problems.join("\n"))
}

/// 单个文件里的违规（纯函数：给定定义集合即可判定，便于单测）。
fn violations(display: &str, text: &str, defined: &BTreeSet<String>) -> Vec<String> {
    let mut problems = Vec::new();

    for capture in usage_regex().captures_iter(text) {
        let name = &capture[1];
        if RUNTIME_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
        {
            continue;
        }
        if !defined.contains(name.trim_start_matches('-')) {
            problems.push(format!(
                "{display}:{} 引用了未定义的样式变量 {name}",
                line_of(text, capture.get(0).map_or(0, |m| m.start()))
            ));
        }
    }

    for capture in triplet_regex().captures_iter(text) {
        if capture.get(1).is_some() {
            continue;
        }
        let name = &capture[2];
        problems.push(format!(
            "{display}:{} {name} 是 Arco 的 `R, G, B` 三元组，不是颜色，必须写成 rgb({name})",
            line_of(text, capture.get(0).map_or(0, |m| m.start()))
        ));
    }

    problems
}

fn usage_regex() -> Regex {
    Regex::new(r"var\((--[A-Za-z0-9-]+)").expect("用法正则固定")
}

fn define_regex() -> Regex {
    Regex::new(r"(--[A-Za-z0-9-]+)\s*:").expect("定义正则固定")
}

/// 主题源里的定义带前缀占位符：`@{arco-cssvars-prefix}-color-bg-2: …`。
fn less_define_regex() -> Regex {
    Regex::new(r"\{arco-cssvars-prefix\}-([A-Za-z0-9-]+)\s*:").expect("主题定义正则固定")
}

fn triplet_regex() -> Regex {
    let palettes = PALETTES.join("|");
    Regex::new(&format!(
        r"(?:(rgb|rgba)\(\s*)?var\((--(?:{palettes})-[0-9]+)\)"
    ))
    .expect("三元组正则由固定词表拼出")
}

fn defined_names(text: &str, path: &Path) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = define_regex()
        .captures_iter(text)
        .map(|capture| capture[1].trim_start_matches('-').to_string())
        .collect();
    if path.extension().and_then(|extension| extension.to_str()) == Some("less") {
        names.extend(
            less_define_regex()
                .captures_iter(text)
                .map(|capture| capture[1].to_string()),
        );
    }
    names
}

fn line_of(text: &str, offset: usize) -> usize {
    text[..offset].matches('\n').count() + 1
}

fn files_with_extension(dir: &Path, extensions: &[&str]) -> Result<Vec<PathBuf>, String> {
    let mut found = Vec::new();
    if !dir.is_dir() {
        return Ok(found);
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries =
            fs::read_dir(&current).map_err(|error| format!("读取 {current:?} 失败：{error}"))?;
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
                if extensions.contains(&extension) {
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

    fn undefined_check(text: &str) -> Vec<String> {
        violations("probe.css", text, &BTreeSet::new())
    }

    /// 只关心「三元组」规则时，把用到的变量先声明成已定义，避免混淆两条规则。
    fn with_defined(text: &str, names: &[&str]) -> Vec<String> {
        let defined: BTreeSet<String> = names.iter().map(|name| name.to_string()).collect();
        violations("probe.css", text, &defined)
    }

    #[test]
    fn unknown_variable_is_reported() {
        let problems = undefined_check(".a { color: var(--text-color-text-1); }");
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("--text-color-text-1"), "{problems:?}");
        assert!(problems[0].contains("probe.css:1"), "{problems:?}");
    }

    #[test]
    fn defined_variable_passes() {
        let mut defined = BTreeSet::new();
        defined.insert("color-text-1".to_string());
        let problems = violations("probe.css", ".a { color: var(--color-text-1); }", &defined);
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn runtime_injected_variable_is_ignored() {
        assert!(undefined_check(".a { width: var(--radix-select-trigger-width); }").is_empty());
        assert!(undefined_check(".a { color: var(--tw-ring-color); }").is_empty());
    }

    #[test]
    fn bare_palette_token_is_reported() {
        // 裸用三元组：声明会失效 —— 实心按钮变透明、文字看不见
        let problems = with_defined(".a { background-color: var(--primary-6); }", &["primary-6"]);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("rgb(--primary-6)"), "{problems:?}");
    }

    #[test]
    fn wrapped_palette_token_passes() {
        assert!(with_defined(
            ".a { background-color: rgb(var(--primary-6)); }",
            &["primary-6"]
        )
        .is_empty());
        assert!(with_defined(
            ".a { border-color: rgba(var(--danger-6), .2); }",
            &["danger-6"]
        )
        .is_empty());
    }

    #[test]
    fn palette_token_inside_other_function_is_reported() {
        // color-mix 的第一个参数要求是颜色，三元组同样不成立
        let problems = with_defined(
            ".a { background: color-mix(in srgb, var(--primary-6) 18%, transparent); }",
            &["primary-6"],
        );
        assert_eq!(problems.len(), 1, "{problems:?}");
    }

    #[test]
    fn non_palette_single_letter_names_are_not_triplets() {
        // `--mc-brand` 这类自建变量是真颜色，不该被三元组规则误报
        let mut defined = BTreeSet::new();
        defined.insert("mc-brand".to_string());
        let problems = violations("probe.css", ".a { color: var(--mc-brand); }", &defined);
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn theme_less_definitions_are_recognised() {
        let names = defined_names(
            "@{arco-cssvars-prefix}-color-bg-2: #fafbfd;",
            Path::new("t.less"),
        );
        assert!(names.contains("color-bg-2"), "{names:?}");
    }

    #[test]
    fn line_numbers_follow_the_usage() {
        let problems = undefined_check("a{}\n.b { color: var(--nope-1); }\n");
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains(":2"), "{problems:?}");
    }
}
