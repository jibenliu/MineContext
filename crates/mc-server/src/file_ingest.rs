//! 文件上传：把本地文档 / 图片 / 代码 / 音视频 / 会议记录抽成笔记树文档，
//! 复用既有 vault → 检索 / 向量索引通路。
//!
//! 原始字节仍落在 `<data_dir>/uploads`，与文件页清单共用，不另开索引管线。
//! 音频 / 视频不做转写（与图片不做 OCR 同形）：以文件名、格式与本地路径进入检索。

use std::io::Read;
use std::path::{Path, PathBuf};

use mc_common::error::{AppError, ErrorCode};
use mc_common::time::Timestamp;
use mc_storage::vaults::{VaultUpsert, DOCUMENT_TYPE_VAULTS};
use mc_storage::Database;

use crate::link_ingest;
use crate::routes::files::{MAX_UPLOAD_BYTES, UPLOADS_DIR};

/// 落库正文上限：与链接上传同量级，检索与编辑器都吃得消。
const MAX_TEXT_CHARS: usize = 200_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Unstructured,
    Structured,
    Image,
    Code,
    Audio,
    Video,
    Meeting,
}

impl FileKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unstructured => "unstructured",
            Self::Structured => "structured",
            Self::Image => "image",
            Self::Code => "code",
            Self::Audio => "audio",
            Self::Video => "video",
            Self::Meeting => "meeting",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileIngestResult {
    pub id: i64,
    pub title: String,
    pub name: String,
    pub kind: FileKind,
    pub file_path: String,
}

/// 按扩展名分类；未知类型直接拒绝。
pub fn classify_file(name: &str) -> Result<(FileKind, String), AppError> {
    let ext = Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext.is_empty() {
        return Err(invalid("文件名缺少扩展名，无法判断类型"));
    }
    let kind = match ext.as_str() {
        "txt" | "md" | "markdown" | "faq" | "csv" | "html" | "htm" => FileKind::Unstructured,
        "pdf" | "docx" | "xlsx" | "pptx" => FileKind::Structured,
        "png" | "jpg" | "jpeg" | "gif" | "webp" => FileKind::Image,
        "mp3" | "wav" | "m4a" | "aac" | "flac" | "ogg" => FileKind::Audio,
        "mp4" | "mov" | "webm" | "mkv" | "avi" => FileKind::Video,
        "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "go" | "java" | "c" | "cpp" | "h" | "hpp"
        | "cs" | "rb" | "php" | "swift" | "kt" | "scala" | "sh" | "bash" | "zsh" | "json"
        | "yaml" | "yml" | "toml" | "xml" | "sql" => FileKind::Code,
        "vtt" | "srt" | "ics" => FileKind::Meeting,
        "doc" | "ppt" | "xls" => {
            return Err(invalid(
                "旧版 Office 二进制格式暂不支持，请另存为 docx / pptx / xlsx 后再上传",
            ));
        }
        other => {
            return Err(invalid(&format!(
                "不支持的文件类型 .{other}（支持文档、表格、幻灯片、图片、代码、音视频与会议记录）"
            )));
        }
    };
    Ok((kind, ext))
}

/// 保存上传字节 → 抽取正文 → 写入 vaults。抽取失败时不落库（已写入的 blob 保留）。
pub fn ingest_file(
    db: &Database,
    data_dir: &Path,
    name: &str,
    bytes: &[u8],
    parent_id: Option<i64>,
    at: Timestamp,
) -> Result<FileIngestResult, AppError> {
    let name = sanitize_upload_name(name)?;
    let (kind, ext) = classify_file(&name)?;
    if bytes.len() as u64 > MAX_UPLOAD_BYTES {
        return Err(invalid(&format!(
            "文件过大（{} 字节，上限 {MAX_UPLOAD_BYTES}）",
            bytes.len()
        )));
    }

    let uploads = ensure_uploads_dir(data_dir)?;
    let path = mc_common::fs::resolve_within(&uploads, &name)?;
    std::fs::write(&path, bytes).map_err(|error| {
        AppError::new(
            ErrorCode::StorageUnavailable,
            format!("无法写入上传文件 {}: {error}", path.display()),
        )
    })?;

    let extracted = extract_content(kind, &ext, &name, bytes, &path)?;
    if extracted.text.trim().is_empty() {
        return Err(invalid("文件没有可提取的正文"));
    }

    let title = if extracted.title.trim().is_empty() {
        title_from_name(&name)
    } else {
        truncate_chars(&extracted.title, 200)
    };
    let summary: String = extracted.text.chars().take(240).collect();
    let content = format!("来源：{name}\n\n{}", extracted.text);
    let tags = vec!["file".to_string(), kind.as_str().to_string(), ext];
    let id = db.insert_vault_row(
        &VaultUpsert {
            title: title.clone(),
            summary,
            content,
            tags,
            parent_id,
            is_folder: false,
            document_type: DOCUMENT_TYPE_VAULTS.to_string(),
            sort_order: 0,
        },
        at,
    )?;

    Ok(FileIngestResult {
        id,
        title,
        name,
        kind,
        file_path: path.to_string_lossy().into_owned(),
    })
}

struct Extracted {
    title: String,
    text: String,
}

fn extract_content(
    kind: FileKind,
    ext: &str,
    name: &str,
    bytes: &[u8],
    path: &Path,
) -> Result<Extracted, AppError> {
    match kind {
        FileKind::Unstructured => extract_unstructured(ext, name, bytes),
        FileKind::Structured => extract_structured(ext, bytes),
        FileKind::Image => extract_image(name, bytes, path),
        FileKind::Code => extract_code(name, bytes),
        FileKind::Audio | FileKind::Video => extract_media(kind, name, bytes, path),
        FileKind::Meeting => extract_meeting(ext, name, bytes),
    }
}

fn extract_unstructured(ext: &str, name: &str, bytes: &[u8]) -> Result<Extracted, AppError> {
    let raw = String::from_utf8_lossy(bytes);
    if matches!(ext, "html" | "htm") {
        let page = link_ingest::extract_page(&raw, name);
        return Ok(Extracted {
            title: page.title,
            text: truncate_chars(&page.text, MAX_TEXT_CHARS),
        });
    }
    Ok(Extracted {
        title: title_from_name(name),
        text: truncate_chars(&collapse_ws(&raw), MAX_TEXT_CHARS),
    })
}

fn extract_structured(ext: &str, bytes: &[u8]) -> Result<Extracted, AppError> {
    let text = match ext {
        "pdf" => extract_pdf(bytes)?,
        "docx" => extract_docx(bytes)?,
        "xlsx" => extract_xlsx(bytes)?,
        "pptx" => extract_pptx(bytes)?,
        other => return Err(invalid(&format!("未实现的结构化类型 .{other}"))),
    };
    Ok(Extracted {
        title: String::new(),
        text: truncate_chars(&collapse_ws(&text), MAX_TEXT_CHARS),
    })
}

fn extract_image(name: &str, bytes: &[u8], path: &Path) -> Result<Extracted, AppError> {
    let title = title_from_name(name);
    let dims = image::load_from_memory(bytes)
        .ok()
        .map(|img| format!("{}×{}", img.width(), img.height()));
    let dims_line = match dims {
        Some(value) => format!("尺寸：{value}。"),
        None => String::new(),
    };
    // 不做 OCR：图片以文件名与 markdown 引用进入检索；视觉理解仍走既有截图管线。
    let text = format!(
        "![{title}](file://{})\n\n图片文件。{dims_line}检索以文件名为准，不进行 OCR。",
        path.to_string_lossy()
    );
    Ok(Extracted { title, text })
}

fn extract_code(name: &str, bytes: &[u8]) -> Result<Extracted, AppError> {
    let raw = String::from_utf8_lossy(bytes);
    let text = truncate_chars(raw.trim(), MAX_TEXT_CHARS);
    if text.is_empty() {
        return Err(invalid("代码文件没有可提取的正文"));
    }
    Ok(Extracted {
        title: title_from_name(name),
        text,
    })
}

fn extract_media(
    kind: FileKind,
    name: &str,
    bytes: &[u8],
    path: &Path,
) -> Result<Extracted, AppError> {
    let title = title_from_name(name);
    let label = match kind {
        FileKind::Audio => "音频",
        FileKind::Video => "视频",
        _ => "媒体",
    };
    // 不做语音转写：与图片不做 OCR 同形，以元数据与本地路径进入检索。
    let text = format!(
        "[{title}](file://{})\n\n{label}文件，大小 {} 字节。检索以文件名为准，不进行语音转写。",
        path.to_string_lossy(),
        bytes.len()
    );
    Ok(Extracted { title, text })
}

fn extract_meeting(ext: &str, name: &str, bytes: &[u8]) -> Result<Extracted, AppError> {
    let raw = String::from_utf8_lossy(bytes);
    let text = match ext {
        "vtt" => extract_vtt(&raw),
        "srt" => extract_srt(&raw),
        "ics" => extract_ics(&raw),
        other => {
            return Err(invalid(&format!("未实现的会议记录类型 .{other}")));
        }
    };
    let text = truncate_chars(&collapse_ws(&text), MAX_TEXT_CHARS);
    if text.is_empty() {
        return Err(invalid("会议记录没有可提取的正文"));
    }
    Ok(Extracted {
        title: title_from_name(name),
        text,
    })
}

fn extract_vtt(raw: &str) -> String {
    let mut parts = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.eq_ignore_ascii_case("WEBVTT")
            || trimmed.contains("-->")
            || trimmed.chars().all(|ch| ch.is_ascii_digit())
        {
            continue;
        }
        parts.push(trimmed.to_string());
    }
    parts.join("\n")
}

fn extract_srt(raw: &str) -> String {
    let mut parts = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.contains("-->")
            || trimmed.chars().all(|ch| ch.is_ascii_digit())
        {
            continue;
        }
        parts.push(trimmed.to_string());
    }
    parts.join("\n")
}

fn extract_ics(raw: &str) -> String {
    let mut parts = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        for prefix in [
            "SUMMARY:",
            "DESCRIPTION:",
            "LOCATION:",
            "DTSTART:",
            "DTEND:",
        ] {
            if let Some(rest) = trimmed.strip_prefix(prefix) {
                parts.push(format!("{prefix}{rest}"));
            }
        }
    }
    parts.join("\n")
}

fn extract_pdf(bytes: &[u8]) -> Result<String, AppError> {
    pdf_extract::extract_text_from_mem(bytes).map_err(|error| {
        AppError::new(
            ErrorCode::ProviderInvalidResponse,
            format!("无法解析 PDF：{error}"),
        )
    })
}

fn extract_docx(bytes: &[u8]) -> Result<String, AppError> {
    let cursor = std::io::Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|error| {
        AppError::new(
            ErrorCode::ProviderInvalidResponse,
            format!("无法打开 docx：{error}"),
        )
    })?;
    let mut file = archive.by_name("word/document.xml").map_err(|error| {
        AppError::new(
            ErrorCode::ProviderInvalidResponse,
            format!("docx 缺少 word/document.xml：{error}"),
        )
    })?;
    let mut xml = String::new();
    file.read_to_string(&mut xml).map_err(|error| {
        AppError::new(
            ErrorCode::ProviderInvalidResponse,
            format!("无法读取 docx 正文：{error}"),
        )
    })?;
    Ok(strip_xml_to_text(&xml))
}

fn extract_xlsx(bytes: &[u8]) -> Result<String, AppError> {
    let cursor = std::io::Cursor::new(bytes);
    let mut workbook = calamine::open_workbook_auto_from_rs(cursor).map_err(|error| {
        AppError::new(
            ErrorCode::ProviderInvalidResponse,
            format!("无法打开 xlsx：{error}"),
        )
    })?;
    use calamine::Reader;
    let mut parts = Vec::new();
    let sheets: Vec<String> = workbook.sheet_names().to_vec();
    for name in sheets {
        let Ok(range) = workbook.worksheet_range(&name) else {
            continue;
        };
        parts.push(format!("## {name}"));
        for row in range.rows() {
            let cells: Vec<String> = row.iter().map(|cell| cell.to_string()).collect();
            let line = cells.join("\t").trim().to_string();
            if !line.is_empty() {
                parts.push(line);
            }
        }
    }
    Ok(parts.join("\n"))
}

fn extract_pptx(bytes: &[u8]) -> Result<String, AppError> {
    let cursor = std::io::Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|error| {
        AppError::new(
            ErrorCode::ProviderInvalidResponse,
            format!("无法打开 pptx：{error}"),
        )
    })?;
    let mut names: Vec<String> = archive
        .file_names()
        .filter(|name| name.starts_with("ppt/slides/slide") && name.ends_with(".xml"))
        .map(str::to_string)
        .collect();
    names.sort();
    let mut parts = Vec::new();
    for name in names {
        let mut file = archive.by_name(&name).map_err(|error| {
            AppError::new(
                ErrorCode::ProviderInvalidResponse,
                format!("无法读取 {name}：{error}"),
            )
        })?;
        let mut xml = String::new();
        file.read_to_string(&mut xml).map_err(|error| {
            AppError::new(
                ErrorCode::ProviderInvalidResponse,
                format!("无法读取 pptx 幻灯片：{error}"),
            )
        })?;
        let text = strip_xml_to_text(&xml);
        if !text.trim().is_empty() {
            parts.push(text);
        }
    }
    Ok(parts.join("\n\n"))
}

fn strip_xml_to_text(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut in_tag = false;
    for ch in xml.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    decode_entities(&out)
}

fn decode_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

fn collapse_ws(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max).collect()
}

fn title_from_name(name: &str) -> String {
    Path::new(name)
        .file_stem()
        .map(|value| value.to_string_lossy().into_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| name.to_string())
}

fn sanitize_upload_name(name: &str) -> Result<String, AppError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(invalid("文件名不能为空"));
    }
    let base = Path::new(trimmed)
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    if base.is_empty() || base != trimmed {
        return Err(invalid("文件名必须是单个文件名，不能包含路径分隔符"));
    }
    Ok(base)
}

fn ensure_uploads_dir(data_dir: &Path) -> Result<PathBuf, AppError> {
    let dir = data_dir.join(UPLOADS_DIR);
    if !dir.exists() {
        std::fs::create_dir_all(&dir).map_err(|error| {
            AppError::new(
                ErrorCode::StorageUnavailable,
                format!("无法创建上传目录 {}: {error}", dir.display()),
            )
        })?;
    }
    Ok(dir)
}

fn invalid(reason: &str) -> AppError {
    AppError::new(ErrorCode::DomainInvalidRange, reason.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_supported_kinds_and_reject_legacy_office() {
        assert_eq!(classify_file("a.md").unwrap().0, FileKind::Unstructured);
        assert_eq!(classify_file("a.PDF").unwrap().0, FileKind::Structured);
        assert_eq!(classify_file("pic.PNG").unwrap().0, FileKind::Image);
        assert_eq!(classify_file("song.mp3").unwrap().0, FileKind::Audio);
        assert_eq!(classify_file("clip.mp4").unwrap().0, FileKind::Video);
        assert_eq!(classify_file("main.rs").unwrap().0, FileKind::Code);
        assert_eq!(classify_file("standup.vtt").unwrap().0, FileKind::Meeting);
        assert!(classify_file("legacy.doc").is_err());
    }

    #[test]
    fn extract_vtt_keeps_spoken_lines() {
        let raw = "WEBVTT\n\n00:00:01.000 --> 00:00:03.000\nShip the context source plan\n";
        assert!(extract_vtt(raw).contains("Ship the context source plan"));
    }

    #[test]
    fn extract_docx_reads_paragraph_text() {
        let bytes = minimal_docx("Hello from docx");
        let text = extract_docx(&bytes).unwrap();
        assert!(text.contains("Hello from docx"));
    }

    fn minimal_docx(text: &str) -> Vec<u8> {
        use std::io::Write;
        let document_xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:body>
</w:document>"#
        );
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut cursor);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip.start_file("[Content_Types].xml", options).unwrap();
            zip.write_all(b"<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"></Types>")
                .unwrap();
            zip.start_file("word/document.xml", options).unwrap();
            zip.write_all(document_xml.as_bytes()).unwrap();
            zip.finish().unwrap();
        }
        cursor.into_inner()
    }
}
