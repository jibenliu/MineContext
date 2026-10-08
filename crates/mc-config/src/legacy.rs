//! 旧配置迁移（YAML → TOML）。
//!
//! 输入是旧版的 `config.yaml` 与 `user_setting.yaml`。三件事必须成立：
//! 已知字段不丢；**无法映射的字段变成 warnings** 而不是静默消失（「升了级
//! 发现设置没了且没人告诉我」是最糟糕的迁移体验）；**明文密钥不进新配置**，
//! 转成 Keychain 引用交给调用方导入。
//!
//! 实现上刻意用「取走（take）」而不是「读取」：取走已知路径，剩下什么就是
//! 未映射什么 —— 新增映射规则时 warnings 会自动减少，不会漏报。

use mc_common::error::{AppError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_yaml::Value;

use crate::load::ConfigWarning;
use crate::model::Config;

/// 未映射字段的告警上限（防止一份超长旧配置刷出上千条）
const UNMAPPED_WARNING_CAP: usize = 200;

pub struct LegacyInput<'a> {
    pub config_yaml: Option<&'a str>,
    pub user_setting_yaml: Option<&'a str>,
}

/// 需要写进 Keychain 的密钥。**不会**出现在 `Config` 里；
/// 调用方应把它写入权限 0600 的 sidecar 并导入系统 Keychain。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeychainImport {
    /// Keychain account，例如 `provider:vision`
    pub account: String,
    pub secret: String,
}

#[derive(Debug, Clone)]
pub struct MigrationResult {
    pub config: Config,
    pub warnings: Vec<ConfigWarning>,
    pub keychain_imports: Vec<KeychainImport>,
}

pub fn migrate_legacy(input: &LegacyInput<'_>) -> Result<MigrationResult, AppError> {
    let mut warnings: Vec<ConfigWarning> = Vec::new();
    let mut keychain_imports: Vec<KeychainImport> = Vec::new();

    let mut tree = match input.config_yaml {
        Some(text) => parse_yaml(text, "legacy config.yaml")?,
        None => Value::Mapping(Default::default()),
    };
    if let Some(text) = input.user_setting_yaml {
        let user = parse_yaml(text, "legacy user_setting.yaml")?;
        merge_yaml(&mut tree, user);
    }

    let mut config = Config::default();

    // ---------- general ----------
    if let Some(lang) = take_str(&mut tree, "prompts.language") {
        config.general.locale = match lang.as_str() {
            "zh" | "zh-CN" => "zh-CN",
            "en" | "en-US" => "en-US",
            other => {
                warnings.push(ConfigWarning {
                    path: "prompts.language".to_string(),
                    message: format!("未知的语言 `{other}`，已回退为 zh-CN。"),
                });
                "zh-CN"
            }
        }
        .to_string();
    }

    // ---------- capture ----------
    if let Some(v) = take_bool(&mut tree, "capture.screenshot.enabled") {
        config.capture.enabled = v;
    }
    if let Some(v) = take_u64(&mut tree, "capture.screenshot.capture_interval") {
        config.capture.interval_secs = v;
    }
    // 截图区域在配置里是 `{left, top, width, height}`，内部统一成
    // `[left, top, right, bottom]`
    if let Some(region) = take(&mut tree, "capture.screenshot.screenshot_region") {
        let mapping = region.as_mapping();
        let number = |key: &str| {
            mapping
                .and_then(|map| map.get(Value::String(key.to_string())))
                .and_then(Value::as_i64)
        };
        match (
            number("left"),
            number("top"),
            number("width"),
            number("height"),
        ) {
            (Some(left), Some(top), Some(width), Some(height)) if width > 0 && height > 0 => {
                config.capture.region = Some([left, top, left + width, top + height]);
            }
            _ => warnings.push(ConfigWarning {
                path: "capture.screenshot.screenshot_region".to_string(),
                message: "截图区域缺少 left/top/width/height 或宽高非正，已忽略。".to_string(),
            }),
        }
    }

    if let Some(v) = take_str(&mut tree, "capture.screenshot.storage_path") {
        // 旧路径含 ${CONTEXT_PATH:.} 占位符时不可用，交给新的默认数据目录
        if v.contains("${") {
            warnings.push(ConfigWarning {
                path: "capture.screenshot.storage_path".to_string(),
                message: "旧的截图目录使用了环境变量占位符，已改用新版默认数据目录。".to_string(),
            });
        } else {
            config.storage.blob_dir = Some(v);
        }
    }

    // ---------- vision ----------
    if let Some(v) = take_str(&mut tree, "vlm_model.base_url") {
        if !v.contains("${") {
            config.ai.vision.base_url = v;
        }
    }
    if let Some(v) = take_str(&mut tree, "vlm_model.model") {
        if !v.contains("${") {
            config.ai.vision.model = v;
        }
    }
    if let Some(provider) = take_str(&mut tree, "vlm_model.provider") {
        config.ai.vision.provider = crate::model::ProviderKind::OpenAiCompatible;
        note_provider_normalization(&mut warnings, "vlm_model.provider", &provider);
    }
    import_legacy_secret(
        &mut tree,
        "vlm_model.api_key",
        "provider:vision",
        &mut config.ai.vision.api_key_ref,
        &mut keychain_imports,
        &mut warnings,
    );

    // ---------- embedding ----------
    if let Some(v) = take_str(&mut tree, "embedding_model.base_url") {
        if !v.contains("${") {
            config.ai.embedding.base_url = v;
        }
    }
    if let Some(v) = take_str(&mut tree, "embedding_model.model") {
        if !v.contains("${") {
            config.ai.embedding.model = v;
        }
    }
    if let Some(provider) = take_str(&mut tree, "embedding_model.provider") {
        config.ai.embedding.provider = crate::model::ProviderKind::OpenAiCompatible;
        note_provider_normalization(&mut warnings, "embedding_model.provider", &provider);
    }
    if let Some(dim) = take_u64(&mut tree, "embedding_model.output_dim") {
        // 仅作参考；真实维度一律以 Provider 返回为准
        config.ai.embedding.dimensions = Some(dim as usize);
    }
    import_legacy_secret(
        &mut tree,
        "embedding_model.api_key",
        "provider:embedding",
        &mut config.ai.embedding.api_key_ref,
        &mut keychain_imports,
        &mut warnings,
    );

    // ---------- logging / server ----------
    if let Some(level) = take_str(&mut tree, "logging.level") {
        config.observability.log_level = level.to_ascii_lowercase();
    }
    if let Some(host) = take_str(&mut tree, "web.host") {
        config.server.host = host;
    }
    if let Some(port) = take_u64(&mut tree, "web.port") {
        config.server.port = port as u16;
    }

    // ---------- 剩下的一律报告 ----------
    let mut unmapped: Vec<ConfigWarning> = Vec::new();
    collect_unmapped(&tree, "", &mut unmapped);
    warnings.extend(unmapped);

    if config.general.timezone.is_none() {
        warnings.push(ConfigWarning {
            path: "general.timezone".to_string(),
            message: "旧配置中没有时区设置，新版将使用系统时区计算日边界；建议显式设置。"
                .to_string(),
        });
    }

    Ok(MigrationResult {
        config,
        warnings,
        keychain_imports,
    })
}

fn note_provider_normalization(warnings: &mut Vec<ConfigWarning>, path: &str, provider: &str) {
    if provider.is_empty() || provider == "openai_compatible" || provider == "openai" {
        return;
    }
    warnings.push(ConfigWarning {
        path: path.to_string(),
        message: format!(
            "旧配置中的 provider = \"{provider}\" 已归一化为 \"openai_compatible\"：\
             新版只通过标准 OpenAI 兼容接口调用模型，不再依赖任何厂商 SDK。"
        ),
    });
}

/// 把旧配置里的明文密钥转成 Keychain 引用。
///
/// 关键点：密钥进入 `keychain_imports` 交由调用方写入 Keychain，
/// **绝不写入新配置**。
fn import_legacy_secret(
    tree: &mut Value,
    path: &str,
    account: &str,
    api_key_ref: &mut Option<String>,
    imports: &mut Vec<KeychainImport>,
    warnings: &mut Vec<ConfigWarning>,
) {
    let Some(raw) = take_str(tree, path) else {
        return;
    };
    let secret = raw.trim().to_string();

    if secret.is_empty() {
        return;
    }
    if secret.contains("${") {
        warnings.push(ConfigWarning {
            path: path.to_string(),
            message: format!(
                "旧配置中的密钥是未解析的占位符 `{secret}`，未迁移；请在「设置 → 模型」中重新填写。"
            ),
        });
        return;
    }

    *api_key_ref = Some(format!("keychain:{account}"));
    imports.push(KeychainImport {
        account: account.to_string(),
        secret,
    });
}

fn parse_yaml(text: &str, source: &str) -> Result<Value, AppError> {
    let value: Value = serde_yaml::from_str(text).map_err(|e| {
        AppError::new(
            ErrorCode::ConfigInvalid,
            format!("{source}: 无法解析旧配置：{e}"),
        )
    })?;
    Ok(match value {
        Value::Null => Value::Mapping(Default::default()),
        other => other,
    })
}

fn merge_yaml(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Mapping(base_map), Value::Mapping(overlay_map)) => {
            for (key, value) in overlay_map {
                match base_map.get_mut(&key) {
                    Some(existing) => merge_yaml(existing, value),
                    None => {
                        base_map.insert(key, value);
                    }
                }
            }
        }
        (base_slot, overlay_value) => *base_slot = overlay_value,
    }
}

/// 取走一个叶子节点。取走后该路径不再出现在「未映射」报告里。
fn take(tree: &mut Value, path: &str) -> Option<Value> {
    let parts: Vec<&str> = path.split('.').collect();
    take_parts(tree, &parts)
}

fn take_parts(node: &mut Value, parts: &[&str]) -> Option<Value> {
    let map = node.as_mapping_mut()?;
    let key = Value::String(parts[0].to_string());
    if parts.len() == 1 {
        return map.remove(&key);
    }
    take_parts(map.get_mut(&key)?, &parts[1..])
}

fn take_str(tree: &mut Value, path: &str) -> Option<String> {
    match take(tree, path)? {
        Value::String(s) => Some(s),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn take_u64(tree: &mut Value, path: &str) -> Option<u64> {
    match take(tree, path)? {
        Value::Number(n) => n.as_u64().or_else(|| n.as_i64().map(|i| i as u64)),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn take_bool(tree: &mut Value, path: &str) -> Option<bool> {
    match take(tree, path)? {
        Value::Bool(b) => Some(b),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn collect_unmapped(value: &Value, prefix: &str, out: &mut Vec<ConfigWarning>) {
    match value {
        Value::Mapping(map) => {
            for (key, child) in map {
                let key = key
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| "<non-string-key>".to_string());
                let path = if prefix.is_empty() {
                    key
                } else {
                    format!("{prefix}.{key}")
                };
                collect_unmapped(child, &path, out);
            }
        }
        // 空节点是「已被取走」的痕迹，不是未映射字段
        Value::Null => {}
        other => {
            if out.len() >= UNMAPPED_WARNING_CAP {
                return;
            }
            let rendered = serde_yaml::to_string(other).unwrap_or_default();
            let message = if rendered.contains("${") {
                format!(
                    "旧配置项 `{prefix}` 使用了环境变量占位符，新版不再从该处读取；请在设置中直接填写。"
                )
            } else {
                format!("旧配置项 `{prefix}` 在新版中没有对应概念，未迁移。")
            };
            out.push(ConfigWarning {
                path: prefix.to_string(),
                message,
            });
        }
    }
}
