//! 一键诊断包：把脱敏后的日志尾巴与运行信息写成目录或 zip。
//!
//! 包是给人分享的（贴 issue / 发邮件），因此内容必须当面可证明「不含密钥与路径」。
//! 组装逻辑放在 mc-common，外壳与 CLI 共用同一套脱敏规则。

use std::io::{Read, Seek, Write};
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::observability::redact_text;

/// 单个日志文件写入诊断包时的默认尾巴长度。
pub const DEFAULT_LOG_TAIL_BYTES: u64 = 256 * 1024;

/// 诊断包里的运行信息（不含 token / API Key）。
#[derive(Debug, Clone, Serialize)]
pub struct PackMeta {
    pub version: String,
    pub port: u16,
    pub generated_at_ms: i64,
    pub app_name: String,
}

/// 组装诊断包的输入。日志字段为**原始**文本，写出前会统一脱敏。
#[derive(Debug, Clone)]
pub struct PackSources {
    pub meta: PackMeta,
    pub diagnostics_json: Option<Value>,
    pub daemon_log: Option<String>,
    pub renderer_log: Option<String>,
    /// 已知密钥（runtime token 等），写出前整段替换为 `<secret>`。
    pub known_secrets: Vec<String>,
}

/// 读取文件末尾最多 `max_bytes` 字节，按 UTF-8 有损转成字符串。
pub fn tail_file(path: &Path, max_bytes: u64) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    if len > max_bytes {
        file.seek(std::io::SeekFrom::End(-(max_bytes as i64)))?;
    }
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// 先抠掉调用方已知的密钥（如 runtime token），再走统一 `redact_text`。
///
/// `redact_text` 对短于 32 的 token 不一定命中；诊断包分享前必须把已知密钥清掉。
pub fn scrub_log_text(raw: &str, known_secrets: &[&str]) -> String {
    let mut text = raw.to_string();
    for secret in known_secrets {
        if secret.len() >= 8 {
            text = text.replace(secret, "<secret>");
        }
    }
    redact_text(&text)
}

fn readme_text() -> &'static str {
    "MineContext diagnostic pack\n\
     \n\
     Included: info.json (version/port), diagnostics.json (shareable status),\n\
     redacted tails of daemon.log and renderer.log when present.\n\
     \n\
     Excluded: API keys, tokens, absolute paths, window/activity titles,\n\
     OCR/accessibility text, and screenshot bytes.\n"
}

fn entries(sources: &PackSources) -> Vec<(String, String)> {
    let secrets: Vec<&str> = sources.known_secrets.iter().map(String::as_str).collect();
    let mut out = Vec::new();
    out.push(("README.txt".to_string(), readme_text().to_string()));
    out.push((
        "info.json".to_string(),
        serde_json::to_string_pretty(&sources.meta).unwrap_or_else(|_| "{}".to_string()),
    ));
    if let Some(diagnostics) = &sources.diagnostics_json {
        let raw = serde_json::to_string_pretty(diagnostics).unwrap_or_else(|_| "{}".to_string());
        out.push((
            "diagnostics.json".to_string(),
            scrub_log_text(&raw, &secrets),
        ));
    }
    if let Some(daemon) = &sources.daemon_log {
        out.push(("daemon.log".to_string(), scrub_log_text(daemon, &secrets)));
    }
    if let Some(renderer) = &sources.renderer_log {
        out.push((
            "renderer.log".to_string(),
            scrub_log_text(renderer, &secrets),
        ));
    }
    out
}

/// 把诊断包写成目录（已存在则覆盖其中的已知文件名）。
pub fn write_pack_dir(dir: &Path, sources: PackSources) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for (name, body) in entries(&sources) {
        std::fs::write(dir.join(name), body)?;
    }
    Ok(())
}

/// 把诊断包写成 zip 文件。
pub fn write_pack_zip(zip_path: &Path, sources: PackSources) -> std::io::Result<()> {
    if let Some(parent) = zip_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::File::create(zip_path)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, body) in entries(&sources) {
        zip.start_file(name, options)?;
        zip.write_all(body.as_bytes())?;
    }
    zip.finish()?;
    Ok(())
}
