//! 从**调用点**反查 IPC 渠道：业务代码用到的渠道，适配层渠道表是否覆盖。
//!
//! 契约夹具是从适配层的渠道表生成的，所以它看不见「业务代码调了一个表里没有的
//! 渠道」这种情况 —— 那会表现为运行时报「渠道 X 尚未映射」，而门禁全绿。
//! 这里补上另一半：扫渲染层源码里的渠道调用点，把表里没有的渠道列出来。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use regex::Regex;

/// 一个渠道调用点：渠道名 + 出处的**文件**（不带行号，行号会让夹具频繁抖动）。
pub fn unresolved(
    sources: &[(String, String)],
    enums: &BTreeMap<String, String>,
    known: &BTreeSet<String>,
) -> Vec<String> {
    let literal = Regex::new(
        r"(?:ipcRenderer|backend)\s*\.\s*(?:invoke|subscribe|on|once|send|removeListener|removeAllListeners|off)\s*\(\s*'([^']+)'",
    )
    .expect("内置正则应当合法");
    let member = Regex::new(
        r"(?:ipcRenderer|backend)\s*\.\s*(?:invoke|subscribe|on|once|send|removeListener|removeAllListeners|off)\s*\(\s*([A-Za-z_][A-Za-z0-9_]*)\.([A-Za-z_][A-Za-z0-9_]*)",
    )
    .expect("内置正则应当合法");

    let mut findings: BTreeSet<String> = BTreeSet::new();
    for (path, text) in sources {
        for capture in literal.captures_iter(text) {
            record(&mut findings, known, &capture[1], path);
        }
        for capture in member.captures_iter(text) {
            let (kind, name) = (capture[1].to_string(), capture[2].to_string());
            match enums.get(&format!("{kind}.{name}")) {
                Some(value) => record(&mut findings, known, value, path),
                // 枚举成员解析不出来（改名/拼错）本身就是要报的问题。
                None => {
                    findings.insert(format!("{kind}.{name}（枚举里找不到，见 {path}）"));
                }
            }
        }
    }
    findings.into_iter().collect()
}

fn record(findings: &mut BTreeSet<String>, known: &BTreeSet<String>, channel: &str, path: &str) {
    if !known.contains(channel) {
        findings.insert(format!("{channel} ({path})"));
    }
}

/// 渲染层业务源码（跳过 `__tests__`：测试里的渠道是刻意的替身）。
pub fn renderer_sources(root: &Path) -> Result<Vec<(String, String)>, String> {
    let base = root.join("frontend/src/renderer/src");
    let mut found = Vec::new();
    let mut stack = vec![base.clone()];
    while let Some(current) = stack.pop() {
        let entries = fs::read_dir(&current)
            .map_err(|error| format!("读不到 {}：{error}", current.display()))?;
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().and_then(|name| name.to_str()) == Some("__tests__") {
                    continue;
                }
                stack.push(path);
                continue;
            }
            let is_source = matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("ts") | Some("tsx")
            );
            if !is_source {
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string();
            let text = fs::read_to_string(&path)
                .map_err(|error| format!("读不到 {}：{error}", path.display()))?;
            found.push((relative, without_comments(&text)));
        }
    }
    found.sort();
    Ok(found)
}

/// 去掉整行注释：注释里出现的 `ipcRenderer.on('...')` 只是说明文字，不是调用点
/// （真实渠道名不会跨行，因此整行注释足以覆盖）。
fn without_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_block = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if in_block {
            if trimmed.contains("*/") {
                in_block = false;
            }
            continue;
        }
        if trimmed.starts_with("/*") {
            in_block = !trimmed.contains("*/");
            continue;
        }
        if trimmed.starts_with("//") || trimmed.starts_with('*') {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// 枚举成员 → 渠道字符串：`IpcChannel.UpdateDownloaded` → `update-downloaded`。
///
/// 枚举名从 `export enum X {` 那一行读，而不是从文件名猜 —— 文件名
/// （`ipc-server-push-channel.ts`）与枚举名（`IpcServerPushChannel`）并不一致。
pub fn enum_members(root: &Path) -> Result<BTreeMap<String, String>, String> {
    let files = [
        "frontend/packages/shared/ipc-channel.ts",
        "frontend/packages/shared/ipc-server-push-channel.ts",
    ];
    let declaration =
        Regex::new(r"enum\s+([A-Za-z_][A-Za-z0-9_]*)\s*\{").expect("内置正则应当合法");
    let member =
        Regex::new(r"^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=\s*'([^']+)'").expect("内置正则应当合法");

    let mut out = BTreeMap::new();
    for file in files {
        let path = root.join(file);
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("读不到 {}：{error}", path.display()))?;
        let mut kind = String::new();
        for line in text.lines() {
            if let Some(capture) = declaration.captures(line) {
                kind = capture[1].to_string();
                continue;
            }
            if kind.is_empty() {
                continue;
            }
            if let Some(capture) = member.captures(line) {
                out.insert(format!("{kind}.{}", &capture[1]), capture[2].to_string());
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enums() -> BTreeMap<String, String> {
        BTreeMap::from([(
            "IpcChannel.UpdateDownloaded".to_string(),
            "update-downloaded".to_string(),
        )])
    }

    fn known(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn unmapped_literal_is_reported() {
        let sources = vec![(
            "App.tsx".to_string(),
            "await ipcRenderer.invoke('backend:get-status')".to_string(),
        )];
        let found = unresolved(&sources, &enums(), &known(&["database:get-all-vaults"]));
        assert_eq!(found, vec!["backend:get-status (App.tsx)".to_string()]);
    }

    #[test]
    fn mapped_literal_is_not_reported() {
        let sources = vec![(
            "adapters/db-api.ts".to_string(),
            "backend.invoke('database:get-all-vaults', [])".to_string(),
        )];
        assert!(unresolved(&sources, &enums(), &known(&["database:get-all-vaults"])).is_empty());
    }

    #[test]
    fn enum_member_is_resolved_to_its_value() {
        let sources = vec![(
            "Button.tsx".to_string(),
            "ipcRenderer.on(IpcChannel.UpdateDownloaded, handler)".to_string(),
        )];
        assert!(unresolved(&sources, &enums(), &known(&["update-downloaded"])).is_empty());
        let found = unresolved(&sources, &enums(), &known(&[]));
        assert_eq!(found, vec!["update-downloaded (Button.tsx)".to_string()]);
    }

    #[test]
    fn unknown_enum_member_is_reported() {
        let sources = vec![(
            "Button.tsx".to_string(),
            "ipcRenderer.on(IpcChannel.UpdateGone, handler)".to_string(),
        )];
        let found = unresolved(&sources, &enums(), &known(&[]));
        assert_eq!(
            found,
            vec!["IpcChannel.UpdateGone（枚举里找不到，见 Button.tsx）".to_string()]
        );
    }

    #[test]
    fn unrelated_on_calls_are_ignored() {
        let sources = vec![(
            "stream.ts".to_string(),
            "source.addEventListener('message', handler)\nwindow.on('resize', handler)".to_string(),
        )];
        assert!(unresolved(&sources, &enums(), &known(&[])).is_empty());
    }

    #[test]
    fn subscription_calls_are_covered() {
        let sources = vec![(
            "server-push-api.ts".to_string(),
            "backend.subscribe('push:latest-activity', callback)".to_string(),
        )];
        assert!(unresolved(&sources, &enums(), &known(&["push:latest-activity"])).is_empty());
        let found = unresolved(&sources, &enums(), &known(&[]));
        assert_eq!(
            found,
            vec!["push:latest-activity (server-push-api.ts)".to_string()]
        );
    }

    #[test]
    fn comment_lines_are_not_call_sites() {
        let source = "// Router.tsx 直接调用 ipcRenderer.on('push:tray-...')\nconst x = 1\n/* ipcRenderer.on('blocked', handler) */\n";
        let stripped = without_comments(source);
        assert!(!stripped.contains("push:tray-..."));
        assert!(!stripped.contains("blocked"));
        assert!(stripped.contains("const x = 1"));
    }

    #[test]
    fn block_comment_spanning_lines_is_skipped() {
        let source =
            "/*\n * ipcRenderer.invoke('inside-block')\n */\nbackend.invoke('real:channel')\n";
        let stripped = without_comments(source);
        assert!(!stripped.contains("inside-block"));
        assert!(stripped.contains("real:channel"));
    }
}
