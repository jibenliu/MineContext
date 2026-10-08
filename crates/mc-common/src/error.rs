//! 统一的错误分类。
//!
//! 任何失败都必须能回答四个问题：**哪个组件、什么错误码、用户能做什么、
//! 是否可重试**。
//!
//! 设计取舍：分类信息（code / component / retryable / severity / 用户文案）
//! **全部挂在 [`ErrorCode`] 上**，[`AppError`] 只携带 code + 原始 detail +
//! 结构化上下文。于是基础 crate 不需要知道任何子系统错误类型（无跨 crate
//! 耦合），新增一种失败只需加一个 code，而编译器会强制补齐文案与分类 ——
//! 原始技术细节完整保留在 `detail` / `context` 里供诊断使用。

use std::collections::BTreeMap;
use std::time::Duration;

use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};

/// 失败归属的子系统。用于 UI 分组与 `pipeline_failures.component`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Component {
    Config,
    Capture,
    Provider,
    Storage,
    Domain,
    Budget,
    Privacy,
    Stage,
    Summary,
}

impl Component {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Capture => "capture",
            Self::Provider => "provider",
            Self::Storage => "storage",
            Self::Domain => "domain",
            Self::Budget => "budget",
            Self::Privacy => "privacy",
            Self::Stage => "stage",
            Self::Summary => "summary",
        }
    }
}

/// 严重程度。`Fatal` 表示 daemon 应进入只读安全模式，不再写入。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Warn,
    Error,
    Fatal,
}

/// 用户可执行的补救动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemediationAction {
    /// 打开应用内设置页（模型配置、预算等）
    OpenSettings,
    /// 打开系统偏好设置（屏幕录制权限等）
    OpenSystemPreferences,
    /// 稍后自动重试即可，无需用户操作
    RetryLater,
    /// 清理磁盘空间
    FreeDiskSpace,
    /// 检查网络或代理
    CheckNetwork,
    /// 无明确动作
    None,
}

/// 给用户的补救建议。文案是静态的（便于本地化与一致性检查）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Remediation {
    pub action: RemediationAction,
    /// 深链或目标标识，例如 `x-apple.systempreferences:...` 或 `settings.models`
    pub target: Option<&'static str>,
    pub text: &'static str,
}

/// 稳定的机器可读错误码。
///
/// `as_str()` 的返回值是**对外契约**（进数据库、进 API、进日志），不得随意改名。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    // ---- config ----
    ConfigInvalid,
    ConfigUnreadable,
    ConfigUnknownTimezone,

    // ---- capture ----
    CapturePermissionDenied,
    CaptureNoDisplay,
    CaptureLocked,
    CaptureTimeout,
    CaptureIo,
    CaptureUnsupported,
    CaptureBlackFrame,

    // ---- provider ----
    ProviderUnconfigured,
    ProviderAuthFailed,
    ProviderNotFound,
    ProviderRateLimited,
    ProviderTimeout,
    ProviderConnection,
    ProviderServerError,
    ProviderInvalidResponse,
    ProviderUnsupported,

    // ---- storage ----
    StorageUnavailable,
    StorageCorrupt,
    StorageDiskFull,
    StorageMigrationFailed,
    /// blob 引用非法（绝对路径 / 路径穿越 / 前缀不在白名单内）
    StorageInvalidBlobPath,
    /// blob 文件不存在（通常是被保留策略清理，或用户手动删除）
    StorageBlobMissing,
    /// 换 embedding 模型后，新向量维度与已建索引不一致。
    /// 必须与「写入失败」区分开：它是**可恢复的配置变更**，用户重建索引即可。
    StorageEmbeddingDimensionMismatch,
    /// 导入历史版本数据目录时找不到源（路径不存在，或不是那个版本的数据布局）。
    ///
    /// 与 [`Self::StorageMigrationFailed`] 分开：那是「库已经在了但升级失败」
    /// （严重、可能需要恢复备份），这只是「你给的路径不对」，用户改路径即可。
    StorageLegacySourceMissing,

    // ---- domain ----
    DomainInvalidRange,
    DomainInvalidTimestamp,
    DomainInvariantViolated,
    DomainNothingToDo,
    /// 用户修正请求不合法（空标题、自我合并、切分点在未来……）
    DomainInvalidOverride,
    /// 总结降级为兜底：功能仍然可用，但内容不是模型写的
    ProviderUnavailableFallback,

    // ---- budget ----
    BudgetExceeded,

    // ---- privacy ----
    PrivacyBlocked,
    PrivacyRuleEngineFailed,
}

impl ErrorCode {
    /// 全部错误码。用于表驱动测试，保证新增 code 时不会漏掉分类与文案。
    pub const ALL: &'static [ErrorCode] = &[
        Self::ConfigInvalid,
        Self::ConfigUnreadable,
        Self::ConfigUnknownTimezone,
        Self::CapturePermissionDenied,
        Self::CaptureNoDisplay,
        Self::CaptureLocked,
        Self::CaptureTimeout,
        Self::CaptureIo,
        Self::CaptureUnsupported,
        Self::CaptureBlackFrame,
        Self::ProviderUnconfigured,
        Self::ProviderAuthFailed,
        Self::ProviderNotFound,
        Self::ProviderRateLimited,
        Self::ProviderTimeout,
        Self::ProviderConnection,
        Self::ProviderServerError,
        Self::ProviderInvalidResponse,
        Self::ProviderUnsupported,
        Self::StorageUnavailable,
        Self::StorageCorrupt,
        Self::StorageDiskFull,
        Self::StorageMigrationFailed,
        Self::StorageInvalidBlobPath,
        Self::StorageBlobMissing,
        Self::StorageEmbeddingDimensionMismatch,
        Self::StorageLegacySourceMissing,
        Self::DomainInvalidRange,
        Self::DomainInvalidTimestamp,
        Self::DomainInvariantViolated,
        Self::DomainNothingToDo,
        Self::DomainInvalidOverride,
        Self::ProviderUnavailableFallback,
        Self::BudgetExceeded,
        Self::PrivacyBlocked,
        Self::PrivacyRuleEngineFailed,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ConfigInvalid => "config_invalid",
            Self::ConfigUnreadable => "config_unreadable",
            Self::ConfigUnknownTimezone => "config_unknown_timezone",
            Self::CapturePermissionDenied => "capture_permission_denied",
            Self::CaptureNoDisplay => "capture_no_display",
            Self::CaptureLocked => "capture_locked",
            Self::CaptureTimeout => "capture_timeout",
            Self::CaptureIo => "capture_io",
            Self::CaptureUnsupported => "capture_unsupported",
            Self::CaptureBlackFrame => "capture_black_frame",
            Self::ProviderUnconfigured => "provider_unconfigured",
            Self::ProviderAuthFailed => "provider_auth_failed",
            Self::ProviderNotFound => "provider_not_found",
            Self::ProviderRateLimited => "provider_rate_limited",
            Self::ProviderTimeout => "provider_timeout",
            Self::ProviderConnection => "provider_connection",
            Self::ProviderServerError => "provider_server_error",
            Self::ProviderInvalidResponse => "provider_invalid_response",
            Self::ProviderUnsupported => "provider_unsupported",
            Self::StorageUnavailable => "storage_unavailable",
            Self::StorageCorrupt => "storage_corrupt",
            Self::StorageDiskFull => "storage_disk_full",
            Self::StorageMigrationFailed => "storage_migration_failed",
            Self::StorageInvalidBlobPath => "storage_invalid_blob_path",
            Self::StorageBlobMissing => "storage_blob_missing",
            Self::StorageEmbeddingDimensionMismatch => "storage_embedding_dimension_mismatch",
            Self::StorageLegacySourceMissing => "storage_legacy_source_missing",
            Self::DomainInvalidRange => "domain_invalid_range",
            Self::DomainInvalidTimestamp => "domain_invalid_timestamp",
            Self::DomainInvariantViolated => "domain_invariant_violated",
            Self::DomainNothingToDo => "domain_nothing_to_do",
            Self::DomainInvalidOverride => "domain_invalid_override",
            Self::ProviderUnavailableFallback => "provider_unavailable_fallback",
            Self::BudgetExceeded => "budget_exceeded",
            Self::PrivacyBlocked => "privacy_blocked",
            Self::PrivacyRuleEngineFailed => "privacy_rule_engine_failed",
        }
    }

    pub const fn component(self) -> Component {
        match self {
            Self::ConfigInvalid | Self::ConfigUnreadable | Self::ConfigUnknownTimezone => {
                Component::Config
            }

            Self::CapturePermissionDenied
            | Self::CaptureNoDisplay
            | Self::CaptureLocked
            | Self::CaptureTimeout
            | Self::CaptureIo
            | Self::CaptureUnsupported
            | Self::CaptureBlackFrame => Component::Capture,

            Self::ProviderUnconfigured
            | Self::ProviderAuthFailed
            | Self::ProviderNotFound
            | Self::ProviderRateLimited
            | Self::ProviderTimeout
            | Self::ProviderConnection
            | Self::ProviderServerError
            | Self::ProviderInvalidResponse
            | Self::ProviderUnsupported => Component::Provider,

            Self::StorageUnavailable
            | Self::StorageCorrupt
            | Self::StorageDiskFull
            | Self::StorageMigrationFailed
            | Self::StorageInvalidBlobPath
            | Self::StorageBlobMissing
            | Self::StorageEmbeddingDimensionMismatch
            | Self::StorageLegacySourceMissing => Component::Storage,

            Self::DomainInvalidRange
            | Self::DomainInvalidTimestamp
            | Self::DomainInvariantViolated
            | Self::DomainNothingToDo
            | Self::DomainInvalidOverride => Component::Domain,

            Self::ProviderUnavailableFallback => Component::Provider,

            Self::BudgetExceeded => Component::Budget,

            Self::PrivacyBlocked | Self::PrivacyRuleEngineFailed => Component::Privacy,
        }
    }

    /// 是否值得自动重试。
    ///
    /// 刻意保守：对 401 / 404 / 不支持的能力重试只会浪费时间与 token
    /// （没有这一层判断会导致同一张图被重复请求，白花 token）。
    pub const fn retryable(self) -> bool {
        matches!(
            self,
            Self::ProviderRateLimited
                | Self::ProviderTimeout
                | Self::ProviderConnection
                | Self::ProviderServerError
                | Self::CaptureTimeout
                | Self::CaptureIo
                | Self::StorageUnavailable
                | Self::BudgetExceeded
        )
    }

    pub const fn severity(self) -> Severity {
        match self {
            Self::StorageCorrupt | Self::StorageMigrationFailed => Severity::Fatal,

            // 这些是「正常但不理想」的情况：限流、预算、隐私拦截、未配置、无事可做
            Self::ProviderRateLimited
            | Self::ProviderUnconfigured
            | Self::BudgetExceeded
            | Self::PrivacyBlocked
            | Self::DomainNothingToDo
            | Self::ProviderUnavailableFallback
            | Self::CaptureLocked
            | Self::CaptureNoDisplay => Severity::Warn,

            _ => Severity::Error,
        }
    }

    /// 用户能看懂的一句话。**不含**任何技术细节（技术细节在 `AppError::detail`）。
    pub const fn user_message(self) -> &'static str {
        match self {
            Self::ConfigInvalid => "配置有误，请检查后重试。",
            Self::ConfigUnreadable => "无法读取配置文件。",
            Self::ConfigUnknownTimezone => {
                "配置中的时区无法识别，请使用标准的 IANA 时区名称（例如 Asia/Shanghai）。"
            }
            Self::CapturePermissionDenied => "缺少屏幕录制权限，无法截图。",
            Self::CaptureNoDisplay => "当前没有可用的显示器。",
            Self::CaptureLocked => "屏幕已锁定，录制已暂停。",
            Self::CaptureTimeout => "截图超时，正在重试。",
            Self::CaptureIo => "写入截图文件失败。",
            Self::CaptureUnsupported => "当前系统不支持该采集方式。",
            Self::CaptureBlackFrame => "截图内容为空（可能是权限未生效），请检查权限后重试。",
            Self::ProviderUnconfigured => "尚未配置模型服务，AI 功能暂不可用。",
            Self::ProviderAuthFailed => "模型服务的 API Key 无效或已过期。",
            Self::ProviderNotFound => "指定的模型不存在或无权访问。",
            Self::ProviderRateLimited => "模型服务限流，将稍后自动重试。",
            Self::ProviderTimeout => "模型服务响应超时，将自动重试。",
            Self::ProviderConnection => "无法连接到模型服务，请检查网络或代理设置。",
            Self::ProviderServerError => "模型服务暂时异常，将自动重试。",
            Self::ProviderInvalidResponse => "模型返回的内容无法解析，已保留原始结果。",
            Self::ProviderUnsupported => "该模型服务不支持所需能力。",
            Self::StorageUnavailable => "本地数据库暂时不可用，正在重试。",
            Self::StorageCorrupt => "本地数据库损坏，已进入只读安全模式。",
            Self::StorageDiskFull => "磁盘空间不足，已暂停写入截图。",
            Self::StorageMigrationFailed => "数据库升级失败。",
            Self::StorageLegacySourceMissing => {
                "找不到旧版本的数据（升级向导会告诉你找过哪些路径）。"
            }
            Self::StorageInvalidBlobPath => "截图引用无效。",
            Self::StorageBlobMissing => "截图文件不存在（可能已被自动清理）。",
            Self::StorageEmbeddingDimensionMismatch => {
                "向量维度与已建索引不一致（通常是换了 embedding 模型），需要重建语义索引。"
            }
            Self::DomainInvalidRange => "时间范围无效。",
            Self::DomainInvalidTimestamp => "时间格式无法识别。",
            Self::DomainInvariantViolated => "内部状态不一致。",
            Self::DomainNothingToDo => "所选时间段内没有可总结的内容。",
            Self::DomainInvalidOverride => "这次修改无法应用，请检查后重试。",
            Self::ProviderUnavailableFallback => "模型暂时不可用，已用本地内容生成了一份摘要。",
            Self::BudgetExceeded => "已达到本时段的用量上限，将降低分析频率。",
            Self::PrivacyBlocked => "该内容命中隐私规则，已跳过。",
            Self::PrivacyRuleEngineFailed => "隐私规则加载失败，已按最保守策略处理。",
        }
    }

    /// 可执行的补救建议。可操作的错误必须有。
    pub const fn remediation(self) -> Option<Remediation> {
        match self {
            Self::CapturePermissionDenied | Self::CaptureBlackFrame => Some(Remediation {
                action: RemediationAction::OpenSystemPreferences,
                target: Some(
                    "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
                ),
                text: "在「系统设置 → 隐私与安全性 → 屏幕录制」中勾选本应用，然后重启应用。",
            }),
            Self::CaptureNoDisplay => Some(Remediation {
                action: RemediationAction::RetryLater,
                target: None,
                text: "接上显示器或唤醒屏幕后会自动恢复。",
            }),
            Self::CaptureUnsupported => Some(Remediation {
                action: RemediationAction::OpenSettings,
                target: Some("settings.capture"),
                text: "请在采集设置中改用其它采集方式。",
            }),
            Self::ProviderUnconfigured => Some(Remediation {
                action: RemediationAction::OpenSettings,
                target: Some("settings.models"),
                text: "在「设置 → 模型」中填写 Base URL、API Key 与模型名。",
            }),
            Self::ProviderAuthFailed => Some(Remediation {
                action: RemediationAction::OpenSettings,
                target: Some("settings.models"),
                text: "请更新 API Key。",
            }),
            Self::ProviderNotFound => Some(Remediation {
                action: RemediationAction::OpenSettings,
                target: Some("settings.models"),
                text: "请确认模型名称，或在服务端开通该模型。",
            }),
            Self::ProviderConnection => Some(Remediation {
                action: RemediationAction::CheckNetwork,
                target: Some("settings.network"),
                text: "请检查网络连接与代理设置。",
            }),
            Self::ProviderRateLimited => Some(Remediation {
                action: RemediationAction::RetryLater,
                target: None,
                text: "无需操作，系统会自动重试；也可以在设置中降低截图频率或提高预算。",
            }),
            Self::ProviderUnsupported => Some(Remediation {
                action: RemediationAction::OpenSettings,
                target: Some("settings.models"),
                text: "请改用支持图像输入的模型。",
            }),
            Self::StorageDiskFull => Some(Remediation {
                action: RemediationAction::FreeDiskSpace,
                target: None,
                text: "请清理磁盘空间，或调低截图保留天数。",
            }),
            Self::StorageCorrupt => Some(Remediation {
                action: RemediationAction::OpenSettings,
                target: Some("settings.diagnostics"),
                text: "请从备份恢复数据，并导出诊断包以便排查。",
            }),
            Self::StorageInvalidBlobPath => Some(Remediation {
                action: RemediationAction::None,
                target: None,
                text: "该引用不是本应用生成的截图路径；若界面显示异常请重新打开应用。",
            }),
            Self::StorageBlobMissing => Some(Remediation {
                action: RemediationAction::RetryLater,
                target: None,
                text: "截图已被自动清理；可调高「保留天数」以避免再次发生。",
            }),
            Self::StorageEmbeddingDimensionMismatch => Some(Remediation {
                action: RemediationAction::OpenSettings,
                target: Some("settings.models"),
                text: "请在「设置 → 模型」改回原来的 embedding 模型，\
                       或在「设置 → 检索」里重建向量索引；重建前检索会退化为关键词检索。",
            }),
            Self::StorageLegacySourceMissing => Some(Remediation {
                action: RemediationAction::None,
                target: None,
                text: "请确认旧版本的数据目录（含 persist/sqlite/app.db 与 screenshots/）后重试。",
            }),
            Self::ConfigUnknownTimezone => Some(Remediation {
                action: RemediationAction::OpenSettings,
                target: Some("settings.general"),
                text: "请选择有效的时区。",
            }),
            Self::BudgetExceeded => Some(Remediation {
                action: RemediationAction::OpenSettings,
                target: Some("settings.budget"),
                text: "可以调高每小时/每日上限，或降低截图频率。",
            }),
            Self::PrivacyRuleEngineFailed => Some(Remediation {
                action: RemediationAction::OpenSettings,
                target: Some("settings.privacy"),
                text: "请检查隐私规则配置；在此之前所有内容都不会外发。",
            }),
            Self::DomainNothingToDo => Some(Remediation {
                action: RemediationAction::None,
                target: None,
                text: "换一个时间段再试。",
            }),
            _ => None,
        }
    }
}

/// 应用统一错误：稳定的错误码 + 完整技术细节 + 结构化上下文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppError {
    code: ErrorCode,
    detail: String,
    context: BTreeMap<String, String>,
    retry_after: Option<Duration>,
}

impl AppError {
    pub fn new(code: ErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
            context: BTreeMap::new(),
            retry_after: None,
        }
    }

    pub fn with_context(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.context.insert(key.into(), value.into());
        self
    }

    pub fn with_retry_after(mut self, after: Duration) -> Self {
        self.retry_after = Some(after);
        self
    }

    pub const fn code(&self) -> ErrorCode {
        self.code
    }

    pub const fn component(&self) -> Component {
        self.code.component()
    }

    pub const fn retryable(&self) -> bool {
        self.code.retryable()
    }

    pub const fn severity(&self) -> Severity {
        self.code.severity()
    }

    pub const fn user_message(&self) -> &'static str {
        self.code.user_message()
    }

    pub const fn remediation(&self) -> Option<Remediation> {
        self.code.remediation()
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }

    pub fn context(&self) -> &BTreeMap<String, String> {
        &self.context
    }

    pub const fn retry_after(&self) -> Option<Duration> {
        self.retry_after
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.detail)
    }
}

impl std::error::Error for AppError {}

/// 对外（API / 数据库 / 日志）的固定形状。
impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("AppError", 9)?;
        s.serialize_field("code", self.code.as_str())?;
        s.serialize_field("component", self.component().as_str())?;
        s.serialize_field("severity", &self.severity())?;
        s.serialize_field("retryable", &self.retryable())?;
        s.serialize_field("detail", &self.detail)?;
        s.serialize_field("user_message", self.user_message())?;
        s.serialize_field("remediation", &self.remediation().map(|r| r.text))?;
        s.serialize_field(
            "retry_after_ms",
            &self.retry_after.map(|d| d.as_millis() as u64),
        )?;
        s.serialize_field("context", &self.context)?;
        s.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_codes_are_unique_strings() {
        let mut seen = std::collections::BTreeSet::new();
        for &code in ErrorCode::ALL {
            assert!(
                seen.insert(code.as_str()),
                "duplicate code string: {}",
                code.as_str()
            );
        }
        assert_eq!(seen.len(), ErrorCode::ALL.len());
    }

    #[test]
    fn display_uses_machine_code_and_detail() {
        let e = AppError::new(ErrorCode::StorageDiskFull, "ENOSPC");
        assert_eq!(e.to_string(), "storage_disk_full: ENOSPC");
    }

    #[test]
    fn component_strings_are_snake_case() {
        assert_eq!(Component::Provider.as_str(), "provider");
        assert_eq!(Component::Privacy.as_str(), "privacy");
    }
}
