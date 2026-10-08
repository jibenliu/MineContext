//! 响应封装。
//!
//! 兼容面（渲染层依赖）必须 **HTTP 200 + `{code, status, message, data}`**：
//! `services/*.ts` 直接读 `response.data.data`，非 200 会直接抛异常。
//! 同时新增 `error_code` / `remediation` 字段供诊断页使用。

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use mc_common::error::AppError;
use mc_common::observability::{error_summary, warn};
use serde_json::{json, Value};

pub const COMPAT_FAILURE_CODE: i32 = 1;

/// 兼容面成功响应。
pub fn ok(data: Value) -> Response {
    Json(json!({
        "code": 0,
        "status": 200,
        "message": "success",
        "data": data,
        "error_code": Value::Null,
        "remediation": Value::Null,
    }))
    .into_response()
}

/// 兼容面失败响应：HTTP 200 + 非零 code（**刻意不用 501/4xx**）。
pub fn compat_failure(error: &AppError) -> Response {
    warn!(
        component = "server",
        event = "request_failed",
        code = error.code().as_str(),
        detail = %error_summary(error),
        "兼容面请求失败"
    );
    Json(json!({
        "code": COMPAT_FAILURE_CODE,
        "status": 200,
        "message": error.user_message(),
        "data": Value::Null,
        "error_code": error.code().as_str(),
        "remediation": error.remediation().map(|r| r.text),
        "detail": error.detail(),
    }))
    .into_response()
}

/// 明确的「尚未实现」：让前端看到结构化原因，而不是 404 静默失败。
pub fn not_implemented(what: &str) -> Response {
    let error = AppError::new(
        mc_common::error::ErrorCode::DomainNothingToDo,
        format!("{what} 尚未实现"),
    );
    Json(json!({
        "code": COMPAT_FAILURE_CODE,
        "status": 200,
        "message": "该功能尚未实现",
        "data": Value::Null,
        "error_code": "not_implemented",
        "remediation": error.remediation().map(|r| r.text),
        "detail": format!("{what} 尚未实现"),
    }))
    .into_response()
}

/// 真正的 404：路径既不在兼容面契约里，也不是 v1 接口。
pub fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({
            "code": COMPAT_FAILURE_CODE,
            "status": 404,
            "message": "接口不存在",
            "data": Value::Null,
            "error_code": "not_found",
            "remediation": Value::Null,
        })),
    )
        .into_response()
}

/// v1 接口的错误响应：使用标准 HTTP 状态码 + 同一套错误 JSON。
pub fn error_response(status: StatusCode, error: &AppError) -> Response {
    error_response_with_data(status, error, Value::Null)
}

/// 带 `data` 的错误响应。
///
/// 有些「失败」本身带着有用的信息：例如任意时段总结的空范围，
/// 前端需要拿到范围内的计数才能解释「为什么不能生成」。
/// 只给一个错误码会让 UI 只能干瞪眼。
pub fn error_response_with_data(status: StatusCode, error: &AppError, data: Value) -> Response {
    warn!(
        component = "server",
        event = "request_failed",
        code = error.code().as_str(),
        status = status.as_u16(),
        detail = %error_summary(error),
        "请求失败"
    );
    (
        status,
        Json(json!({
            "code": COMPAT_FAILURE_CODE,
            "status": status.as_u16(),
            "message": error.user_message(),
            "data": data,
            "error_code": error.code().as_str(),
            "remediation": error.remediation().map(|r| r.text),
            "detail": error.detail(),
        })),
    )
        .into_response()
}
