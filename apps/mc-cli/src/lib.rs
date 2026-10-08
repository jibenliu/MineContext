//! `mc-cli` —— 迁移与诊断工具。
//!
//! - `doctor`：一眼看清「哪个组件坏了、什么错误、该怎么办」
//! - `config validate`：迁移或手工改配置后先校验，不要等启动才发现
//! - `config migrate`：旧 YAML → 新 TOML，**明文密钥不外泄**
//!
//! 业务逻辑放在 lib 里，`main` 只负责把结果映射到 stdout/stderr 与退出码，
//! 这样所有子命令都能被集成测试直接调用。

use std::path::PathBuf;

use mc_common::time::{Clock, SystemClock};
use mc_config::{legacy::LegacyInput, LayerSource, LoadRequest};
use mc_domain::activity::AggregationPolicy;
use mc_domain::projector::ProjectionOptions;
use mc_domain::rules::RuleSet;

pub const HELP: &str = "\
mc-cli — MineContext 迁移与诊断工具

用法:
  mc-cli doctor [--config <path>] [--data-dir <path>]
  mc-cli config validate <path> [--env KEY=VALUE]...
  mc-cli config migrate --config <legacy.yaml> [--user-setting <yaml>] --out <config.toml>
  mc-cli import legacy --from <旧数据目录> [--data-dir <path>] [--dry-run] [--json]
  mc-cli replay [--projector activities] [--rules <yaml>] [--data-dir <path>]

子命令:
  doctor            检查数据目录、数据库、配置与模型配置
  config validate   校验一个 TOML 配置层
  config migrate    把旧版 config.yaml / user_setting.yaml 迁移为新版 config.toml
  import legacy     把旧版数据（SQLite 库 + 截图目录）导入新版数据目录
  replay            用事件日志重建派生数据（算法/规则升级后重算历史）

选项:
  -h, --help        显示帮助
";

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Doctor {
        config_path: Option<PathBuf>,
        data_dir: PathBuf,
    },
    ConfigValidate {
        path: PathBuf,
        env: Vec<(String, String)>,
    },
    ConfigMigrate {
        legacy_config: PathBuf,
        user_setting: Option<PathBuf>,
        out: PathBuf,
    },
    ImportLegacy {
        from: PathBuf,
        data_dir: PathBuf,
        dry_run: bool,
        json: bool,
    },
    Replay {
        projector: String,
        data_dir: PathBuf,
        rules: Option<PathBuf>,
    },
}

impl Command {
    /// 命令名。只用于日志字段，因此保持稳定、短、无空格。
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Doctor { .. } => "doctor",
            Self::ConfigValidate { .. } => "config_validate",
            Self::ConfigMigrate { .. } => "config_migrate",
            Self::ImportLegacy { .. } => "import_legacy",
            Self::Replay { .. } => "replay",
        }
    }
}

/// 目前支持的投影器。刻意做成常量而不是散落在 match 里：
/// 报错信息与帮助要能自动跟上。
pub const SUPPORTED_PROJECTORS: &[&str] = &["activities"];

#[derive(Debug, Clone, PartialEq)]
pub struct RunOutcome {
    pub exit_code: u8,
    pub stdout: String,
    pub stderr: String,
}

impl RunOutcome {
    fn ok(stdout: String) -> Self {
        Self {
            exit_code: 0,
            stdout,
            stderr: String::new(),
        }
    }

    fn failed(stdout: String) -> Self {
        Self {
            exit_code: 1,
            stdout,
            stderr: String::new(),
        }
    }
}

pub fn parse_args(raw: &[String]) -> Result<Command, String> {
    if raw.is_empty() {
        return Err(HELP.to_string());
    }
    if matches!(raw[0].as_str(), "-h" | "--help" | "help") {
        return Err(HELP.to_string());
    }

    match raw[0].as_str() {
        "doctor" => parse_doctor(&raw[1..]),
        "replay" => parse_replay(&raw[1..]),
        "config" => {
            let sub = raw
                .get(1)
                .ok_or_else(|| format!("config 需要子命令（validate | migrate）\n\n{HELP}"))?;
            match sub.as_str() {
                "validate" => parse_validate(&raw[2..]),
                "migrate" => parse_migrate(&raw[2..]),
                other => Err(format!("未知的 config 子命令 {other}\n\n{HELP}")),
            }
        }
        "import" => {
            let sub = raw
                .get(1)
                .ok_or_else(|| format!("import 需要子命令（legacy）\n\n{HELP}"))?;
            match sub.as_str() {
                "legacy" => parse_import_legacy(&raw[2..]),
                other => Err(format!("未知的 import 子命令 {other}\n\n{HELP}")),
            }
        }
        other => Err(format!("未知子命令 {other}\n\n{HELP}")),
    }
}

fn parse_doctor(raw: &[String]) -> Result<Command, String> {
    let mut config_path = None;
    let mut data_dir = None;

    let mut iter = raw.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--config" => config_path = Some(PathBuf::from(need(iter.next(), "--config")?)),
            "--data-dir" => data_dir = Some(PathBuf::from(need(iter.next(), "--data-dir")?)),
            other => return Err(format!("doctor 收到未知参数 {other}\n\n{HELP}")),
        }
    }

    Ok(Command::Doctor {
        config_path,
        data_dir: data_dir.unwrap_or_else(default_data_dir),
    })
}

fn parse_validate(raw: &[String]) -> Result<Command, String> {
    let path = raw
        .first()
        .ok_or_else(|| format!("config validate 需要配置文件路径\n\n{HELP}"))?;
    let mut path = PathBuf::from(path);
    let mut env = Vec::new();

    let mut iter = raw[1..].iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--env" => {
                let pair = need(iter.next(), "--env")?;
                let (key, value) = pair
                    .split_once('=')
                    .ok_or_else(|| format!("--env 需要 KEY=VALUE 形式，收到 {pair}"))?;
                env.push((key.to_string(), value.to_string()));
            }
            // 允许 `config validate --env K=V <path>` 的写法
            other if other.starts_with("--") => {
                return Err(format!("config validate 收到未知参数 {other}"))
            }
            other => path = PathBuf::from(other),
        }
    }

    Ok(Command::ConfigValidate { path, env })
}

fn parse_migrate(raw: &[String]) -> Result<Command, String> {
    let mut legacy_config = None;
    let mut user_setting = None;
    let mut out = None;

    let mut iter = raw.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--config" => legacy_config = Some(PathBuf::from(need(iter.next(), "--config")?)),
            "--user-setting" => {
                user_setting = Some(PathBuf::from(need(iter.next(), "--user-setting")?))
            }
            "--out" => out = Some(PathBuf::from(need(iter.next(), "--out")?)),
            other => return Err(format!("config migrate 收到未知参数 {other}\n\n{HELP}")),
        }
    }

    Ok(Command::ConfigMigrate {
        legacy_config: legacy_config
            .ok_or_else(|| format!("config migrate 需要 --config <legacy.yaml>\n\n{HELP}"))?,
        user_setting,
        out: out.ok_or_else(|| format!("config migrate 需要 --out <config.toml>\n\n{HELP}"))?,
    })
}

fn parse_import_legacy(raw: &[String]) -> Result<Command, String> {
    let mut from = None;
    let mut data_dir = None;
    let mut dry_run = false;
    let mut json = false;

    let mut iter = raw.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--from" => from = Some(PathBuf::from(need(iter.next(), "--from")?)),
            "--data-dir" => data_dir = Some(PathBuf::from(need(iter.next(), "--data-dir")?)),
            "--dry-run" => dry_run = true,
            "--json" => json = true,
            other => return Err(format!("import legacy 收到未知参数 {other}\n\n{HELP}")),
        }
    }

    Ok(Command::ImportLegacy {
        from: from.ok_or_else(|| format!("import legacy 需要 --from <旧数据目录>\n\n{HELP}"))?,
        data_dir: data_dir.unwrap_or_else(default_data_dir),
        dry_run,
        json,
    })
}

fn parse_replay(raw: &[String]) -> Result<Command, String> {
    let mut projector = SUPPORTED_PROJECTORS[0].to_string();
    let mut data_dir = None;
    let mut rules = None;

    let mut iter = raw.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--projector" => projector = need(iter.next(), "--projector")?.to_string(),
            "--data-dir" => data_dir = Some(PathBuf::from(need(iter.next(), "--data-dir")?)),
            "--rules" => rules = Some(PathBuf::from(need(iter.next(), "--rules")?)),
            other => return Err(format!("replay 收到未知参数 {other}\n\n{HELP}")),
        }
    }

    Ok(Command::Replay {
        projector,
        data_dir: data_dir.unwrap_or_else(default_data_dir),
        rules,
    })
}

fn need<'a>(value: Option<&'a String>, flag: &str) -> Result<&'a str, String> {
    value
        .map(String::as_str)
        .ok_or_else(|| format!("{flag} 需要一个参数"))
}

/// 与 `mc-daemon` 保持一致的默认数据目录。
pub fn default_data_dir() -> PathBuf {
    if let Ok(explicit) = std::env::var("MC_DATA_DIR") {
        return PathBuf::from(explicit);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    #[cfg(target_os = "macos")]
    {
        PathBuf::from(home).join("Library/Application Support/MineContext")
    }
    #[cfg(not(target_os = "macos"))]
    {
        PathBuf::from(home).join(".minecontext")
    }
}

pub fn run(command: Command) -> RunOutcome {
    // 日志装不上也不影响命令本身：CLI 的价值在于它输出的那几行字。
    let guard =
        mc_common::observability::init(mc_common::observability::LogOptions::for_cli()).ok();
    let name = command.name();
    mc_common::observability::debug!(
        component = "cli",
        event = "command_started",
        command = name,
        "开始执行"
    );

    let outcome = match command {
        Command::Doctor {
            config_path,
            data_dir,
        } => run_doctor(config_path, data_dir),
        Command::ConfigValidate { path, env } => run_validate(path, env),
        Command::ConfigMigrate {
            legacy_config,
            user_setting,
            out,
        } => run_migrate(legacy_config, user_setting, out),
        Command::ImportLegacy {
            from,
            data_dir,
            dry_run,
            json,
        } => run_import_legacy(from, data_dir, dry_run, json),
        Command::Replay {
            projector,
            data_dir,
            rules,
        } => run_replay(projector, data_dir, rules),
    };

    if outcome.exit_code == 0 {
        mc_common::observability::debug!(
            component = "cli",
            event = "command_finished",
            command = name,
            "执行结束"
        );
    } else {
        mc_common::observability::error!(
            component = "cli",
            event = "command_failed",
            command = name,
            exit_code = outcome.exit_code,
            "命令失败"
        );
    }
    if let Some(guard) = guard {
        guard.flush();
    }
    outcome
}

// ---------------------------------------------------------------- import legacy

fn run_import_legacy(from: PathBuf, data_dir: PathBuf, dry_run: bool, json: bool) -> RunOutcome {
    let db_path = data_dir.join("data").join("minecontext.db");
    let db = match mc_storage::Database::open(&db_path) {
        Ok(db) => db,
        Err(error) => {
            return RunOutcome::failed(format!(
                "无法打开新版数据库 {}：{}\n",
                db_path.display(),
                error.detail()
            ))
        }
    };

    let options = mc_storage::import::ImportOptions {
        from,
        data_dir: data_dir.clone(),
        dry_run,
        now: SystemClock.now(),
    };

    let report = match mc_storage::import::import_legacy(&db, &options) {
        Ok(report) => report,
        Err(error) => {
            // 带上稳定错误码：用户报障时只要给这一行，
            // 排查文档（docs/troubleshooting.md）按码索引
            return RunOutcome::failed(format!(
                "导入失败 [{}]：{}\n",
                error.code().as_str(),
                error.detail()
            ));
        }
    };

    if json {
        match serde_json::to_string_pretty(&report) {
            Ok(text) => return RunOutcome::ok(format!("{text}\n")),
            Err(error) => return RunOutcome::failed(format!("报告无法序列化：{error}\n")),
        }
    }

    let mut out = String::new();
    out.push_str(&format!(
        "旧数据导入{}\n  源：{}\n\n",
        if dry_run {
            "（dry-run：不会写入任何数据）"
        } else {
            ""
        },
        report.source.display()
    ));

    for table in &report.tables {
        match &table.skipped_reason {
            Some(reason) => out.push_str(&format!("  {:<24} 跳过（{reason}）\n", table.table)),
            None => {
                out.push_str(&format!(
                    "  {:<24} 导入 {} 行，重复 {} 行",
                    table.table, table.imported, table.duplicates
                ));
                if !table.ignored_columns.is_empty() {
                    out.push_str(&format!("，忽略列：{}", table.ignored_columns.join(", ")));
                }
                out.push('\n');
            }
        }
    }

    out.push_str(&format!(
        "\n  截图：{} 张 → {}（已存在 {} 张）\n  说明：{}\n",
        report.screenshots.copied,
        report.screenshots.destination.display(),
        report.screenshots.already_present,
        report.screenshots.note
    ));
    out.push_str(&format!(
        "  合计：导入 {} 行，重复 {} 行\n",
        report.total_imported(),
        report.total_duplicates()
    ));

    RunOutcome::ok(out)
}

// ---------------------------------------------------------------- replay

/// 规则文件里的活动参数 → 聚合策略。
///
/// 重放必须用**和实时一样的参数**，否则重算结果与线上数据对不上，
/// 用户会以为「重放坏了」。
fn aggregation_policy(activity: &mc_config::model::Activity) -> AggregationPolicy {
    AggregationPolicy {
        debounce_secs: activity.debounce_secs,
        min_duration_secs: activity.min_duration_secs,
        merge_gap_secs: activity.merge_gap_secs,
        max_duration_secs: activity.max_duration_secs,
        idle_gap_secs: AggregationPolicy::default().idle_gap_secs,
    }
}

fn run_replay(projector: String, data_dir: PathBuf, rules: Option<PathBuf>) -> RunOutcome {
    let mut out = String::new();

    if projector != "activities" {
        return RunOutcome::failed(format!(
            "未知投影器 {projector}\n支持的投影器：{}\n",
            SUPPORTED_PROJECTORS.join(", ")
        ));
    }

    // 规则文件坏了要报错：静默回退到「无规则」会让用户以为自己写的规则生效了
    let rule_set = match &rules {
        Some(path) => match std::fs::read_to_string(path) {
            Ok(text) => match RuleSet::parse_yaml(&text) {
                Ok(set) => set,
                Err(message) => {
                    return RunOutcome::failed(format!(
                        "规则文件无法使用：{}\n  {}\n",
                        path.display(),
                        message
                    ))
                }
            },
            Err(error) => {
                return RunOutcome::failed(format!(
                    "无法读取规则文件 {}：{error}\n",
                    path.display()
                ))
            }
        },
        None => RuleSet::default(),
    };

    let request = LoadRequest {
        layers: Vec::new(),
        env: Vec::new(),
        read_process_env: true,
    };
    let config = match mc_config::load(&request) {
        Ok(loaded) => loaded.config,
        Err(error) => {
            return RunOutcome::failed(format!(
                "配置无法加载，重放中止（用错误的参数重算会污染派生数据）：{}\n",
                error.detail()
            ))
        }
    };

    let db_path = data_dir.join("data").join("minecontext.db");
    let db = match mc_storage::Database::open(&db_path) {
        Ok(db) => db,
        Err(error) => {
            return RunOutcome::failed(format!(
                "无法打开数据库 {}：{}\n",
                db_path.display(),
                error.detail()
            ))
        }
    };
    if db.is_read_only() {
        return RunOutcome::failed(format!(
            "数据库处于只读安全模式（{}），不能重放\n",
            db_path.display()
        ));
    }

    let options = ProjectionOptions {
        policy: aggregation_policy(&config.activity),
        allow_inference: false,
    };

    let at = SystemClock.now();
    let projection = match mc_storage::projectors::activities::replay(&db, &rule_set, options, at) {
        Ok(projection) => projection,
        Err(error) => {
            return RunOutcome::failed(format!(
                "重放失败：{}\n  code:   {}\n  detail: {}\n",
                error.user_message(),
                error.code().as_str(),
                error.detail()
            ))
        }
    };

    let events = db.event_count().unwrap_or(0);
    let checkpoint = mc_storage::projectors::activities::checkpoint(&db)
        .ok()
        .flatten()
        .unwrap_or(0);

    out.push_str(&format!("重放完成：projector={projector}\n"));
    out.push_str(&format!("  {:<12} {}\n", "events", events));
    out.push_str(&format!(
        "  {:<12} {}\n",
        "activities",
        projection.activities.len()
    ));
    out.push_str(&format!("  {:<12} {}\n", "noise", projection.noise.len()));
    out.push_str(&format!(
        "  {:<12} {}\n",
        "unknown", projection.unknown_event_kinds
    ));
    out.push_str(&format!("  {:<12} {}\n", "checkpoint", checkpoint));
    if rules.is_some() {
        out.push_str(&format!("  {:<12} {}\n", "rules", rule_set.len()));
    }

    RunOutcome::ok(out)
}

// ---------------------------------------------------------------- doctor

fn run_doctor(config_path: Option<PathBuf>, data_dir: PathBuf) -> RunOutcome {
    let mut out = String::new();
    let mut problems: Vec<String> = Vec::new();

    out.push_str("MineContext doctor\n");
    out.push_str(&format!(
        "  {:<12} {}\n",
        "version",
        env!("CARGO_PKG_VERSION")
    ));
    out.push_str(&format!("  {:<12} {}\n", "data dir", data_dir.display()));

    // 配置
    let request = LoadRequest {
        layers: config_path.iter().cloned().map(LayerSource::File).collect(),
        env: Vec::new(),
        read_process_env: true,
    };
    let loaded = match mc_config::load(&request) {
        Ok(loaded) => {
            if loaded.warnings.is_empty() {
                out.push_str(&format!(
                    "  {:<12} ok ({} 层)\n",
                    "config",
                    loaded.sources.len()
                ));
            } else {
                out.push_str(&format!(
                    "  {:<12} ok ({} 个提醒)\n",
                    "config",
                    loaded.warnings.len()
                ));
                for warning in &loaded.warnings {
                    out.push_str(&format!("    ! {:<24} {}\n", warning.path, warning.message));
                }
            }
            loaded
        }
        Err(error) => {
            out.push_str(&format!("  {:<12} FAILED\n", "config"));
            out.push_str(&format!("    {:<24} {}\n", "code", error.code().as_str()));
            out.push_str(&format!("    {:<24} {}\n", "detail", error.detail()));
            problems.push("config".to_string());
            // 配置坏了仍然继续检查存储，便于一次看清全部问题
            mc_config::load(&LoadRequest::default()).unwrap_or_else(|_| mc_config::LoadedConfig {
                config: mc_config::Config::default(),
                warnings: Vec::new(),
                sources: Vec::new(),
            })
        }
    };

    // 存储
    let db_path = data_dir.join("data").join("minecontext.db");
    let storage_ok = match mc_storage::Database::open(&db_path) {
        Ok(db) => {
            let migrations = db
                .applied_migrations()
                .map(|v| {
                    v.iter()
                        .map(|x| x.to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_else(|_| "?".to_string());
            out.push_str(&format!(
                "  {:<12} ok (schema v{}, read-only={}, {})\n",
                "storage",
                migrations,
                db.is_read_only(),
                db_path.display()
            ));
            true
        }
        Err(error) => {
            out.push_str(&format!("  {:<12} FAILED\n", "storage"));
            out.push_str(&format!("    {:<24} {}\n", "code", error.code().as_str()));
            out.push_str(&format!("    {:<24} {}\n", "detail", error.detail()));
            if let Some(remediation) = error.remediation() {
                out.push_str(&format!("    {:<24} {}\n", "建议", remediation.text));
            }
            // 损坏时验证安全模式可用
            match mc_storage::Database::open_safe_mode(&db_path) {
                Ok(_) => out.push_str(&format!(
                    "    {:<24} 可以只读安全模式打开（历史数据仍可查看/导出）\n",
                    "note"
                )),
                Err(safe_error) => out.push_str(&format!(
                    "    {:<24} 只读模式也不可用：{}\n",
                    "note",
                    safe_error.detail()
                )),
            }
            problems.push("storage".to_string());
            false
        }
    };

    // 模型配置
    let vision = &loaded.config.ai.vision;
    let configured = !vision.base_url.is_empty() && !vision.model.is_empty();
    out.push_str(&format!(
        "  {:<12} {}\n",
        "provider",
        if configured {
            format!(
                "ok (openai_compatible, model={}, concurrency={})",
                vision.model, vision.max_concurrency
            )
        } else {
            "unconfigured（未配置视觉模型，采集仍可用，AI 分析暂停）".to_string()
        }
    ));

    // 平台支持范围：装到不受支持的系统上要能立刻看出来，
    // 而不是让一个按错误平台构建的二进制悄悄出问题。
    match mc_common::platform::current() {
        Some(version) => {
            let report = mc_common::platform::support_report(version);
            out.push_str(&format!(
                "  {:<12} {}（最低 {}，{})\n",
                "platform",
                version,
                mc_common::platform::MIN_SUPPORTED_MACOS,
                if report.supported {
                    "supported"
                } else {
                    "unsupported"
                }
            ));
            if !report.supported {
                out.push_str(&format!("    ! {}\n", report.message));
                problems.push("platform".to_string());
            }
        }
        None => {
            out.push_str(&format!(
                "  {:<12} unknown（无法探测系统版本；最低要求 {})\n",
                "platform",
                mc_common::platform::MIN_SUPPORTED_MACOS
            ));
        }
    }

    // 采集：macOS 权限缺失时采集会**静默返回全黑帧**，doctor 必须直接告诉
    // 用户「缺什么、去哪里开」。
    let readiness = mc_capture::platform::probe_readiness();
    if readiness.available {
        out.push_str(&format!(
            "  {:<12} ok (permission={:?}, monitors={})\n",
            "capture", readiness.permission, readiness.monitor_count
        ));
    } else {
        out.push_str(&format!(
            "  {:<12} NOT READY (permission={:?}, monitors={})\n",
            "capture", readiness.permission, readiness.monitor_count
        ));
        if let Some(message) = &readiness.message {
            out.push_str(&format!("    {:<24} {}\n", "reason", message));
        }
        if readiness.permission == mc_capture::source::PermissionState::Denied {
            let error = mc_common::error::AppError::new(
                mc_common::error::ErrorCode::CapturePermissionDenied,
                "缺少屏幕录制权限",
            );
            if let Some(remediation) = error.remediation() {
                out.push_str(&format!("    {:<24} {}\n", "建议", remediation.text));
            }
        }
        problems.push("capture".to_string());
    }
    out.push_str(&format!(
        "  {:<12} {}\n",
        "result",
        if problems.is_empty() {
            "OK".to_string()
        } else {
            format!("PROBLEM: {}", problems.join(", "))
        }
    ));

    if problems.is_empty() && storage_ok {
        RunOutcome::ok(out)
    } else {
        RunOutcome::failed(out)
    }
}

// ---------------------------------------------------------------- validate

fn run_validate(path: PathBuf, env: Vec<(String, String)>) -> RunOutcome {
    let mut out = String::new();

    let request = LoadRequest {
        layers: vec![LayerSource::File(path.clone())],
        env,
        read_process_env: false,
    };

    match mc_config::load(&request) {
        Ok(loaded) => {
            out.push_str(&format!("配置校验通过：{}\n", path.display()));
            if loaded.warnings.is_empty() {
                out.push_str("  无提醒\n");
            } else {
                for warning in &loaded.warnings {
                    out.push_str(&format!("  ! {:<24} {}\n", warning.path, warning.message));
                }
            }
            RunOutcome::ok(out)
        }
        Err(error) => {
            out.push_str(&format!("配置校验失败：{}\n", path.display()));
            out.push_str(&format!("  {:<10} {}\n", "code", error.code().as_str()));
            out.push_str(&format!("  {:<10} {}\n", "message", error.user_message()));
            out.push_str(&format!("  {:<10} {}\n", "detail", error.detail()));
            if let Some(remediation) = error.remediation() {
                out.push_str(&format!("  {:<10} {}\n", "建议", remediation.text));
            }
            RunOutcome::failed(out)
        }
    }
}

// ---------------------------------------------------------------- migrate

fn run_migrate(
    legacy_config: PathBuf,
    user_setting: Option<PathBuf>,
    out_path: PathBuf,
) -> RunOutcome {
    let mut out = String::new();

    let config_text = match std::fs::read_to_string(&legacy_config) {
        Ok(text) => text,
        Err(e) => {
            return RunOutcome::failed(format!("无法读取旧配置 {}: {e}\n", legacy_config.display()))
        }
    };

    let user_text = match &user_setting {
        Some(path) => match std::fs::read_to_string(path) {
            Ok(text) => Some(text),
            Err(e) => return RunOutcome::failed(format!("无法读取 {}: {e}\n", path.display())),
        },
        None => None,
    };

    let result = match mc_config::legacy::migrate_legacy(&LegacyInput {
        config_yaml: Some(&config_text),
        user_setting_yaml: user_text.as_deref(),
    }) {
        Ok(result) => result,
        Err(error) => {
            return RunOutcome::failed(format!(
                "迁移失败：{}\n  code:   {}\n  detail: {}\n",
                error.user_message(),
                error.code().as_str(),
                error.detail()
            ))
        }
    };

    // 写配置（不含任何明文密钥）
    let toml_text = match toml::to_string_pretty(&result.config) {
        Ok(text) => text,
        Err(e) => return RunOutcome::failed(format!("无法序列化新配置：{e}\n")),
    };
    if let Err(e) = mc_common::fs::write_private_str(&out_path, &toml_text) {
        return RunOutcome::failed(format!("无法写入 {}: {e}\n", out_path.display()));
    }

    out.push_str(&format!(
        "已迁移配置：{} → {}\n",
        legacy_config.display(),
        out_path.display()
    ));
    out.push_str(&format!(
        "  {:<28} {}\n",
        "ai.vision.model", result.config.ai.vision.model
    ));
    out.push_str(&format!(
        "  {:<28} {}\n",
        "ai.embedding.model", result.config.ai.embedding.model
    ));
    out.push_str(&format!(
        "  {:<28} {}\n",
        "capture.interval_secs", result.config.capture.interval_secs
    ));

    // 未映射字段
    let unmapped: Vec<_> = result
        .warnings
        .iter()
        .filter(|w| w.path != "general.timezone")
        .collect();
    if !unmapped.is_empty() {
        // 全部列出：迁移的意义就是让用户知道「哪些设置没跟过来」，
        // 截断显示会让这条信息失去价值。
        out.push_str(&format!("\n未映射字段（{} 条）：\n", unmapped.len()));
        for warning in &unmapped {
            out.push_str(&format!("  - {:<40} {}\n", warning.path, warning.message));
        }
    }

    // 密钥：写 sidecar，绝不打印内容
    if !result.keychain_imports.is_empty() {
        let sidecar = out_path.with_extension("keychain.json");
        let payload = serde_json::to_string_pretty(&result.keychain_imports).unwrap_or_default();

        if let Err(e) = mc_common::fs::write_private_str(&sidecar, &payload) {
            return RunOutcome::failed(format!("无法写入密钥文件 {}: {e}\n", sidecar.display()));
        }

        out.push_str(&format!(
            "\n待导入 Keychain 的凭据（已写入 {}，权限 0600）：\n",
            sidecar.display()
        ));
        for item in &result.keychain_imports {
            out.push_str(&format!("  - {}\n", item.account));
        }
        out.push_str("  配置文件中只保存 keychain 引用，不含明文密钥。\n");
    }

    out.push_str("\n下一步：运行 `mc-cli doctor` 确认。\n");
    RunOutcome::ok(out)
}
