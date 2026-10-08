//! `check-shell-vars`：脚本里 `$变量` 紧跟**全角标点**会被 macOS 自带的 bash 3.2
//! 当成变量名的一部分，在 `set -u` 下直接 unbound variable 退出。
//!
//! 只拦这一种形态，不检查别的 shell 风格问题；实现放在这里是为了让仓库不依赖 python3。

use std::fs;
use std::path::Path;

use regex::Regex;

/// 变量名后紧跟全角标点（不含 `}` / `"` / 空格等合法边界）。
const PATTERN: &str = r"\$[A-Za-z_][A-Za-z0-9_]*[（），。：；、？！]";

/// 扫 `scripts/*.sh`；有问题返回 Err（调用方据此给非零退出码）。
pub fn check(root: &Path) -> Result<(), String> {
    let pattern = Regex::new(PATTERN).expect("内置正则应当合法");
    let dir = root.join("scripts");
    let mut problems = Vec::new();

    let mut files: Vec<_> = fs::read_dir(&dir)
        .map_err(|error| format!("无法读取 {}：{error}", dir.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "sh"))
        .collect();
    files.sort();

    for path in files {
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("无法读取 {}：{error}", path.display()))?;
        for (index, line) in text.lines().enumerate() {
            // 跳过注释：说明性文字里出现这种写法是正常的
            if line.trim_start().starts_with('#') {
                continue;
            }
            if let Some(found) = pattern.find(line) {
                problems.push(format!(
                    "{}:{}: {}  →  应写成 ${{变量}}",
                    path.display(),
                    index + 1,
                    found.as_str()
                ));
            }
        }
    }

    if problems.is_empty() {
        println!("shell 变量展开检查通过（没有变量紧跟全角标点的写法）");
        return Ok(());
    }

    let mut message =
        String::from("shell 变量展开写成 `$变量紧跟全角标点`（bash 3.2 会把标点当成变量名）：\n");
    for problem in &problems {
        message.push_str(&format!("  - {problem}\n"));
    }
    message.push_str("\n改成 `${变量}` 加标点。");
    Err(message)
}
