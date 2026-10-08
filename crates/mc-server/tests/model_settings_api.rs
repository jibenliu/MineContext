//! 模型设置的读写（`/api/model_settings/*`）：前端已在用的接口，原先只返回「未实现」。
//!
//! 设置页一直通过 `services/axiosConfig` 直接打旧后端的这两个路径。后端切到
//! rust 之后它们必须仍然可用，否则「设置页」直接是坏的 —— 而它是 S1 验收里
//! 明确要求能打开的第三页。
//!
//! 与**一处刻意的差异**：旧 `get` 把 `api_key` 明文回传给前端；
//! 这里一律回空串，只给 `hasApiKey` 布尔值。明文密钥不进任何 HTTP 响应
//! （安全属性清单里的 `api_key_never_appears_in_logs` 同样覆盖这条路径）。
//! 更新时如果前端给了新密钥，写进 **0600 的 sidecar**，配置里只留引用。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mc_common::time::Timestamp;
use mc_server::{router, ServerState};
use mc_storage::Database;
use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;
use tower::ServiceExt;

const TOKEN: &str = "model-settings-token-0123456789";
const SECRET: &str = "sk-live-MODELSETTINGS0123456789";

struct Ctx {
    dir: tempfile::TempDir,
    state: Arc<ServerState>,
    config_path: std::path::PathBuf,
}

fn ctx() -> Ctx {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    std::fs::write(&config_path, "[capture]\nenabled = true\n").unwrap();

    let db = Arc::new(Database::open(dir.path().join("data/minecontext.db")).unwrap());
    let request = mc_config::load::LoadRequest {
        layers: vec![mc_config::load::LayerSource::File(config_path.clone())],
        env: Vec::new(),
        read_process_env: false,
    };
    let loaded = mc_config::load::load(&request).unwrap();

    let state = Arc::new(
        ServerState::new(
            mc_config::ConfigHandle::new(loaded),
            db,
            TOKEN.to_string(),
            Timestamp::from_millis(T0),
            dir.path().to_path_buf(),
        )
        .with_config_write(request, config_path.clone()),
    );

    Ctx {
        dir,
        state,
        config_path,
    }
}

async fn call(
    state: &Arc<ServerState>,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "127.0.0.1:12345")
        .header("x-mc-token", TOKEN);
    let request = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            builder.body(Body::from(value.to_string())).unwrap()
        }
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = router(Arc::clone(state)).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

fn update_body(api_key: &str) -> serde_json::Value {
    serde_json::json!({
        "config": {
            "modelPlatform": "openai",
            "modelId": "qwen3-vl",
            "baseUrl": "https://api.example.com/v1",
            "apiKey": api_key,
            "embeddingModelPlatform": "openai",
            "embeddingModelId": "embed-small",
            "embeddingBaseUrl": "https://api.example.com/v1",
            "embeddingApiKey": ""
        }
    })
}

#[tokio::test]
async fn get_returns_the_shape_the_settings_page_reads() {
    let ctx = ctx();

    let (status, json) = call(&ctx.state, "GET", "/api/model_settings/get", None).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["code"], 0, "{json}");
    let config = &json["data"]["config"];
    for field in [
        "modelPlatform",
        "modelId",
        "baseUrl",
        "apiKey",
        "embeddingModelId",
        "embeddingBaseUrl",
        "embeddingApiKey",
        "embeddingModelPlatform",
    ] {
        assert!(
            config.get(field).is_some(),
            "设置页会读 {field}，缺了它表单就是空的：{json}"
        );
    }
    assert_eq!(config["apiKey"], "", "密钥不明文回传");
    assert_eq!(config["embeddingApiKey"], "");
}

#[tokio::test]
async fn update_writes_the_config_layer_and_takes_effect_immediately() {
    let ctx = ctx();

    let (status, json) = call(
        &ctx.state,
        "POST",
        "/api/model_settings/update",
        Some(update_body(SECRET)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["code"], 0, "{json}");

    let written = std::fs::read_to_string(&ctx.config_path).unwrap();
    assert!(written.contains("qwen3-vl"), "写进用户配置层：{written}");
    assert!(written.contains("https://api.example.com/v1"), "{written}");

    // 热重载：运行中的配置立刻变了（不用重启 daemon）
    let live = ctx.state.config.current();
    assert_eq!(live.config.ai.vision.model, "qwen3-vl");
    assert_eq!(live.config.ai.embedding.model, "embed-small");
}

#[tokio::test]
async fn a_new_api_key_goes_to_a_private_sidecar_not_into_the_config() {
    let ctx = ctx();

    let (status, json) = call(
        &ctx.state,
        "POST",
        "/api/model_settings/update",
        Some(update_body(SECRET)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["code"], 0, "{json}");

    let written = std::fs::read_to_string(&ctx.config_path).unwrap();
    assert!(
        !written.contains(SECRET),
        "明文密钥绝不能进配置文件：{written}"
    );
    assert!(
        written.contains("api_key_ref"),
        "配置里应当只留引用：{written}"
    );

    // sidecar 是 0600，且内容里是待导入的凭据
    let sidecar = ctx.dir.path().join("model-keys.json");
    let payload = std::fs::read_to_string(&sidecar).expect("sidecar 必须写出来");
    assert!(payload.contains(SECRET), "密钥要写进 sidecar 供导入");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&sidecar).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "sidecar 必须是 0600，实际 {mode:o}");
    }
}

#[tokio::test]
async fn update_without_a_key_is_rejected_when_none_is_configured() {
    let ctx = ctx();

    let (status, json) = call(
        &ctx.state,
        "POST",
        "/api/model_settings/update",
        Some(update_body("")),
    )
    .await;

    // 写一个空引用会让配置看起来「保存成功」，真正调用模型时才失败 —— 不如当场说清
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert!(
        json["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("API Key"),
        "{json}"
    );
    let written = std::fs::read_to_string(&ctx.config_path).unwrap();
    assert_eq!(written, "[capture]\nenabled = true\n", "什么都没写");
}

#[tokio::test]
async fn update_rejects_an_incomplete_configuration() {
    let ctx = ctx();

    let (status, json) = call(
        &ctx.state,
        "POST",
        "/api/model_settings/update",
        Some(serde_json::json!({
            "config": {
                "modelPlatform": "openai",
                "modelId": "",
                "baseUrl": "https://api.example.com/v1",
                "apiKey": "",
                "embeddingModelId": "embed-small"
            }
        })),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_ne!(json["code"], 0);
    assert!(
        json["detail"].as_str().unwrap_or_default().contains("模型"),
        "要告诉用户缺什么（具体原因在 detail 里）：{json}"
    );
    // 校验失败绝不能动磁盘
    let written = std::fs::read_to_string(&ctx.config_path).unwrap();
    assert_eq!(written, "[capture]\nenabled = true\n");
}

#[tokio::test]
async fn the_api_key_never_appears_in_any_response() {
    let ctx = ctx();

    let (_, update) = call(
        &ctx.state,
        "POST",
        "/api/model_settings/update",
        Some(update_body(SECRET)),
    )
    .await;
    assert!(!update.to_string().contains(SECRET), "{update}");

    let (_, get) = call(&ctx.state, "GET", "/api/model_settings/get", None).await;
    assert!(!get.to_string().contains(SECRET), "{get}");
    assert_eq!(get["data"]["hasApiKey"], true, "只回「有没有配」");
}

#[tokio::test]
async fn validate_reports_shape_problems_without_calling_the_model() {
    let ctx = ctx();

    let (status, json) = call(
        &ctx.state,
        "POST",
        "/api/model_settings/validate",
        Some(serde_json::json!({
            "config": {
                "modelPlatform": "openai",
                "modelId": "qwen3-vl",
                "baseUrl": "not-a-url",
                "apiKey": "",
                "embeddingModelId": "embed-small"
            }
        })),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["data"]["valid"], false, "{json}");
    assert!(
        json["data"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Base URL"),
        "要说清哪一项不合法：{json}"
    );
}

#[tokio::test]
async fn these_paths_no_longer_answer_with_not_implemented() {
    let ctx = ctx();

    for (method, uri) in [
        ("GET", "/api/model_settings/get"),
        ("POST", "/api/model_settings/update"),
    ] {
        let (_, json) = call(&ctx.state, method, uri, Some(update_body(""))).await;
        assert_ne!(
            json["error_code"], "not_implemented",
            "{method} {uri} 仍然返回未实现：{json}"
        );
    }
}
