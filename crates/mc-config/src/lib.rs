//! `mc-config` — 配置分层加载、校验、热重载与旧配置迁移。
//!
//! 层次（低 → 高）：内置默认 < 系统配置 < 用户配置 < 环境变量。
//! 非法配置一律报错并指出字段路径，绝不静默忽略。

pub mod legacy;
pub mod load;
pub mod model;
pub mod write;

pub use load::{
    load, ConfigHandle, ConfigWarning, LayerSource, LoadRequest, LoadedConfig, ReloadOutcome,
};
pub use model::{ApplyToDays, Config, Mcp, McpServerConfig, McpTransportKind, ProviderKind};
