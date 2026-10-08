//! 分层配置加载、校验与热重载。
//!
//! 层次（低 → 高）：内置默认 < 系统配置 < 用户配置 < 环境变量。
//!
//! 两条硬要求：
//! - **非法配置报错并指位置**，绝不静默忽略
//! - **热重载失败保留旧配置**，不因为一次手滑把生效配置清空

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use mc_common::error::{AppError, ErrorCode};
use serde::{Deserialize, Serialize};

use crate::model::Config;

/// 环境变量前缀：`MC_CONFIG__capture__interval_secs=30`
pub const ENV_PREFIX: &str = "MC_CONFIG__";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigWarning {
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoadedConfig {
    pub config: Config,
    /// 需要让用户知道、但不影响启动的问题
    pub warnings: Vec<ConfigWarning>,
    /// 实际参与合并的来源（按优先级从低到高）
    pub sources: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum LayerSource {
    /// 内联 TOML（测试与「保存设置」回读用）
    Inline {
        name: String,
        toml: String,
    },
    File(PathBuf),
}

impl LayerSource {
    fn read(&self) -> Result<(String, String), AppError> {
        match self {
            Self::Inline { name, toml } => Ok((name.clone(), toml.clone())),
            Self::File(path) => std::fs::read_to_string(path)
                .map(|text| (path.display().to_string(), text))
                .map_err(|e| {
                    AppError::new(
                        ErrorCode::ConfigUnreadable,
                        format!("{}: {e}", path.display()),
                    )
                }),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct LoadRequest {
    /// 按优先级从低到高排列
    pub layers: Vec<LayerSource>,
    /// 显式传入的环境变量（测试用）
    pub env: Vec<(String, String)>,
    /// 是否同时读取真实进程环境
    pub read_process_env: bool,
}

impl LoadRequest {
    /// 生产用法：系统配置 → 用户配置 → 进程环境。
    pub fn production(system: Option<PathBuf>, user: Option<PathBuf>) -> Self {
        let mut layers = Vec::new();
        if let Some(p) = system {
            layers.push(LayerSource::File(p));
        }
        if let Some(p) = user {
            layers.push(LayerSource::File(p));
        }
        Self {
            layers,
            env: Vec::new(),
            read_process_env: true,
        }
    }
}

pub fn load(req: &LoadRequest) -> Result<LoadedConfig, AppError> {
    let mut merged = toml::Value::try_from(Config::default()).map_err(|e| {
        AppError::new(
            ErrorCode::ConfigInvalid,
            format!("builtin defaults are not serializable: {e}"),
        )
    })?;

    let mut sources = Vec::new();

    for layer in &req.layers {
        let (name, text) = layer.read()?;
        let value: toml::Value = toml::from_str(&text)
            .map_err(|e| AppError::new(ErrorCode::ConfigInvalid, format!("{name}: {e}")))?;
        merge_value(&mut merged, value);
        sources.push(name);
    }

    let mut env_vars = req.env.clone();
    if req.read_process_env {
        env_vars.extend(std::env::vars().filter(|(k, _)| k.starts_with(ENV_PREFIX)));
    }
    if !env_vars.is_empty() {
        let overlay = env_overlay(&env_vars)?;
        merge_value(&mut merged, overlay);
        sources.push("environment".to_string());
    }

    let config = deserialize_with_path(&merged, &sources)?;
    validate(&config)?;

    let mut warnings = Vec::new();
    if config.general.timezone.is_none() {
        warnings.push(ConfigWarning {
            path: "general.timezone".to_string(),
            message: "未设置时区，将使用系统时区计算日边界；建议显式设置（例如 Asia/Shanghai）。"
                .to_string(),
        });
    }

    Ok(LoadedConfig {
        config,
        warnings,
        sources,
    })
}

/// 语义校验：类型正确但**取值不可能工作**的配置必须在这里拦住。
///
/// `serde` 只能保证类型，`deny_unknown_fields` 只能保证字段名；
/// 「批量上限为 0」这种配置能顺利反序列化，却会让索引永远失败。
/// 让错误出现在启动时（带字段路径），而不是运行到某一轮索引时才炸。
fn validate(config: &Config) -> Result<(), AppError> {
    if let Some(region) = config.capture.region {
        // 顺序写反的矩形会得到一张空图，必须在加载期就说清楚
        if region[2] <= region[0] || region[3] <= region[1] {
            return Err(AppError::new(
                ErrorCode::ConfigInvalid,
                format!(
                    "ai.capture.region 必须是 [left, top, right, bottom] 且 right>left、bottom>top，收到 {region:?}"
                ),
            )
            .with_context("path", "capture.region"));
        }
    }

    if config.ai.embedding.batch_limit == 0 {
        return Err(AppError::new(
            ErrorCode::ConfigInvalid,
            "ai.embedding.batch_limit 必须大于 0（0 会让向量索引无法提交任何批次）",
        )
        .with_context("path", "ai.embedding.batch_limit"));
    }
    Ok(())
}

/// 深合并：表递归合并，标量/数组整体覆盖。
pub fn merge_value(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base_table), toml::Value::Table(overlay_table)) => {
            for (key, value) in overlay_table {
                match base_table.get_mut(&key) {
                    Some(existing) => merge_value(existing, value),
                    None => {
                        base_table.insert(key, value);
                    }
                }
            }
        }
        (base_slot, overlay_value) => *base_slot = overlay_value,
    }
}

/// 把环境变量树应用到合并结果上。
fn env_overlay(vars: &[(String, String)]) -> Result<toml::Value, AppError> {
    let mut root = toml::Value::Table(toml::map::Map::new());
    for (key, raw) in vars {
        let path = key.strip_prefix(ENV_PREFIX).ok_or_else(|| {
            AppError::new(
                ErrorCode::ConfigInvalid,
                format!("environment key {key:?} must start with {ENV_PREFIX}"),
            )
        })?;
        let parts: Vec<&str> = path.split("__").filter(|s| !s.is_empty()).collect();
        if parts.is_empty() {
            return Err(AppError::new(
                ErrorCode::ConfigInvalid,
                format!("environment key {key:?} has an empty path"),
            ));
        }
        insert_path(&mut root, &parts, parse_env_value(raw))?;
    }
    Ok(root)
}

fn insert_path(node: &mut toml::Value, parts: &[&str], value: toml::Value) -> Result<(), AppError> {
    let table = node.as_table_mut().ok_or_else(|| {
        AppError::new(
            ErrorCode::ConfigInvalid,
            "environment override conflicts with a non-table value",
        )
    })?;

    if parts.len() == 1 {
        table.insert(parts[0].to_string(), value);
        return Ok(());
    }

    let child = table
        .entry(parts[0].to_string())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    insert_path(child, &parts[1..], value)
}

/// 环境变量的值按 TOML 字面量解析（`30` → 整数，`true` → 布尔，`"x"` → 字符串）；
/// 解析不出来才当字符串。这样 `MC_CONFIG__capture__interval_secs=30` 不会变成字符串。
fn parse_env_value(raw: &str) -> toml::Value {
    let probe = format!("v = {raw}");
    if let Ok(parsed) = toml::from_str::<toml::Value>(&probe) {
        if let Some(inner) = parsed.get("v") {
            return inner.clone();
        }
    }
    toml::Value::String(raw.to_string())
}

/// 反序列化时保留字段路径（`serde_path_to_error`）。
///
/// 没有这一层，用户只会看到 `invalid type: string, expected u64`，
/// 而不知道是哪个字段错了 —— 这正是要避免的「配置改了没生效」体验。
fn deserialize_with_path(value: &toml::Value, sources: &[String]) -> Result<Config, AppError> {
    let text = toml::to_string(value).map_err(|e| {
        AppError::new(
            ErrorCode::ConfigInvalid,
            format!("merged configuration is not serializable: {e}"),
        )
    })?;

    let deserializer = toml::de::Deserializer::new(&text);
    serde_path_to_error::deserialize::<_, Config>(deserializer).map_err(|e| {
        let path = e.path().to_string();
        let inner = e.into_inner();
        AppError::new(ErrorCode::ConfigInvalid, format!("{path}: {inner}")).with_context(
            "sources",
            if sources.is_empty() {
                "builtin defaults".to_string()
            } else {
                sources.join(", ")
            },
        )
    })
}

#[derive(Debug)]
pub enum ReloadOutcome {
    Applied(Arc<LoadedConfig>),
    Rejected(AppError),
}

/// 持有当前生效配置，支持原子替换。
///
/// 读者拿到的是 `Arc<LoadedConfig>` 快照，因此永远不会读到「半个新版本 +
/// 只加载了一半」的撕裂状态（对应测试 `reload_is_atomic_for_readers`）。
#[derive(Debug, Clone)]
pub struct ConfigHandle {
    inner: Arc<RwLock<Arc<LoadedConfig>>>,
}

impl ConfigHandle {
    pub fn new(initial: LoadedConfig) -> Self {
        Self {
            inner: Arc::new(RwLock::new(Arc::new(initial))),
        }
    }

    pub fn current(&self) -> Arc<LoadedConfig> {
        Arc::clone(&self.inner.read().expect("config lock poisoned"))
    }

    /// 重新加载。失败时**保留旧配置**并返回错误，调用方负责提示用户。
    pub fn reload(&self, req: &LoadRequest) -> ReloadOutcome {
        match load(req) {
            Ok(loaded) => {
                let arc = Arc::new(loaded);
                *self.inner.write().expect("config lock poisoned") = Arc::clone(&arc);
                ReloadOutcome::Applied(arc)
            }
            Err(error) => ReloadOutcome::Rejected(error),
        }
    }
}
