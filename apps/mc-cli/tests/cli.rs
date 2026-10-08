//! `mc-cli` —— 迁移与诊断工具。
//!
//! 三个子命令对应阶段退出标准里的最后几项：
//! - `doctor`：一眼看清「哪个组件坏了、什么错误、该怎么办」
//! - `config validate`：迁移/手工改配置后先校验，避免启动才发现
//! - `config migrate`：把旧 YAML 配置转成新 TOML，**明文密钥不外泄**

use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};

use mc_cli::{parse_args, run, Command, RunOutcome};

const LEGACY_CONFIG: &str = include_str!("../../../fixtures/legacy/config.yaml");
const LEGACY_USER: &str = include_str!("../../../fixtures/legacy/user_setting.yaml");

const SECRET_VLM: &str = "sk-SECRET-VLM-KEY-0123456789abcdef";
const SECRET_EMB: &str = "sk-SECRET-EMB-KEY-0123456789abcdef";

fn write(dir: &Path, name: &str, content: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path
}

fn run_cli(cmd: Command) -> RunOutcome {
    run(cmd)
}

// ---------------------------------------------------------------- 参数解析

#[test]
fn parse_doctor_requires_data_dir_optionally() {
    let cmd = parse_args(&["doctor".to_string()]).unwrap();
    match cmd {
        Command::Doctor {
            config_path,
            data_dir,
        } => {
            assert_eq!(config_path, None);
            assert!(data_dir.ends_with("MineContext") || data_dir.ends_with(".minecontext"));
        }
        other => panic!("期望 Doctor，实际 {other:?}"),
    }
}

#[test]
fn parse_validate_requires_path() {
    assert!(parse_args(&["config".to_string(), "validate".to_string()]).is_err());
    let cmd = parse_args(&[
        "config".to_string(),
        "validate".to_string(),
        "/tmp/config.toml".to_string(),
    ])
    .unwrap();
    assert!(matches!(cmd, Command::ConfigValidate { .. }));
}

#[test]
fn parse_unknown_subcommand_is_rejected() {
    let err = parse_args(&["frobnicate".to_string()]).unwrap_err();
    assert!(err.contains("未知") || err.contains("用法"), "{err}");
}

#[test]
fn parse_no_args_shows_usage() {
    let err = parse_args(&[]).unwrap_err();
    assert!(err.contains("mc-cli"), "{err}");
}

#[test]
fn parse_migrate_reads_all_options() {
    let cmd = parse_args(&[
        "config".to_string(),
        "migrate".to_string(),
        "--config".to_string(),
        "/tmp/old.yaml".to_string(),
        "--user-setting".to_string(),
        "/tmp/user.yaml".to_string(),
        "--out".to_string(),
        "/tmp/new.toml".to_string(),
    ])
    .unwrap();

    match cmd {
        Command::ConfigMigrate {
            legacy_config,
            user_setting,
            out,
        } => {
            assert_eq!(legacy_config, PathBuf::from("/tmp/old.yaml"));
            assert_eq!(user_setting, Some(PathBuf::from("/tmp/user.yaml")));
            assert_eq!(out, PathBuf::from("/tmp/new.toml"));
        }
        other => panic!("期望 ConfigMigrate，实际 {other:?}"),
    }
}

// ---------------------------------------------------------------- config validate

#[test]
fn validate_accepts_good_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        dir.path(),
        "config.toml",
        "[general]\ntimezone = \"Asia/Shanghai\"\nlocale = \"zh-CN\"\n\n[capture]\ninterval_secs = 30\n",
    );

    let outcome = run_cli(Command::ConfigValidate {
        path,
        env: Vec::new(),
    });

    assert_eq!(outcome.exit_code, 0, "{}", outcome.stdout);
    assert!(outcome.stdout.contains("通过"), "{}", outcome.stdout);
}

#[test]
fn validate_reports_field_path_on_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        dir.path(),
        "config.toml",
        "[capture]\ninterval_secs = \"thirty\"\n",
    );

    let outcome = run_cli(Command::ConfigValidate {
        path,
        env: Vec::new(),
    });

    assert_eq!(outcome.exit_code, 1);
    assert!(
        outcome.stdout.contains("capture.interval_secs"),
        "必须指出字段路径：{}",
        outcome.stdout
    );
    // 用户可见文案与技术细节都要有
    assert!(
        outcome.stdout.contains("配置有误") || outcome.stdout.contains("配置"),
        "{}",
        outcome.stdout
    );
}

#[test]
fn validate_reports_warnings_but_still_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    // 没有 timezone → 应当有 warning，但配置合法
    let path = write(dir.path(), "config.toml", "[capture]\ninterval_secs = 30\n");

    let outcome = run_cli(Command::ConfigValidate {
        path,
        env: Vec::new(),
    });

    assert_eq!(outcome.exit_code, 0);
    assert!(
        outcome.stdout.contains("general.timezone"),
        "应当报出时区未设置的 warning：{}",
        outcome.stdout
    );
}

#[test]
fn validate_reports_missing_file_without_panicking() {
    let outcome = run_cli(Command::ConfigValidate {
        path: PathBuf::from("/nonexistent/minecontext/config.toml"),
        env: Vec::new(),
    });

    assert_eq!(outcome.exit_code, 1);
    assert!(outcome.stdout.contains("config.toml"), "{}", outcome.stdout);
}

// ---------------------------------------------------------------- config migrate

#[test]
fn migrate_writes_toml_and_never_leaks_plaintext_secret() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = write(dir.path(), "config.yaml", LEGACY_CONFIG);
    let user = write(dir.path(), "user_setting.yaml", LEGACY_USER);
    let out = dir.path().join("config.toml");

    let outcome = run_cli(Command::ConfigMigrate {
        legacy_config: legacy,
        user_setting: Some(user),
        out: out.clone(),
    });

    assert_eq!(outcome.exit_code, 0, "{}", outcome.stdout);
    assert!(out.exists(), "必须写出 config.toml");

    let toml_text = std::fs::read_to_string(&out).unwrap();
    assert!(
        !toml_text.contains(SECRET_VLM),
        "config.toml 泄漏了明文密钥"
    );
    assert!(
        !toml_text.contains(SECRET_EMB),
        "config.toml 泄漏了明文密钥"
    );
    assert!(toml_text.contains("keychain:provider:vision"));

    // stdout 也不得包含密钥
    assert!(
        !outcome.stdout.contains(SECRET_VLM),
        "stdout 泄漏了明文密钥"
    );
    assert!(
        !outcome.stdout.contains(SECRET_EMB),
        "stdout 泄漏了明文密钥"
    );
}

#[test]
fn migrate_writes_keychain_sidecar_with_0600() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = write(dir.path(), "config.yaml", LEGACY_CONFIG);
    let user = write(dir.path(), "user_setting.yaml", LEGACY_USER);
    let out = dir.path().join("config.toml");

    let outcome = run_cli(Command::ConfigMigrate {
        legacy_config: legacy,
        user_setting: Some(user),
        out: out.clone(),
    });
    assert_eq!(outcome.exit_code, 0, "{}", outcome.stdout);

    let sidecar = out.with_extension("keychain.json");
    assert!(
        sidecar.exists(),
        "应当写出待导入的密钥文件：{}",
        sidecar.display()
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&sidecar).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "密钥文件权限必须是 0600");
    }

    let text = std::fs::read_to_string(&sidecar).unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let accounts: Vec<String> = json
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["account"].as_str().unwrap().to_string())
        .collect();
    assert!(accounts.contains(&"provider:vision".to_string()));
    assert!(accounts.contains(&"provider:embedding".to_string()));

    // stdout 只告诉用户「有个文件要导入」，不打印密钥本身
    assert!(
        outcome.stdout.contains("keychain"),
        "应当提示密钥导入文件：{}",
        outcome.stdout
    );
}

#[test]
fn migrate_moves_unmapped_fields_to_warnings() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = write(dir.path(), "config.yaml", LEGACY_CONFIG);
    let out = dir.path().join("config.toml");

    let outcome = run_cli(Command::ConfigMigrate {
        legacy_config: legacy,
        user_setting: None,
        out,
    });

    assert_eq!(outcome.exit_code, 0, "{}", outcome.stdout);
    assert!(
        outcome.stdout.contains("storage.backends"),
        "未映射字段必须逐条报出：{}",
        outcome.stdout
    );
    assert!(
        outcome.stdout.contains("未迁移") || outcome.stdout.contains("没有对应概念"),
        "{}",
        outcome.stdout
    );
}

#[test]
fn migrate_reports_broken_yaml_without_writing_output() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = write(dir.path(), "config.yaml", "[capture\n  bad yaml: :\n");
    let out = dir.path().join("config.toml");

    let outcome = run_cli(Command::ConfigMigrate {
        legacy_config: legacy,
        user_setting: None,
        out: out.clone(),
    });

    assert_eq!(outcome.exit_code, 1);
    assert!(!out.exists(), "解析失败时不应写出半截配置");
}

// ---------------------------------------------------------------- doctor

#[test]
fn doctor_reports_ok_on_fresh_data_dir() {
    let dir = tempfile::tempdir().unwrap();

    let outcome = run_cli(Command::Doctor {
        config_path: None,
        data_dir: dir.path().to_path_buf(),
    });

    // 存储必须健康
    assert!(outcome.stdout.contains("storage"), "{}", outcome.stdout);
    assert!(outcome.stdout.contains("schema v"), "{}", outcome.stdout);

    // 未配置模型时必须如实说明，而不是报 healthy
    assert!(
        outcome.stdout.contains("unconfigured") || outcome.stdout.contains("未配置"),
        "{}",
        outcome.stdout
    );

    // 退出码取决于采集是否就绪：缺屏幕录制权限、或会话里没有显示器**确实**是个问题
    // （macOS 会静默返回全黑帧，核心功能不可用），因此此时应当非零。
    //
    // 判据取**这次报告自己的结论**（PROBLEM 字样），而不是在测试里另探一次采集能力：
    // 采集探测受会话状态与机器负载影响，同一进程前后两次探测都可能给出不同结论
    // （实测本机 load ＞ 25 时会随机变红）。这样断言同时还能抓住「报告说有问题却退了 0」。
    let reported_problem = outcome.stdout.contains("PROBLEM");
    assert_eq!(
        outcome.exit_code,
        if reported_problem { 1 } else { 0 },
        "退出码与报告结论不一致：\n{}",
        outcome.stdout
    );
}

#[test]
fn doctor_detects_corrupt_database_and_exits_nonzero() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("data")).unwrap();
    std::fs::write(
        dir.path().join("data/minecontext.db"),
        b"not a database at all, just garbage bytes",
    )
    .unwrap();

    let outcome = run_cli(Command::Doctor {
        config_path: None,
        data_dir: dir.path().to_path_buf(),
    });

    assert_eq!(
        outcome.exit_code, 1,
        "损坏的库必须导致非零退出：{}",
        outcome.stdout
    );
    assert!(
        outcome.stdout.contains("storage_corrupt"),
        "必须给出错误码：{}",
        outcome.stdout
    );
    assert!(
        outcome.stdout.contains("建议"),
        "必须给出可执行建议：{}",
        outcome.stdout
    );
}

#[test]
fn doctor_reports_config_error_but_still_runs() {
    let dir = tempfile::tempdir().unwrap();
    let bad = write(
        dir.path(),
        "config.toml",
        "[capture]\ninterval_secs = = 30\n",
    );

    let outcome = run_cli(Command::Doctor {
        config_path: Some(bad),
        data_dir: dir.path().to_path_buf(),
    });

    assert_eq!(outcome.exit_code, 1);
    assert!(outcome.stdout.contains("config"), "{}", outcome.stdout);
}

// ---------------------------------------------------------------- 真二进制 E2E

fn cli_binary() -> &'static str {
    env!("CARGO_BIN_EXE_mc-cli")
}

#[test]
fn binary_runs_doctor_and_reports_exit_code() {
    let dir = tempfile::tempdir().unwrap();

    let output = ProcessCommand::new(cli_binary())
        .arg("doctor")
        .arg("--data-dir")
        .arg(dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("mc-cli 必须能启动");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("storage"), "{stdout}");
    assert!(stdout.contains("capture"), "{stdout}");

    // 退出码必须与**子进程自己报告的结论**一致：报告里说有问题就退 1，没问题就退 0。
    //
    // 不要在测试进程里另探一次采集能力再比对：显示器数量与权限探测受会话状态和机器
    // 负载影响，两边结论可能不同 —— 那会让这条测试的通过与否取决于
    // 「跑它的时候机器在干什么」，而不是代码。
    let reported_problem = stdout.contains("PROBLEM");
    assert_eq!(
        output.status.code(),
        Some(if reported_problem { 1 } else { 0 }),
        "退出码与报告结论不一致：\n{stdout}"
    );
}

#[test]
fn binary_rejects_unknown_subcommand_with_exit_code_2() {
    let output = ProcessCommand::new(cli_binary())
        .arg("frobnicate")
        .output()
        .expect("mc-cli 必须能启动");

    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn binary_migrate_produces_loadable_config() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = write(dir.path(), "config.yaml", LEGACY_CONFIG);
    let user = write(dir.path(), "user_setting.yaml", LEGACY_USER);
    let out = dir.path().join("config.toml");

    let output = ProcessCommand::new(cli_binary())
        .args(["config", "migrate"])
        .arg("--config")
        .arg(&legacy)
        .arg("--user-setting")
        .arg(&user)
        .arg("--out")
        .arg(&out)
        .output()
        .expect("mc-cli 必须能启动");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );

    // 迁移产物必须能被 mc-config 正常加载（端到端闭环）
    let loaded = mc_config::load::load(&mc_config::load::LoadRequest {
        layers: vec![mc_config::load::LayerSource::File(out)],
        env: Vec::new(),
        read_process_env: false,
    })
    .expect("迁移产物必须可加载");

    assert_eq!(
        loaded.config.ai.vision.model,
        "doubao-seed-1-6-vision-250815"
    );
    assert_eq!(loaded.config.capture.interval_secs, 15);
}

// ---------------------------------------------------------------- 采集就绪度

/// `doctor` 必须报告采集权限 —— macOS 权限缺失时是**静默返回全黑帧**，
/// 不在 doctor 里显式暴露，用户就只会看到「一切正常但什么都没记录」。
#[test]
fn doctor_reports_capture_readiness() {
    let dir = tempfile::tempdir().unwrap();

    let outcome = run_cli(Command::Doctor {
        config_path: None,
        data_dir: dir.path().to_path_buf(),
    });

    let stdout = &outcome.stdout;
    assert!(
        stdout.contains("capture"),
        "doctor 必须报告采集状态：{stdout}"
    );

    // 未就绪时必须给出原因与建议（本机未授权时走这条分支；已授权时走 ok 分支）
    if stdout.contains("NOT READY") {
        assert!(
            stdout.contains("reason") || stdout.contains("建议"),
            "采集不可用时必须解释原因并给出建议：{stdout}"
        );
    } else {
        assert!(stdout.contains("permission="), "{stdout}");
        assert!(stdout.contains("monitors="), "{stdout}");
    }
}

/// 支持范围必须能在 `doctor` 里看到。
///
/// 用户装在不受支持的系统上时，需要看到「macOS 12.7 不受支持，最低 13.0」，
/// 而不是一个链接期就已经错配的二进制悄悄出问题。
#[test]
fn doctor_reports_the_supported_platform_range() {
    let dir = tempfile::tempdir().unwrap();
    let outcome = run_cli(Command::Doctor {
        config_path: None,
        data_dir: dir.path().to_path_buf(),
    });

    // 最低要求必须出现（"13.0"），并且有平台/系统一行的说明
    assert!(
        outcome.stdout.contains("13.0"),
        "doctor 必须写出最低支持的 macOS 版本：\n{}",
        outcome.stdout
    );
    assert!(
        outcome.stdout.contains("platform") || outcome.stdout.contains("系统"),
        "doctor 必须有平台段落：\n{}",
        outcome.stdout
    );
}

/// 采集状态（权限 + 显示器数）也要如实报告：这是「为什么没截图」的第一现场。
#[test]
fn doctor_reports_capture_readiness_in_detail() {
    let dir = tempfile::tempdir().unwrap();
    let outcome = run_cli(Command::Doctor {
        config_path: None,
        data_dir: dir.path().to_path_buf(),
    });

    assert!(
        outcome.stdout.contains("capture"),
        "doctor 必须有采集段落：\n{}",
        outcome.stdout
    );
    // 权限结论要么是就绪，要么给出可执行的原因
    assert!(
        outcome.stdout.contains("ready")
            || outcome.stdout.contains("permission")
            || outcome.stdout.contains("权限")
            || outcome.stdout.contains("显示器"),
        "采集段落要能解释「能不能采、为什么不能」：\n{}",
        outcome.stdout
    );
}
