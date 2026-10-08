//! `check-no-test-fixtures`：生产代码里不得出现测试夹具（固定时间基准、示例文案），
//! 测试目录里的固定时间基准必须来自 `mc-testkit`。
//!
//! 夹具值看起来无害，一旦漏进 `src/**` 就成了业务行为的一部分 —— 例如某处「默认
//! 活动标题」变成测试用的 `活动 xxx`，而它在测试里永远不会被发现。

use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;

/// 夹具特征：固定时间基准与示例文案。
const FIXTURE_PATTERNS: &[(&str, &str)] = &[
    (
        r"1_790_758_800_000",
        "固定时间基准字面量（请用 mc_testkit::fixtures）",
    ),
    (
        r"\b1790758800000\b",
        "固定时间基准字面量（请用 mc_testkit::fixtures）",
    ),
    (
        r#"Some\("开发""#,
        "示例分类字面量（请用 mc_testkit::fixtures::SAMPLE_CATEGORY）",
    ),
    (
        r#"format!\("活动 "#,
        "示例标题字面量（请用 mc_testkit::fixtures::sample_activity）",
    ),
];

const TIME_BASE: &str = r"1_790_758_800_000|\b1790758800000\b";

pub fn check(root: &Path) -> Result<(), String> {
    let patterns = FIXTURE_PATTERNS
        .iter()
        .map(|(pattern, reason)| {
            Regex::new(pattern)
                .map(|regex| (regex, *reason))
                .map_err(|error| format!("内置正则非法：{error}"))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let time_base = Regex::new(TIME_BASE).expect("内置正则应当合法");

    // ---- 生产代码 ----
    let mut problems = Vec::new();
    for dir in ["crates", "apps"] {
        for root_dir in subdirs(&root.join(dir), "src")? {
            if is_testkit(&root_dir) {
                continue;
            }
            for path in rust_files(&root_dir)? {
                for (number, line) in lines(&path)? {
                    if is_comment(&line) {
                        continue;
                    }
                    for (pattern, reason) in &patterns {
                        if pattern.is_match(&line) {
                            problems.push(format!(
                                "{}:{number}: {reason}\n    {}",
                                path.display(),
                                line.trim()
                            ));
                        }
                    }
                }
            }
        }
    }

    // ---- 测试目录：固定时间基准必须来自 mc-testkit，不得各写一份 ----
    //
    // 固定时间基准只能有一处来源（`mc-testkit`）：同一个值在几十个测试文件里
    // 各写一次时，改基准必然漏掉几个。
    let mut test_problems = Vec::new();
    for dir in ["crates", "apps"] {
        for root_dir in subdirs(&root.join(dir), "tests")? {
            if is_testkit(&root_dir) {
                continue;
            }
            for path in rust_files(&root_dir)? {
                for (number, line) in lines(&path)? {
                    if is_comment(&line) {
                        continue;
                    }
                    if time_base.is_match(&line) {
                        test_problems.push(format!("{}:{number}: {}", path.display(), line.trim()));
                    }
                }
            }
        }
    }

    if !test_problems.is_empty() {
        let mut message =
            String::from("测试里出现了固定时间基准字面量（应当用 mc_testkit::fixtures）：\n");
        for problem in &test_problems {
            message.push_str(&format!("  - {problem}\n"));
        }
        return Err(message);
    }

    if !problems.is_empty() {
        let mut message = String::from("生产代码里出现了测试夹具：\n");
        for problem in &problems {
            message.push_str(&format!("  - {problem}\n"));
        }
        message.push_str("\n把这些值放到测试里，或引用 mc_testkit::fixtures::*。");
        return Err(message);
    }

    println!("测试夹具检查通过（生产代码里没有夹具字面量）");
    Ok(())
}

/// `crates/mc-testkit` 就是夹具的家（dev-dependency，不进发布产物）；`apps/xtask`
/// 是开发工具、不进发布产物，而且**它自己就是这条守卫的实现**，模式表里必然包含
/// 那些夹具字面量。这两个目录必须显式排除，否则守卫会把自己的模式表当成违规。
fn is_testkit(path: &Path) -> bool {
    path.components().any(|part| {
        let name = part.as_os_str();
        name == "mc-testkit" || name == "xtask"
    })
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

fn lines(path: &Path) -> Result<Vec<(usize, String)>, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("无法读取 {}：{error}", path.display()))?;
    Ok(text
        .lines()
        .enumerate()
        .map(|(index, line)| (index + 1, line.to_string()))
        .collect())
}

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with("//")
}
