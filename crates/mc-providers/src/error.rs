//! Provider 错误分类。
//!
//! 目标：任何失败都要能回答「哪个组件、什么原因、是否可重试、用户该做什么」，
//! 而不是把技术细节展示成一句笼统的 `timeout / validation failed`。

use std::time::Duration;

use mc_common::error::{AppError, ErrorCode};

use crate::transport::TransportError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    Unconfigured,
    AuthFailed { status: u16, detail: String },
    NotFound { model: String },
    RateLimited { retry_after: Option<Duration> },
    Timeout,
    Connection { detail: String },
    ServerError { status: u16 },
    InvalidResponse { detail: String },
    Unsupported { feature: String },
    Cancelled,
}

impl ProviderError {
    /// 是否值得自动重试。
    ///
    /// 刻意保守：对 401 / 404 / 不支持的能力重试只会浪费时间与 token
    /// （缺这一层判断会导致同一张图被重复请求）。
    pub const fn retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. }
                | Self::Timeout
                | Self::Connection { .. }
                | Self::ServerError { .. }
        )
    }

    pub fn to_app_error(&self) -> AppError {
        let (code, detail) = match self {
            Self::Unconfigured => (
                ErrorCode::ProviderUnconfigured,
                "provider 未配置（base_url 或 model 为空）".to_string(),
            ),
            Self::AuthFailed { status, detail } => (
                ErrorCode::ProviderAuthFailed,
                format!("HTTP {status}: {detail}"),
            ),
            Self::NotFound { model } => (
                ErrorCode::ProviderNotFound,
                format!("模型 {model} 不存在或无权访问"),
            ),
            Self::RateLimited { retry_after } => (
                ErrorCode::ProviderRateLimited,
                match retry_after {
                    Some(after) => format!("HTTP 429，Retry-After={}s", after.as_secs()),
                    None => "HTTP 429".to_string(),
                },
            ),
            Self::Timeout => (ErrorCode::ProviderTimeout, "请求超时".to_string()),
            Self::Connection { detail } => {
                (ErrorCode::ProviderConnection, format!("无法连接: {detail}"))
            }
            Self::ServerError { status } => {
                (ErrorCode::ProviderServerError, format!("HTTP {status}"))
            }
            Self::InvalidResponse { detail } => (
                ErrorCode::ProviderInvalidResponse,
                format!("响应无法解析: {detail}"),
            ),
            Self::Unsupported { feature } => (
                ErrorCode::ProviderUnsupported,
                format!("端点不支持能力: {feature}"),
            ),
            Self::Cancelled => (ErrorCode::ProviderTimeout, "请求已取消".to_string()),
        };

        let mut error = AppError::new(code, detail);
        if let Self::RateLimited {
            retry_after: Some(after),
        } = self
        {
            error = error.with_retry_after(*after);
        }
        error
    }
}

/// HTTP 状态码 → 分类。这是错误映射的唯一实现处。
pub fn classify_status(
    status: u16,
    body: &str,
    retry_after: Option<Duration>,
    model: &str,
) -> ProviderError {
    let detail = extract_error_message(body);

    match status {
        401 | 403 => ProviderError::AuthFailed { status, detail },
        404 => ProviderError::NotFound {
            model: model.to_string(),
        },
        408 | 504 => ProviderError::Timeout,
        429 => ProviderError::RateLimited { retry_after },
        400..=499 => ProviderError::InvalidResponse { detail },
        _ => ProviderError::ServerError { status },
    }
}

pub fn classify_transport(error: TransportError) -> ProviderError {
    match error {
        TransportError::Timeout => ProviderError::Timeout,
        TransportError::Connect(detail) => ProviderError::Connection { detail },
        TransportError::Other(detail) => ProviderError::Connection { detail },
    }
}

/// 尽力从错误响应里抽出人话。抽不到就返回原文截断。
fn extract_error_message(body: &str) -> String {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
        for path in [
            &json["error"]["message"],
            &json["error"]["code"],
            &json["message"],
            &json["detail"],
        ] {
            if let Some(text) = path.as_str() {
                if !text.is_empty() {
                    return text.to_string();
                }
            }
        }
    }

    let trimmed = body.trim();
    if trimmed.is_empty() {
        "（响应体为空）".to_string()
    } else {
        trimmed.chars().take(200).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_message_from_various_error_shapes() {
        assert_eq!(
            extract_error_message(r#"{"error":{"message":"invalid api key"}}"#),
            "invalid api key"
        );
        assert_eq!(extract_error_message(r#"{"message":"nope"}"#), "nope");
        assert_eq!(extract_error_message("plain text"), "plain text");
        assert_eq!(extract_error_message(""), "（响应体为空）");
    }

    #[test]
    fn long_bodies_are_truncated() {
        let long = "x".repeat(500);
        assert_eq!(extract_error_message(&long).chars().count(), 200);
    }
}
