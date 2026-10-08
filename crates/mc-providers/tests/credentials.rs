//! 配置里的密钥引用解析。
//!
//! 教训是「Provider 配置写死了厂商细节」，
//! 而密钥这一环的对应要求是：**配置里永远不出现明文密钥**，
//! 只出现引用（`keychain:provider:vision` / `env:NAME`）。
//!
//! 这里测的是引用的解析与「找不到密钥」的语义：
//! 找不到**不是错误** —— 系统要降级到「不调模型也能用」，
//! 而不是启动失败（密钥缺失时时截图分析持续失败）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mc_common::error::ErrorCode;
use mc_providers::credentials::{
    resolve_secret, CommandOutput, KeychainCommand, StaticSecretStore,
};

fn store(pairs: &[(&str, &str)]) -> StaticSecretStore {
    StaticSecretStore::new(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<HashMap<_, _>>(),
    )
}

#[test]
fn keychain_reference_is_resolved_through_the_store() {
    let secrets = store(&[("provider:vision", "sk-from-keychain")]);

    let key = resolve_secret(&secrets, Some("keychain:provider:vision")).expect("解析不该失败");
    assert_eq!(key.as_deref(), Some("sk-from-keychain"));
}

#[test]
fn missing_secret_is_none_not_an_error() {
    let secrets = store(&[]);

    let key = resolve_secret(&secrets, Some("keychain:provider:vision"))
        .expect("找不到密钥不是错误：系统应当降级为「不调模型」而不是启动失败");
    assert_eq!(key, None);
}

#[test]
fn env_reference_reads_the_process_environment() {
    // 用测试专用的变量名，避免碰到真实环境
    std::env::set_var("MC_TEST_VISION_KEY", "sk-from-env");

    let secrets = store(&[]);
    let key = resolve_secret(&secrets, Some("env:MC_TEST_VISION_KEY")).expect("解析 env 引用");

    assert_eq!(key.as_deref(), Some("sk-from-env"));
    std::env::remove_var("MC_TEST_VISION_KEY");
}

#[test]
fn absent_reference_means_no_secret() {
    let secrets = store(&[]);
    assert_eq!(resolve_secret(&secrets, None).unwrap(), None);
    assert_eq!(resolve_secret(&secrets, Some("   ")).unwrap(), None);
}

#[test]
fn unknown_scheme_is_rejected_instead_of_silently_ignored() {
    let secrets = store(&[]);
    let error =
        resolve_secret(&secrets, Some("vault:secret/vision")).expect_err("未知引用形式必须报错");

    assert_eq!(error.code(), ErrorCode::ConfigInvalid);
    assert!(
        error.detail().contains("keychain") && error.detail().contains("env"),
        "报错要说清支持哪些形式：{}",
        error.detail()
    );
}

// macOS Keychain 通过 `security` 命令行读取：不链接 Security.framework，
// 也就没有平台 SDK 依赖（Provider 纯净性要求）。
#[test]
fn keychain_command_targets_the_documented_service_and_account() {
    let seen: Arc<Mutex<Vec<Vec<String>>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&seen);
    let command = KeychainCommand::new("MineContext").with_runner(move |args| {
        recorder.lock().unwrap().push(args.to_vec());
        Ok(CommandOutput {
            success: true,
            stdout: "sk-secret-from-macos\n".to_string(),
            stderr: String::new(),
        })
    });

    let secret = command.get("provider:vision").expect("读取 Keychain");
    assert_eq!(
        secret.as_deref(),
        Some("sk-secret-from-macos"),
        "输出要去掉换行"
    );

    assert_eq!(
        *seen.lock().unwrap(),
        vec![vec![
            "find-generic-password".to_string(),
            "-s".to_string(),
            "MineContext".to_string(),
            "-a".to_string(),
            "provider:vision".to_string(),
            "-w".to_string(),
        ]],
        "服务和账号的写法一旦变化，用户已有的密钥就读不到了"
    );
}

#[test]
fn missing_keychain_item_is_not_an_error() {
    let command = KeychainCommand::new("MineContext").with_runner(|_| {
        Ok(CommandOutput {
            success: false,
            stdout: String::new(),
            stderr: "security: SecKeychainSearchCopyNext: The specified item could not be found in the keychain.".to_string(),
        })
    });

    assert_eq!(
        command.get("provider:vision").expect("找不到条目不是错误"),
        None
    );
}

#[test]
fn keychain_failure_with_a_real_problem_is_reported() {
    let command = KeychainCommand::new("MineContext").with_runner(|_| {
        Ok(CommandOutput {
            success: false,
            stdout: String::new(),
            stderr: "security: User interaction is not allowed.".to_string(),
        })
    });

    let error = command
        .get("provider:vision")
        .expect_err("钥匙串被锁定这类问题必须让用户看到");
    assert_eq!(error.code(), ErrorCode::ConfigUnreadable);
    assert!(
        error.detail().contains("User interaction is not allowed"),
        "原始错误要保留：{}",
        error.detail()
    );
}
