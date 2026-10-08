//! `check-comment-style`：交付物里的注释要写「为什么」，不写开发过程。
//!
//! 三件事：注释里不许出现开发过程引用与叙事（阶段编号、内部文档引用、规划文档名、
//! 新旧对比、切片号，以及往事与版本沿革类的叙述）；模块注释与函数注释各有长度
//! 棘轮，超限要列进对应白名单（函数那条还必须写明理由）；白名单与现状必须
//! **双向**对齐 —— 只增不减的棘轮等于没有棘轮。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;

const MODULE_HEADER_LIMIT: usize = 10;
const FUNCTION_DOC_LIMIT: usize = 8;

/// 模式分两组：交付引用按**文件**记，叙事类按**行**报。
const PATTERNS: &[(&str, &str)] = &[
    (r"踩到|踩过|踩了坑|踩坑", "开发过程叙事：踩坑"),
    (r"第一版|第二版|上一版|之前那版", "开发过程叙事：版本沿革"),
    (r"我们之前|之前我们|当初", "开发过程叙事：往事"),
    // 「现状 + 未来计划」式的叙事同样是开发过程痕迹（与往事、版本沿革同类）
    (r"后续优化|留作后续|将来再|以后再", "开发过程叙事：未来计划"),
    (r"TODO\(临时\)|FIXME\(临时\)", "临时代办标记"),
    (r"Phase\s*\d", "开发阶段编号（Phase x.y）"),
    (r"docs/internal|§", "内部文档引用（docs/… §n）"),
    (r"problem\.md|suggestion\.md|simlar\.md", "规划文档引用"),
    (r"旧实现|新实现|旧系统|旧前端", "旧实现↔新实现对比"),
    (r"切片\s*\d+|tdd-log", "开发切片号"),
    // 内部编号文档（形如 NN-name.md）与开发需求条目号：读者在仓库里找不到它们
    (
        r"[0-9]{2}-[a-z][a-z0-9-]*\.md",
        "内部编号文档引用（NN-name.md）",
    ),
    (r"requirement\s*#\s*\d|硬要求\s*\d", "开发需求条目号"),
    (r"^\s*//+!?\s*\d+\.\d+\s*(——|\+)", "开发阶段条目号"),
];

const DELIVERY_REASONS: &[&str] = &[
    "开发阶段编号（Phase x.y）",
    "内部文档引用（docs/… §n）",
    "规划文档引用",
    "旧实现↔新实现对比",
    "开发切片号",
];

pub fn check(root: &Path) -> Result<(), String> {
    let patterns = compile()?;
    let delivery: Vec<_> = patterns
        .iter()
        .filter(|(_, reason)| DELIVERY_REASONS.contains(reason))
        .cloned()
        .collect();
    let lexical: Vec<_> = patterns
        .iter()
        .filter(|(_, reason)| !DELIVERY_REASONS.contains(reason))
        .cloned()
        .collect();

    let roots = source_roots(root)?;
    let delivery_allow = simple_allowlist(&root.join("scripts/delivery-comment-allow.txt"))?;
    let module_allow = simple_allowlist(&root.join("scripts/comment-length-allow.txt"))?;
    let (function_allow, broken_function_allow) =
        reason_allowlist(&root.join("scripts/function-doc-allow.txt"))?;

    let mut problems = Vec::new();

    // ---- 交付物注释：开发过程引用（按文件）----
    let mut offenders: BTreeSet<String> = BTreeSet::new();
    for dir in &roots {
        for path in source_files(dir, true, &roots)? {
            let text = read(&path)?;
            for line in text.lines() {
                if !is_comment(&path, line) {
                    continue;
                }
                if delivery.iter().any(|(pattern, _)| pattern.is_match(line)) {
                    offenders.insert(key(root, &path));
                    break;
                }
            }
        }
    }
    for path in offenders.difference(&delivery_allow) {
        problems.push(format!(
            "{path}: 注释里写了开发过程（阶段编号、内部条目号、切片号、旧实现↔新实现这类叙事）\n    交付物里的注释只写「为什么」与契约；确实要留的，列进 scripts/delivery-comment-allow.txt"
        ));
    }
    for path in delivery_allow.difference(&offenders) {
        problems.push(format!(
            "{path}: 已经没有开发过程引用了，请从 scripts/delivery-comment-allow.txt 删除这条陈旧条目"
        ));
    }

    // ---- 交付物注释：开发过程叙事（按行）----
    for dir in &roots {
        for path in rust_files(dir)? {
            for (number, line) in read(&path)?.lines().enumerate() {
                if !is_comment(&path, line) {
                    continue;
                }
                if let Some((_, reason)) = lexical.iter().find(|(p, _)| p.is_match(line)) {
                    problems.push(format!(
                        "{}:{number}: {reason}\n    {}",
                        key(root, &path),
                        line.trim_start()
                    ));
                }
            }
        }
    }

    // ---- 模块注释棘轮 ----
    let mut too_long = 0usize;
    let mut listed_files: BTreeSet<String> = BTreeSet::new();
    for dir in &roots {
        for path in rust_files(dir)? {
            let name = key(root, &path);
            let length = module_header_lines(&path)?;
            if length > MODULE_HEADER_LIMIT {
                too_long += 1;
                if !module_allow.contains(&name) {
                    problems.push(format!(
                        "{name}: 模块注释 {length} 行（上限 {MODULE_HEADER_LIMIT} 行）\n    要么删到只留「调用方看不出来的约束」，要么明确列进 scripts/comment-length-allow.txt 再逐 crate 收敛"
                    ));
                }
            }
            if module_allow.contains(&name) {
                listed_files.insert(name.clone());
                if length <= MODULE_HEADER_LIMIT {
                    problems.push(format!(
                        "{name}: 模块注释已经收敛到 {length} 行，请从 scripts/comment-length-allow.txt 里删除这条陈旧条目"
                    ));
                }
            }
        }
    }
    for stale in module_allow.difference(&listed_files) {
        problems.push(format!("{stale}: 白名单条目指向的文件不存在，请删除"));
    }

    // ---- 函数注释棘轮（白名单必须写理由）----
    problems.extend(broken_function_allow);
    let mut long_function_docs = 0usize;
    let mut seen_function_docs: BTreeSet<String> = BTreeSet::new();
    for dir in &roots {
        for path in rust_files(dir)? {
            for (name, length) in function_doc_blocks(&path)? {
                if length <= FUNCTION_DOC_LIMIT {
                    continue;
                }
                long_function_docs += 1;
                let entry = format!("{}::{name}", key(root, &path));
                seen_function_docs.insert(entry.clone());
                if !function_allow.contains_key(&entry) {
                    problems.push(format!(
                        "{entry}: 函数注释 {length} 行（上限 {FUNCTION_DOC_LIMIT} 行）\n    公开 API 的契约说明可以长，但要在 scripts/function-doc-allow.txt 里写明「为什么必须这么长」"
                    ));
                }
            }
        }
    }
    for entry in function_allow.keys() {
        if !seen_function_docs.contains(entry) {
            problems.push(format!(
                "{entry}: 这条函数注释已经不长于 {FUNCTION_DOC_LIMIT} 行了，请从 scripts/function-doc-allow.txt 删除这条陈旧条目"
            ));
        }
    }

    if problems.is_empty() {
        println!(
            "注释风格检查通过（无开发过程叙事；模块注释超限文件 {too_long} 个，白名单 {} 条；函数注释超限 {long_function_docs} 处，登记 {} 条 —— 棘轮只减不增）",
            listed_files.len(),
            seen_function_docs.len()
        );
        return Ok(());
    }

    let mut message = String::from("注释风格检查未通过：\n");
    for problem in &problems {
        message.push_str(&format!("  - {problem}\n"));
    }
    message.push_str("\n规则见 docs/comment-style.md。");
    Err(message)
}

fn compile() -> Result<Vec<(Regex, &'static str)>, String> {
    PATTERNS
        .iter()
        .map(|(pattern, reason)| {
            Regex::new(pattern)
                .map(|regex| (regex, *reason))
                .map_err(|error| format!("内置正则非法：{error}"))
        })
        .collect()
}

/// 注释前缀按文件类型判定：Rust/TS 用 `//`，SQL 用 `--`，shell 用 `#`。
///
/// 不认 `#` 之外的方言差异会漏扫（SQL/shell 的注释里同样不该出现过程叙事）；
/// 而把 `#` 一律当注释又会把 Rust 的属性当成注释，所以这里按扩展名分派。
fn is_comment(path: &Path, line: &str) -> bool {
    let stripped = line.trim_start();
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("sql") => stripped.starts_with("--"),
        Some("sh") => stripped.starts_with('#'),
        _ => stripped.starts_with("//") || stripped.starts_with("/*") || stripped.starts_with('*'),
    }
}

fn key(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn read(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|error| format!("无法读取 {}：{error}", path.display()))
}

/// `crates/*/src` + `apps/*/src` + `frontend/src` + `scripts`。
///
/// **不含 `migrations/`**：迁移文件一旦发布就连注释都不能改 —— 数据库会记录每个已应用
/// 迁移的 checksum，改动（哪怕只动注释）会让已有库拒绝启动（`storage_migration_failed`）。
/// 要改 schema 就新增一个迁移文件。
fn source_roots(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut roots = Vec::new();
    for dir in ["crates", "apps"] {
        let base = root.join(dir);
        let entries =
            fs::read_dir(&base).map_err(|error| format!("无法读取 {}：{error}", base.display()))?;
        let mut found: Vec<PathBuf> = Vec::new();
        for entry in entries.filter_map(Result::ok) {
            let crate_root = entry.path();
            for sub in ["src", "tests"] {
                let path = crate_root.join(sub);
                if path.is_dir() {
                    found.push(path);
                }
            }
        }
        found.sort();
        roots.extend(found);
    }
    let frontend = root.join("frontend/src");
    if frontend.is_dir() {
        roots.push(frontend);
    }
    // 门禁脚本自己也按同一标准写注释：漏掉它们等于给自己开后门。
    let scripts = root.join("scripts");
    if scripts.is_dir() {
        roots.push(scripts);
    }
    Ok(roots)
}

fn rust_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    source_files(dir, false, &[])
}

/// 扫 Rust/TS/TSX，另加迁移 SQL 与 shell 脚本。
fn source_files(dir: &Path, _delivery: bool, _roots: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = fs::read_dir(&current)
            .map_err(|error| format!("无法读取 {}：{error}", current.display()))?;
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("rs") | Some("ts") | Some("tsx") | Some("sql") | Some("sh")
            ) {
                found.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}

fn module_header_lines(path: &Path) -> Result<usize, String> {
    let text = read(path)?;
    let lines: Vec<&str> = text.lines().collect();
    let mut count = 0usize;
    while count < lines.len() {
        let stripped = lines[count].trim();
        let blank_before_header = stripped.is_empty()
            && count + 1 < lines.len()
            && lines[count + 1].trim().starts_with("//!");
        if stripped.starts_with("//!") || blank_before_header {
            count += 1;
        } else {
            break;
        }
    }
    Ok(count)
}

fn function_doc_blocks(path: &Path) -> Result<Vec<(String, usize)>, String> {
    let text = read(path)?;
    let lines: Vec<&str> = text.lines().collect();
    let mut blocks = Vec::new();
    let mut index = 0usize;
    while index < lines.len() {
        if lines[index].trim_start().starts_with("///") {
            let start = index;
            while index < lines.len() && lines[index].trim_start().starts_with("///") {
                index += 1;
            }
            if let Some(name) = attached_function(&lines, index) {
                blocks.push((name, index - start));
            }
        } else {
            index += 1;
        }
    }
    Ok(blocks)
}

fn attached_function(lines: &[&str], index: usize) -> Option<String> {
    let pattern = Regex::new(r"\bfn\s+(\w+)").expect("内置正则应当合法");
    for raw in lines.iter().skip(index).take(5) {
        let text = raw.trim();
        if text.is_empty() {
            return None;
        }
        if let Some(captures) = pattern.captures(text) {
            return captures.get(1).map(|name| name.as_str().to_string());
        }
        if text.starts_with("#[")
            || text.starts_with("pub")
            || text.starts_with("async")
            || text.starts_with("const")
            || text.starts_with("unsafe")
            || text.starts_with("extern")
            || text.starts_with("default")
        {
            continue;
        }
        return None;
    }
    None
}

fn simple_allowlist(path: &Path) -> Result<BTreeSet<String>, String> {
    if !path.is_file() {
        return Ok(BTreeSet::new());
    }
    let text = read(path)?;
    Ok(text
        .lines()
        .map(|line| {
            line.split('#')
                .next()
                .unwrap_or_default()
                .trim()
                .to_string()
        })
        .filter(|line| !line.is_empty())
        .collect())
}

fn reason_allowlist(path: &Path) -> Result<(BTreeMap<String, String>, Vec<String>), String> {
    let mut entries = BTreeMap::new();
    let mut broken = Vec::new();
    if !path.is_file() {
        return Ok((entries, broken));
    }
    let text = read(path)?;
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, reason) = match line.split_once('#') {
            Some((key, reason)) => (key.trim(), reason.trim()),
            None => (line, ""),
        };
        if key.is_empty() {
            continue;
        }
        if reason.is_empty() {
            broken.push(format!(
                "{}:{}: 「{key}」没有写明理由",
                path.display(),
                index + 1
            ));
            continue;
        }
        entries.insert(key.to_string(), reason.to_string());
    }
    Ok((entries, broken))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_and_shell_comments_are_recognised() {
        // 迁移 SQL 与门禁脚本都在扫描面里：漏认注释语法会让它们整片不被检查
        assert!(is_comment(Path::new("0001_init.sql"), "-- 兼容层"));
        assert!(is_comment(Path::new("verify-all.sh"), "  # 每步计时"));
        assert!(!is_comment(
            Path::new("0001_init.sql"),
            "CREATE TABLE t (id TEXT);"
        ));
        assert!(!is_comment(Path::new("verify-all.sh"), "set -euo pipefail"));
    }

    #[test]
    fn rust_attributes_are_not_comments() {
        // `#` 只有在 shell 里才是注释；Rust 的属性行不是
        assert!(!is_comment(Path::new("lib.rs"), "#[derive(Debug)]"));
        assert!(!is_comment(Path::new("lib.rs"), "# [allow(dead_code)]"));
        assert!(is_comment(Path::new("lib.rs"), "// 说明"));
    }
}
