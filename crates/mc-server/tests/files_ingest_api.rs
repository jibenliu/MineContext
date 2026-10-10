//! `POST /api/v1/files/import` —— 用户上传本地文件，抽取正文落成笔记树文档并进入检索。
//!
//! 验收：文档 / 图片 / 代码 / 音视频 / 会议记录变成可搜的本地笔记；
//! 未知类型与空正文给出结构化错误，而不是静默空笔记。

use std::io::Write;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mc_common::time::Timestamp;
use mc_search::{DocumentKind, SearchFilters};
use mc_server::retrieval::retrieve_filtered;
use mc_server::{router, ServerState};
use mc_storage::Database;
use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;
use tower::ServiceExt;

const TOKEN: &str = "test-token";

struct Ctx {
    _dir: tempfile::TempDir,
    state: Arc<ServerState>,
}

fn ctx() -> Ctx {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let config = mc_config::load::load(&mc_config::load::LoadRequest::default()).unwrap();
    let state = Arc::new(ServerState::new(
        mc_config::ConfigHandle::new(config),
        db,
        TOKEN.to_string(),
        Timestamp::from_millis(T0),
        dir.path().to_path_buf(),
    ));
    Ctx { _dir: dir, state }
}

async fn call(
    state: &Arc<ServerState>,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/api/v1/files/import")
        .header("host", "127.0.0.1:12345")
        .header("x-mc-token", TOKEN);
    let request = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            builder
                .body(Body::from(serde_json::to_vec(&value).unwrap()))
                .unwrap()
        }
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = router(Arc::clone(state)).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).expect("必须返回 JSON 信封");
    (status, json)
}

fn bytes_as_object(bytes: &[u8]) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (index, byte) in bytes.iter().enumerate() {
        map.insert(index.to_string(), serde_json::json!(*byte));
    }
    serde_json::Value::Object(map)
}

fn assert_searchable(state: &ServerState, id: i64, needle: &str) {
    let hits = retrieve_filtered(
        &state.db,
        needle,
        10,
        SearchFilters {
            kinds: vec![DocumentKind::Document],
            ..Default::default()
        },
        None,
    )
    .expect("笔记必须进入文档检索");
    assert!(
        hits.iter()
            .any(|hit| hit.document.id == format!("note-{id}")),
        "检索应命中新笔记 note-{id}：{hits:?}"
    );
}

#[tokio::test]
async fn importing_markdown_creates_a_searchable_vault_note_and_keeps_upload_blob() {
    let ctx = ctx();
    let body_text = "# Sprint notes\n\nOwnership and borrowing are core ideas.\n";
    let (status, envelope) = call(
        &ctx.state,
        Some(serde_json::json!({
            "name": "sprint-notes.md",
            "data": body_text.as_bytes(),
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    assert_eq!(envelope["code"], 0);
    let data = &envelope["data"];
    let id = data["id"].as_i64().expect("必须返回笔记 id");
    assert_eq!(data["title"], "sprint-notes");
    assert_eq!(data["name"], "sprint-notes.md");
    assert_eq!(data["kind"], "unstructured");

    let row = ctx
        .state
        .db
        .vault_row_by_id(id)
        .unwrap()
        .expect("笔记必须落库");
    assert_eq!(row.document_type, "vaults");
    assert!(row.content.contains("sprint-notes.md"));
    assert!(row.content.contains("Ownership and borrowing"));
    assert!(row.tags.contains("file"), "tags={}", row.tags);
    assert!(row.tags.contains("unstructured"), "tags={}", row.tags);

    let stored = ctx.state.data_dir.join("uploads").join("sprint-notes.md");
    assert!(stored.is_file(), "原始文件应保留在 uploads/");
    assert_eq!(std::fs::read_to_string(&stored).unwrap(), body_text);

    assert_searchable(&ctx.state, id, "Ownership and borrowing");
}

#[tokio::test]
async fn importing_pdf_extracts_text_into_a_searchable_vault_note() {
    let ctx = ctx();
    let pdf = minimal_pdf_with_text("Hello Structured PDF");
    let (status, envelope) = call(
        &ctx.state,
        Some(serde_json::json!({
            "name": "brief.pdf",
            "data": bytes_as_object(&pdf),
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    let data = &envelope["data"];
    assert_eq!(data["kind"], "structured");
    let id = data["id"].as_i64().unwrap();
    let row = ctx.state.db.vault_row_by_id(id).unwrap().unwrap();
    assert!(
        row.content.contains("Hello Structured PDF"),
        "PDF 正文应被抽出：{}",
        row.content
    );
    assert!(row.tags.contains("structured"), "tags={}", row.tags);
    assert_searchable(&ctx.state, id, "Hello Structured PDF");
}

#[tokio::test]
async fn importing_docx_extracts_paragraph_text() {
    let ctx = ctx();
    let docx = minimal_docx_with_text("Quarterly review action items");
    let (status, envelope) = call(
        &ctx.state,
        Some(serde_json::json!({
            "name": "review.docx",
            "data": base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                &docx
            ),
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    let id = envelope["data"]["id"].as_i64().unwrap();
    assert_eq!(envelope["data"]["kind"], "structured");
    let row = ctx.state.db.vault_row_by_id(id).unwrap().unwrap();
    assert!(row.content.contains("Quarterly review action items"));
    assert_searchable(&ctx.state, id, "Quarterly review");
}

#[tokio::test]
async fn importing_image_creates_vault_note_with_filename_and_keeps_blob() {
    let ctx = ctx();
    let png = minimal_png();
    let (status, envelope) = call(
        &ctx.state,
        Some(serde_json::json!({
            "name": "whiteboard.png",
            "data": bytes_as_object(&png),
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    let data = &envelope["data"];
    assert_eq!(data["kind"], "image");
    let id = data["id"].as_i64().unwrap();
    let row = ctx.state.db.vault_row_by_id(id).unwrap().unwrap();
    assert!(row.content.contains("whiteboard.png"));
    assert!(row.content.contains("!["), "图片笔记应带 markdown 图片引用");
    assert!(row.tags.contains("image"), "tags={}", row.tags);
    assert!(ctx
        .state
        .data_dir
        .join("uploads")
        .join("whiteboard.png")
        .is_file());
    assert_searchable(&ctx.state, id, "whiteboard");
}

#[tokio::test]
async fn importing_code_creates_a_searchable_vault_note() {
    let ctx = ctx();
    let source = "fn ownership_demo() { /* borrow checker */ }\n";
    let (status, envelope) = call(
        &ctx.state,
        Some(serde_json::json!({
            "name": "ownership.rs",
            "data": source.as_bytes(),
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    let id = envelope["data"]["id"].as_i64().expect("id");
    assert_eq!(envelope["data"]["kind"], "code");
    let row = ctx.state.db.vault_row_by_id(id).unwrap().unwrap();
    assert!(row.content.contains("borrow checker"));
    assert!(row.tags.contains("code"), "tags={}", row.tags);
    assert_searchable(&ctx.state, id, "borrow checker");
}

#[tokio::test]
async fn importing_audio_and_video_creates_metadata_notes() {
    let ctx = ctx();
    for (name, kind, needle) in [
        ("standup.mp3", "audio", "standup"),
        ("demo.mp4", "video", "demo"),
    ] {
        let (status, envelope) = call(
            &ctx.state,
            Some(serde_json::json!({
                "name": name,
                "data": b"media-bytes-not-empty",
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "name={name} envelope={envelope}");
        assert_eq!(envelope["data"]["kind"], kind);
        let id = envelope["data"]["id"].as_i64().expect("id");
        let row = ctx.state.db.vault_row_by_id(id).unwrap().unwrap();
        assert!(
            row.content.contains("不进行语音转写"),
            "content={}",
            row.content
        );
        assert!(row.tags.contains(kind), "tags={}", row.tags);
        assert!(
            ctx.state.data_dir.join("uploads").join(name).is_file(),
            "blob missing for {name}"
        );
        assert_searchable(&ctx.state, id, needle);
    }
}

#[tokio::test]
async fn importing_meeting_transcript_creates_a_searchable_note() {
    let ctx = ctx();
    let vtt = "WEBVTT\n\n1\n00:00:01.000 --> 00:00:04.000\nQuarterly roadmap review decided to ship RSS ingest.\n";
    let (status, envelope) = call(
        &ctx.state,
        Some(serde_json::json!({
            "name": "roadmap-review.vtt",
            "data": vtt.as_bytes(),
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    let id = envelope["data"]["id"].as_i64().expect("id");
    assert_eq!(envelope["data"]["kind"], "meeting");
    let row = ctx.state.db.vault_row_by_id(id).unwrap().unwrap();
    assert!(row.content.contains("RSS ingest"));
    assert!(row.tags.contains("meeting"), "tags={}", row.tags);
    assert_searchable(&ctx.state, id, "RSS ingest");
}

#[tokio::test]
async fn empty_extractable_text_does_not_leave_an_empty_note() {
    let ctx = ctx();
    let (status, envelope) = call(
        &ctx.state,
        Some(serde_json::json!({
            "name": "blank.txt",
            "data": b"   \n\t  ",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{envelope}");
    assert_ne!(envelope["code"], 0);
    let rows = ctx
        .state
        .db
        .query_vault_rows(&mc_storage::vaults::VaultQuery {
            document_type: vec!["vaults".into()],
            parent_id: None,
            title: None,
            is_folder: Some(0),
            is_deleted: Some(0),
        })
        .unwrap();
    assert!(rows.is_empty(), "空正文不得留下空笔记：{rows:?}");
}

/// 最小可抽取 PDF：带正确 xref 的 Helvetica 单行文本。
fn minimal_pdf_with_text(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"%PDF-1.4\n");
    let mut offsets = [0usize; 6];
    let mut write_obj = |out: &mut Vec<u8>, id: usize, body: &[u8]| {
        offsets[id] = out.len();
        out.extend_from_slice(format!("{id} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    };
    write_obj(&mut out, 1, b"<< /Type /Catalog /Pages 2 0 R >>");
    write_obj(&mut out, 2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    write_obj(
        &mut out,
        3,
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
    );
    let stream = format!("BT /F1 12 Tf 50 100 Td ({text}) Tj ET");
    let mut content = format!("<< /Length {} >>\nstream\n", stream.len()).into_bytes();
    content.extend_from_slice(stream.as_bytes());
    content.extend_from_slice(b"\nendstream");
    write_obj(&mut out, 4, &content);
    write_obj(
        &mut out,
        5,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    );
    let xref_pos = out.len();
    out.extend_from_slice(b"xref\n0 6\n");
    out.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets.iter().skip(1) {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref_pos}\n%%EOF\n").as_bytes(),
    );
    out
}

fn minimal_png() -> Vec<u8> {
    // 1×1 RGBA PNG
    hex_bytes("89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000a49444154789c63000100000500010d0a2db40000000049454e44ae426082")
}

fn hex_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

fn minimal_docx_with_text(text: &str) -> Vec<u8> {
    let document_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:body>
</w:document>"#
    );
    let content_types = r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"#;
    let rels = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#;

    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut cursor);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("[Content_Types].xml", options).unwrap();
        zip.write_all(content_types.as_bytes()).unwrap();
        zip.start_file("_rels/.rels", options).unwrap();
        zip.write_all(rels.as_bytes()).unwrap();
        zip.start_file("word/document.xml", options).unwrap();
        zip.write_all(document_xml.as_bytes()).unwrap();
        zip.finish().unwrap();
    }
    cursor.into_inner()
}
