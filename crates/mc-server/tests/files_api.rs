//! `/api/files*`（上传文件列表 / 保存 / 读取 / 复制）。
//!
//! 文件服务在 `filesDir` 里
//! `writeFile` / `readFile` / `copyFile` / `readdir`。daemon 走 HTTP，
//! 因此多了一层必须自己把关的东西：**文件名不可信**。
//!
//! 直接 `path.join(filesDir, fileName)` 的话，传 `../../id_rsa` 就能读任意文件。
//! token 只证明「是本应用」，不证明参数可信 —— 这里必须失败即关闭。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mc_common::time::Timestamp;
use mc_server::{router, ServerState};
use mc_storage::Database;
use tower::ServiceExt;

const TOKEN: &str = "test-token";
use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

struct Ctx {
    dir: tempfile::TempDir,
    state: Arc<ServerState>,
}

impl Ctx {
    /// 上传目录：`<data_dir>/uploads`（`getFilesDir()`）。
    fn uploads(&self) -> std::path::PathBuf {
        self.dir.path().join("uploads")
    }
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
    Ctx { dir, state }
}

fn request(method: &str, uri: &str, body: Option<serde_json::Value>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "127.0.0.1:12345")
        .header("x-mc-token", TOKEN);
    let body = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&value).unwrap())
        }
        None => Body::empty(),
    };
    builder.body(body).unwrap()
}

async fn call(
    state: &Arc<ServerState>,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> serde_json::Value {
    let response = router(Arc::clone(state))
        .oneshot(request(method, uri, body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{method} {uri}");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).expect("必须返回 JSON 信封")
}

fn data(envelope: &serde_json::Value) -> serde_json::Value {
    assert_eq!(envelope["code"], 0, "渲染层只认 code == 0：{envelope}");
    envelope["data"].clone()
}

// 渲染层用 `new Uint8Array(...)` 传数据，`JSON.stringify` 之后是
// `{"0":104,"1":105}`（对象而非数组）。这个形状必须能存。
#[tokio::test]
async fn save_file_accepts_the_shapes_the_renderer_sends() {
    let ctx = ctx();

    // 1) JSON.stringify(Uint8Array) 的形状
    let envelope = call(
        &ctx.state,
        "POST",
        "/api/files",
        Some(serde_json::json!({ "name": "a.txt", "data": { "0": 104, "1": 105 } })),
    )
    .await;
    let payload = data(&envelope);
    assert_eq!(payload["success"], true, "{payload}");
    let path = payload["filePath"].as_str().unwrap().to_string();
    assert!(path.ends_with("a.txt"));
    assert!(std::path::Path::new(&path).exists(), "文件必须真的落盘");

    // 2) 普通数组
    call(
        &ctx.state,
        "POST",
        "/api/files",
        Some(serde_json::json!({ "name": "b.txt", "data": [104, 105] })),
    )
    .await;

    // 3) base64 字符串
    call(
        &ctx.state,
        "POST",
        "/api/files",
        Some(serde_json::json!({ "name": "c.txt", "data": "aGk=" })),
    )
    .await;

    for name in ["a.txt", "b.txt", "c.txt"] {
        let envelope = call(&ctx.state, "GET", &format!("/api/files/{name}/data"), None).await;
        assert_eq!(data(&envelope)["data"], "hi", "{name} 的内容必须是 hi");
    }
}

#[tokio::test]
async fn list_files_matches_the_legacy_row_shape() {
    let ctx = ctx();
    call(
        &ctx.state,
        "POST",
        "/api/files",
        Some(serde_json::json!({ "name": "报告.md", "data": [35, 32] })),
    )
    .await;

    let envelope = call(&ctx.state, "GET", "/api/files", None).await;
    let payload = data(&envelope);
    assert_eq!(payload["success"], true);

    let files = payload["files"].as_array().expect("files 必须是数组");
    assert_eq!(files.len(), 1);
    let file = &files[0];
    assert_eq!(file["name"], "报告.md");
    assert_eq!(file["status"], "Uploaded");
    assert!(
        file["filePath"]
            .as_str()
            .unwrap()
            .starts_with(&ctx.uploads().to_string_lossy().to_string()),
        "filePath 必须是绝对路径：{file}"
    );
    // `source` 是给人看的：`<扩展名大写> · <MB>MB`
    let source = file["source"].as_str().unwrap();
    assert!(source.starts_with("MD · "), "source 形状：{source}");
    assert!(source.ends_with("MB"), "source 形状：{source}");
}

#[tokio::test]
async fn read_of_a_missing_file_is_a_payload_error_not_a_crash() {
    let ctx = ctx();
    let envelope = call(&ctx.state, "GET", "/api/files/nope.txt/data", None).await;
    // 旧 `readFile` 返回 `{success:false, error}`，不抛异常
    let payload = data(&envelope);
    assert_eq!(payload["success"], false);
    assert!(payload["error"].is_string());
}

/// 目录穿越必须失败即关闭：token 只证明「是本应用」，不证明参数可信。
#[tokio::test]
async fn traversal_is_rejected_everywhere() {
    let ctx = ctx();

    // 造一个「目录外的秘密文件」
    let secret = ctx.dir.path().join("secret.txt");
    std::fs::write(&secret, b"top secret").unwrap();
    std::fs::create_dir_all(ctx.uploads()).unwrap();

    // 字面量 `..` 在 HTTP 层就被规范化掉了（axum 返回 404，压根到不了处理器）。
    // 这也是拒绝，但不是我们这层把关的证明 —— 因此只断言「绝不返回内容」。
    let response = router(Arc::clone(&ctx.state))
        .oneshot(request("GET", "/api/files/../secret.txt/data", None))
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&bytes).to_string();
    assert!(
        status != StatusCode::OK || !body.contains("top secret"),
        "绝不能把目录外的内容读出来：{status} {body}"
    );

    // URL 编码的穿越不会被 HTTP 层规范化，必须由我们这层挡住
    let envelope = call(
        &ctx.state,
        "GET",
        "/api/files/%2e%2e%2fsecret.txt/data",
        None,
    )
    .await;
    assert_eq!(envelope["code"], 1, "编码后的穿越必须被拒绝：{envelope}");
    assert!(!envelope.to_string().contains("top secret"));

    // 编码后的绝对路径同理
    let envelope = call(&ctx.state, "GET", "/api/files/%2Fetc%2Fpasswd/data", None).await;
    assert_eq!(envelope["code"], 1, "绝对路径必须被拒绝：{envelope}");

    // 写入也不能穿越
    let envelope = call(
        &ctx.state,
        "POST",
        "/api/files",
        Some(serde_json::json!({ "name": "../escape.txt", "data": [1] })),
    )
    .await;
    assert_eq!(envelope["code"], 1);
    assert!(!ctx.dir.path().join("escape.txt").exists());
}

#[tokio::test]
async fn copy_brings_an_external_file_in_using_only_its_basename() {
    let ctx = ctx();
    let outside = ctx.dir.path().join("outside.csv");
    std::fs::write(&outside, b"a,b\n1,2\n").unwrap();

    let envelope = call(
        &ctx.state,
        "POST",
        "/api/files/copy",
        Some(serde_json::json!({ "path": outside.to_string_lossy() })),
    )
    .await;
    assert_eq!(data(&envelope)["success"], true, "{envelope}");

    assert!(
        ctx.uploads().join("outside.csv").exists(),
        "必须复制进上传目录"
    );
    assert!(outside.exists(), "源文件保持不动");

    // 不存在的源文件 → `{success:false}`（也不抛）
    let envelope = call(
        &ctx.state,
        "POST",
        "/api/files/copy",
        Some(serde_json::json!({ "path": ctx.dir.path().join("nope.csv").to_string_lossy() })),
    )
    .await;
    assert_eq!(data(&envelope)["success"], false);
}

#[tokio::test]
async fn copy_rejects_relative_sources() {
    let ctx = ctx();
    let envelope = call(
        &ctx.state,
        "POST",
        "/api/files/copy",
        Some(serde_json::json!({ "path": "../../etc/passwd" })),
    )
    .await;
    assert_eq!(envelope["code"], 1, "相对路径的来源必须被拒绝：{envelope}");
}

#[tokio::test]
async fn uploads_directory_is_created_on_demand() {
    let ctx = ctx();
    assert!(!ctx.uploads().exists(), "前置条件：目录还不存在");

    let envelope = call(&ctx.state, "GET", "/api/files", None).await;
    let payload = data(&envelope);
    assert_eq!(payload["success"], true);
    assert_eq!(payload["files"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn binary_files_can_be_read_as_base64_without_utf8_loss() {
    let ctx = ctx();
    call(
        &ctx.state,
        "POST",
        "/api/files",
        Some(serde_json::json!({
            "name": "image.png", "data": [137, 80, 78, 71]
        })),
    )
    .await;
    let response = call(
        &ctx.state,
        "GET",
        "/api/files/image.png/data?encoding=base64",
        None,
    )
    .await;
    assert_eq!(data(&response)["data"], "iVBORw==");
}
