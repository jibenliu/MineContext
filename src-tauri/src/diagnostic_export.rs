//! 一键诊断导出：在 Downloads 写出脱敏 zip（日志尾巴 + 运行/诊断信息）。
//!
//! 组装与脱敏复用 `mc_common::diagnostic_pack`，避免外壳另搞一套规则。

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mc_common::diagnostic_pack::{self, PackMeta, PackSources, DEFAULT_LOG_TAIL_BYTES};
use serde::Serialize;
use serde_json::Value;
use tauri::Manager;

#[derive(Debug, Serialize)]
pub struct ExportResult {
    pub path: String,
    pub folder: String,
}

fn read_runtime_value(data_dir: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(data_dir.join("runtime.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// 本机 loopback GET：拉可分享的诊断 JSON（信封 `data`）。失败时返回 None，包仍可导出日志。
fn fetch_diagnostics_data(port: u16, token: &str) -> Option<Value> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    stream.set_write_timeout(Some(Duration::from_secs(3))).ok()?;
    let request = format!(
        "GET /api/v1/diagnostics/export HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         x-mc-token: {token}\r\n\
         Connection: close\r\n\
         \r\n"
    );
    stream.write_all(request.as_bytes()).ok()?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let body = text.split("\r\n\r\n").nth(1)?;
    let envelope: Value = serde_json::from_str(body.trim()).ok()?;
    envelope.get("data").cloned()
}

fn stamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn downloads_dir(app: &tauri::AppHandle) -> PathBuf {
    app.path().download_dir().unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join("Downloads")
    })
}

fn renderer_log_path(app: &tauri::AppHandle) -> Option<PathBuf> {
    app.path()
        .app_log_dir()
        .ok()
        .map(|dir| dir.join("renderer.log"))
}

/// 写出脱敏诊断包到 Downloads，返回 zip 与同名目录路径。
pub fn export_pack(app: &tauri::AppHandle, data_dir: &Path) -> Result<ExportResult, String> {
    let runtime = read_runtime_value(data_dir);
    let port = runtime
        .as_ref()
        .and_then(|v| v.get("port"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u16;
    let token = runtime
        .as_ref()
        .and_then(|v| v.get("token"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let version = runtime
        .as_ref()
        .and_then(|v| v.get("version"))
        .and_then(|v| v.as_str())
        .unwrap_or(env!("CARGO_PKG_VERSION"))
        .to_string();

    let known_secrets = if token.len() >= 8 {
        vec![token.clone()]
    } else {
        Vec::new()
    };

    let diagnostics = if port > 0 && !token.is_empty() {
        fetch_diagnostics_data(port, &token)
    } else {
        None
    };

    let daemon_raw = diagnostic_pack::tail_file(
        &data_dir.join("logs").join("daemon.log"),
        DEFAULT_LOG_TAIL_BYTES,
    )
    .ok();
    let renderer_raw = renderer_log_path(app)
        .and_then(|path| diagnostic_pack::tail_file(&path, DEFAULT_LOG_TAIL_BYTES).ok());

    let sources = PackSources {
        meta: PackMeta {
            version,
            port,
            generated_at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
            app_name: "MineContext".to_string(),
        },
        diagnostics_json: diagnostics,
        daemon_log: daemon_raw,
        renderer_log: renderer_raw,
        known_secrets,
    };

    let base = downloads_dir(app).join(format!("MineContext-diagnostics-{}", stamp_secs()));
    let folder = base.clone();
    let zip_path = PathBuf::from(format!("{}.zip", base.display()));

    diagnostic_pack::write_pack_dir(&folder, sources.clone()).map_err(|e| e.to_string())?;
    diagnostic_pack::write_pack_zip(&zip_path, sources).map_err(|e| e.to_string())?;

    Ok(ExportResult {
        path: zip_path.display().to_string(),
        folder: folder.display().to_string(),
    })
}
