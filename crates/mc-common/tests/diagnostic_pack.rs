//! 一键诊断包：脱敏日志尾巴 + 运行信息，可写成目录或 zip。
//!
//! 用户会把这个包发给别人，因此密钥、绝对路径、token 都不能出现；
//! 同时要自带 version / port / 权限侧诊断 JSON，避免对方还得再问一轮。

use std::io::Read;
use std::path::Path;

use mc_common::diagnostic_pack::{self, PackMeta, PackSources, DEFAULT_LOG_TAIL_BYTES};

const API_KEY: &str = "sk-live-DIAGPACK0123456789abcdef";
const TOKEN: &str = "export-token-0123456789abcdef";
const HOME_PATH: &str = "/Users/someone/Library/Application Support/MineContext/logs/daemon.log";

fn seed_log(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

fn sample_sources(daemon: &str, renderer: &str) -> PackSources {
    PackSources {
        meta: PackMeta {
            version: "1.0.7".to_string(),
            port: 18432,
            generated_at_ms: 1_700_000_000_000,
            app_name: "MineContext".to_string(),
        },
        diagnostics_json: Some(serde_json::json!({
            "schema_version": "mc-diagnostics/1",
            "version": "1.0.7",
            "components": {
                "capture": { "permission": "granted", "status": "running" }
            }
        })),
        daemon_log: Some(daemon.to_string()),
        renderer_log: Some(renderer.to_string()),
        known_secrets: vec![TOKEN.to_string()],
    }
}

#[test]
fn known_short_token_is_scrubbed_even_without_sk_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("pack");
    let mut sources = sample_sources(&format!("x-mc-token: {TOKEN}"), "ok");
    sources.daemon_log = Some(format!("x-mc-token: {TOKEN}"));
    diagnostic_pack::write_pack_dir(&out, sources).unwrap();
    let daemon_out = std::fs::read_to_string(out.join("daemon.log")).unwrap();
    assert!(!daemon_out.contains(TOKEN), "{daemon_out}");
    assert!(daemon_out.contains("<secret>"), "{daemon_out}");
}

#[test]
fn tail_file_keeps_only_the_end() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daemon.log");
    // 开头用独特前缀，尾巴窗口放不下它，避免按字节截断时碰到半个单词误伤断言。
    let body = format!(
        "{}{}{}",
        "UNIQUE-HEAD-ONLY-",
        "x".repeat(200),
        "TAIL-MARKER"
    );
    seed_log(&path, &body);

    let tailed = diagnostic_pack::tail_file(&path, 32).unwrap();
    assert!(tailed.contains("TAIL-MARKER"), "{tailed}");
    assert!(tailed.len() <= 32, "尾巴不应超过上限：{}", tailed.len());
    assert!(
        !tailed.contains("UNIQUE-HEAD-ONLY"),
        "应丢掉文件开头：{tailed}"
    );
}

#[test]
fn write_pack_redacts_secrets_and_paths() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("pack");
    let daemon = format!("auth failed key={API_KEY} path={HOME_PATH} token={TOKEN}");
    let renderer = format!("axios 401 Authorization: Bearer {TOKEN}");

    diagnostic_pack::write_pack_dir(&out, sample_sources(&daemon, &renderer)).unwrap();

    let daemon_out = std::fs::read_to_string(out.join("daemon.log")).unwrap();
    let renderer_out = std::fs::read_to_string(out.join("renderer.log")).unwrap();
    let info = std::fs::read_to_string(out.join("info.json")).unwrap();
    let diagnostics = std::fs::read_to_string(out.join("diagnostics.json")).unwrap();

    for (label, text) in [
        ("daemon.log", daemon_out.as_str()),
        ("renderer.log", renderer_out.as_str()),
        ("info.json", info.as_str()),
        ("diagnostics.json", diagnostics.as_str()),
    ] {
        assert!(!text.contains(API_KEY), "{label} leaked API key");
        assert!(!text.contains(TOKEN), "{label} leaked token");
        assert!(!text.contains("/Users/someone"), "{label} leaked home path");
    }
    assert!(
        daemon_out.contains("<secret>") || daemon_out.contains("<path>"),
        "{daemon_out}"
    );
    assert!(info.contains("\"port\": 18432"), "{info}");
    assert!(info.contains("1.0.7"), "{info}");
    assert!(diagnostics.contains("mc-diagnostics/1"), "{diagnostics}");
    assert!(
        out.join("README.txt").is_file(),
        "pack must explain what is included"
    );
}

#[test]
fn write_pack_zip_is_shareable_and_sanitized() {
    let dir = tempfile::tempdir().unwrap();
    let zip_path = dir.path().join("MineContext-diagnostics.zip");
    let daemon = format!("secret={API_KEY}");
    let renderer = "ok";

    diagnostic_pack::write_pack_zip(&zip_path, sample_sources(&daemon, &renderer)).unwrap();
    assert!(zip_path.is_file());

    let file = std::fs::File::open(&zip_path).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let mut names = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).unwrap();
        names.push(entry.name().to_string());
        let mut body = String::new();
        entry.read_to_string(&mut body).unwrap();
        assert!(!body.contains(API_KEY), "{} leaked key", entry.name());
    }
    for expected in [
        "daemon.log",
        "renderer.log",
        "info.json",
        "diagnostics.json",
        "README.txt",
    ] {
        assert!(
            names.iter().any(|n| n.ends_with(expected)),
            "zip missing {expected}: {names:?}"
        );
    }
}

#[test]
fn missing_logs_still_produce_info_and_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("pack");
    let mut sources = sample_sources("", "");
    sources.daemon_log = None;
    sources.renderer_log = None;

    diagnostic_pack::write_pack_dir(&out, sources).unwrap();
    assert!(out.join("info.json").is_file());
    assert!(out.join("diagnostics.json").is_file());
    assert!(!out.join("daemon.log").is_file());
    assert!(!out.join("renderer.log").is_file());
}

#[test]
fn default_tail_budget_is_bounded() {
    assert!(DEFAULT_LOG_TAIL_BYTES <= 512 * 1024);
    assert!(DEFAULT_LOG_TAIL_BYTES >= 64 * 1024);
}
