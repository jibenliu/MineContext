//! 密钥引用解析。
//!
//! 配置里**只有引用**，没有明文：
//! `api_key_ref = "keychain:provider:vision"` 或 `"env:MC_VISION_KEY"`。
//! 两种形式都支持，因为两种真实场景都存在：桌面用户把密钥放进钥匙串，
//! CI / 服务器用环境变量。
//!
//! **找不到密钥不是错误**：返回 `Ok(None)`，让上层降级成「规则 + 元数据也能用」
//! 而不是启动失败 —— 密钥缺失时让分析持续失败且用户看不出原因，是最难排查的形态。

use std::collections::HashMap;
use std::sync::Arc;

use mc_common::error::{AppError, ErrorCode};

/// 读取密钥的抽象。真实实现是 macOS 钥匙串，测试用具名实现。
pub trait SecretStore: Send + Sync {
    fn get(&self, account: &str) -> Result<Option<String>, AppError>;
}

/// 测试与嵌入式场景用：密钥直接放在内存里。
#[derive(Debug, Clone, Default)]
pub struct StaticSecretStore {
    secrets: HashMap<String, String>,
}

impl StaticSecretStore {
    pub fn new(secrets: HashMap<String, String>) -> Self {
        Self { secrets }
    }
}

impl SecretStore for StaticSecretStore {
    fn get(&self, account: &str) -> Result<Option<String>, AppError> {
        Ok(self.secrets.get(account).cloned())
    }
}

/// 解析一条密钥引用。
///
/// 支持 `keychain:<account>` 与 `env:<NAME>`；其它形式**直接报错**，
/// 不静默忽略 —— 静默忽略的结果是「配置看起来生效了，实际一次都没调用」。
pub fn resolve_secret(
    store: &dyn SecretStore,
    api_key_ref: Option<&str>,
) -> Result<Option<String>, AppError> {
    let Some(raw) = api_key_ref.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok(None);
    };

    if let Some(account) = raw.strip_prefix("keychain:") {
        let account = account.trim();
        if account.is_empty() {
            return Err(AppError::new(
                ErrorCode::ConfigInvalid,
                "api_key_ref 写成 `keychain:` 但没给账号名".to_string(),
            ));
        }
        return store.get(account);
    }

    if let Some(name) = raw.strip_prefix("env:") {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::new(
                ErrorCode::ConfigInvalid,
                "api_key_ref 写成 `env:` 但没给变量名".to_string(),
            ));
        }
        return Ok(std::env::var(name)
            .ok()
            .filter(|value| !value.trim().is_empty()));
    }

    Err(AppError::new(
        ErrorCode::ConfigInvalid,
        format!(
            "无法识别的 api_key_ref `{raw}`：只支持 `keychain:<账号>` 与 `env:<变量名>` 两种形式"
        ),
    ))
}

/// `security` 命令的输出。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

type Runner = Arc<dyn Fn(&[String]) -> Result<CommandOutput, AppError> + Send + Sync>;

/// 通过系统 `security` 命令读写 macOS 钥匙串。
///
/// 刻意**不链接 Security.framework**：Provider 层只依赖标准 HTTP 契约与
/// 命令行工具，不引入平台 SDK（Provider 纯净性要求）。
/// 副作用是 Linux/CI 上读不到钥匙串 —— 那种场景本来就应该用 `env:` 引用。
#[derive(Clone)]
pub struct KeychainCommand {
    service: String,
    runner: Runner,
}

impl KeychainCommand {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            runner: Arc::new(run_security_command),
        }
    }

    /// 注入命令执行器（测试用假实现，生产用真的 `security`）。
    pub fn with_runner(
        mut self,
        runner: impl Fn(&[String]) -> Result<CommandOutput, AppError> + Send + Sync + 'static,
    ) -> Self {
        self.runner = Arc::new(runner);
        self
    }

    pub fn service(&self) -> &str {
        &self.service
    }

    /// 读一条密钥。**条目不存在返回 `Ok(None)`** —— 这是正常情况
    /// （用户还没填过密钥），不是错误。
    pub fn get(&self, account: &str) -> Result<Option<String>, AppError> {
        let args = vec![
            "find-generic-password".to_string(),
            "-s".to_string(),
            self.service.clone(),
            "-a".to_string(),
            account.to_string(),
            "-w".to_string(),
        ];

        let output = (self.runner)(&args)?;
        if output.success {
            let secret = output.stdout.trim().to_string();
            return Ok((!secret.is_empty()).then_some(secret));
        }

        // 「找不到」是正常路径；其它失败（钥匙串被锁、拒绝授权）必须让用户看到
        let stderr = output.stderr.trim();
        if stderr.contains("could not be found") || stderr.contains("The specified item") {
            return Ok(None);
        }

        Err(AppError::new(
            ErrorCode::ConfigUnreadable,
            format!(
                "读取钥匙串失败（service={}, account={account}）：{stderr}",
                self.service
            ),
        ))
    }
}

impl SecretStore for KeychainCommand {
    fn get(&self, account: &str) -> Result<Option<String>, AppError> {
        KeychainCommand::get(self, account)
    }
}

/// 默认的钥匙串服务名。**改动会让所有用户已有的密钥失效**，不要动。
pub const KEYCHAIN_SERVICE: &str = "MineContext";

impl Default for KeychainCommand {
    fn default() -> Self {
        Self::new(KEYCHAIN_SERVICE)
    }
}

#[cfg(target_os = "macos")]
fn run_security_command(args: &[String]) -> Result<CommandOutput, AppError> {
    let output = std::process::Command::new("security")
        .args(args)
        .output()
        .map_err(|error| {
            AppError::new(
                ErrorCode::ConfigUnreadable,
                format!("无法执行 `security`（macOS 钥匙串命令行工具）：{error}"),
            )
        })?;

    Ok(CommandOutput {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

#[cfg(not(target_os = "macos"))]
fn run_security_command(_args: &[String]) -> Result<CommandOutput, AppError> {
    Err(AppError::new(
        ErrorCode::ConfigUnreadable,
        "当前平台没有 macOS 钥匙串；请改用 `env:<变量名>` 形式的 api_key_ref".to_string(),
    ))
}
