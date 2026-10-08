//! 错误分类。
//!
//! 错误的三条硬要求：**机器可读分类 + 用户能看懂的话 + 可执行建议**，
//! 而不是把 SDK 异常原样抛到 UI 上显示成 "timeout / validation failed"。

use mc_common::error::{AppError, Component, ErrorCode, Severity};

#[test]
fn error_exposes_code_component_retryable() {
    let e = AppError::new(ErrorCode::ProviderAuthFailed, "401 unauthorized");
    assert_eq!(e.code(), ErrorCode::ProviderAuthFailed);
    assert_eq!(e.component(), Component::Provider);
    assert!(!e.retryable());

    let e = AppError::new(ErrorCode::ProviderRateLimited, "429");
    assert_eq!(e.component(), Component::Provider);
    assert!(e.retryable());

    let e = AppError::new(ErrorCode::CapturePermissionDenied, "TCC denied");
    assert_eq!(e.component(), Component::Capture);
    assert!(!e.retryable());

    let e = AppError::new(ErrorCode::StorageDiskFull, "no space left");
    assert_eq!(e.component(), Component::Storage);
}

#[test]
fn error_has_user_message_for_every_code() {
    for &code in ErrorCode::ALL {
        assert!(
            !code.as_str().is_empty(),
            "code {code:?} must have a stable machine-readable string"
        );
        assert!(
            !code.user_message().is_empty(),
            "code {code:?} ({}) must have a non-empty user-facing message",
            code.as_str()
        );
    }
}

// 0.6b — 用户可见文案不得泄漏原始技术细节（技术细节走 detail/context）
#[test]
fn user_message_is_not_the_raw_detail() {
    let e = AppError::new(
        ErrorCode::ProviderAuthFailed,
        "AttributeError: 'OpenAI' object has no attribute 'multimodal_embeddings'",
    );

    assert!(!e.user_message().contains("AttributeError"));
    assert!(e.to_string().contains("AttributeError"));
}

// 0.6c — 只有设计上可重试的错误才可重试（避免对 401 无限重试烧钱）
#[test]
fn retryable_set_is_exactly_as_designed() {
    let retryable: Vec<ErrorCode> = ErrorCode::ALL
        .iter()
        .copied()
        .filter(|c| c.retryable())
        .collect();

    for expected in [
        ErrorCode::ProviderRateLimited,
        ErrorCode::ProviderTimeout,
        ErrorCode::ProviderConnection,
        ErrorCode::ProviderServerError,
        ErrorCode::CaptureTimeout,
    ] {
        assert!(retryable.contains(&expected), "{expected:?} should retry");
    }

    for forbidden in [
        ErrorCode::ProviderAuthFailed,
        ErrorCode::ProviderNotFound,
        ErrorCode::ProviderUnsupported,
        ErrorCode::ProviderUnconfigured,
        ErrorCode::ProviderInvalidResponse,
        ErrorCode::PrivacyBlocked,
        ErrorCode::StorageCorrupt,
        ErrorCode::DomainInvalidRange,
    ] {
        assert!(
            !retryable.contains(&forbidden),
            "{forbidden:?} must not retry"
        );
    }
}

// 0.6d
#[test]
fn severity_matches_design() {
    assert_eq!(ErrorCode::StorageCorrupt.severity(), Severity::Fatal);
    assert_eq!(ErrorCode::StorageDiskFull.severity(), Severity::Error);
    assert_eq!(ErrorCode::ProviderRateLimited.severity(), Severity::Warn);
    assert_eq!(ErrorCode::PrivacyBlocked.severity(), Severity::Warn);
}

// 0.6e — 「错误要能告诉用户怎么办」：可操作的错误必须有 remediation
#[test]
fn actionable_codes_have_remediation() {
    for code in [
        ErrorCode::CapturePermissionDenied,
        ErrorCode::CaptureNoDisplay,
        ErrorCode::ProviderAuthFailed,
        ErrorCode::ProviderNotFound,
        ErrorCode::ProviderRateLimited,
        ErrorCode::ProviderUnconfigured,
        ErrorCode::StorageDiskFull,
    ] {
        assert!(
            code.remediation().is_some(),
            "{code:?} is actionable and must carry a remediation"
        );
        assert!(!code.remediation().unwrap().text.is_empty());
    }
}

// 0.6f — 诊断上下文：结构化字段可附加，且不丢失原始 detail
#[test]
fn error_carries_structured_context_without_losing_detail() {
    let e = AppError::new(ErrorCode::ProviderAuthFailed, "401 unauthorized")
        .with_context("provider", "openai_compatible")
        .with_context("model", "qwen3-vl");

    assert_eq!(
        e.context().get("model").map(String::as_str),
        Some("qwen3-vl")
    );
    assert!(e.to_string().contains("401 unauthorized"));
}

// 0.6g — retry_after 必须能透传（429 的 Retry-After）
#[test]
fn retry_after_is_preserved() {
    let e = AppError::new(ErrorCode::ProviderRateLimited, "429")
        .with_retry_after(std::time::Duration::from_secs(7));

    assert_eq!(e.retry_after(), Some(std::time::Duration::from_secs(7)));
}

// 5.2 — 换 embedding 模型导致维度变化：必须是**独立错误码 + 可执行建议**，
// 而不是让用户的检索悄悄返回空结果（硬编码 1536 维度，换模型即全盘失败）。
#[test]
fn embedding_dimension_mismatch_is_its_own_actionable_code() {
    let code = ErrorCode::StorageEmbeddingDimensionMismatch;
    assert_eq!(code.as_str(), "storage_embedding_dimension_mismatch");
    assert_eq!(code.component(), Component::Storage);
    assert!(!code.retryable(), "换模型不是重试能解决的");

    let remediation = code
        .remediation()
        .expect("维度不一致必须给出补救建议（重建索引）");
    assert!(
        remediation.text.contains("重建") || remediation.text.contains("索引"),
        "建议必须提到重建索引，实际为：{}",
        remediation.text
    );

    let e = AppError::new(code, "已存 1024 维，收到 768 维");
    assert!(e.user_message().contains("维度"));
    assert!(e.to_string().contains("1024"), "技术细节不能丢");
}

// 0.6h — 错误可序列化（要能进 pipeline_failures 表并被 API 返回）
#[test]
fn error_serializes_for_persistence_and_api() {
    let e = AppError::new(ErrorCode::ProviderServerError, "503")
        .with_context("provider", "openai_compatible");

    let json = serde_json::to_value(&e).expect("AppError must serialize");
    assert_eq!(json["code"], "provider_server_error");
    assert_eq!(json["component"], "provider");
    assert_eq!(json["retryable"], true);
    assert_eq!(json["context"]["provider"], "openai_compatible");
    assert!(!json["user_message"].as_str().unwrap().is_empty());
}
