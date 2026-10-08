//! `mc-cli import legacy` —— 旧数据导入的命令行入口。
//!
//! 库层的语义由 `mc-storage` 的 `legacy_import.rs` 钉住；这里只保证
//! 命令行这一层：参数解析的报错要说清楚、人类可读的输出能核对、
//! `--json` 能被脚本消费。

use std::path::{Path, PathBuf};

use mc_cli::{parse_args, run, Command, RunOutcome};
use rusqlite::Connection;

fn write_legacy(dir: &Path) -> PathBuf {
    let db_path = dir.join("persist/sqlite/app.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        r#"
        CREATE TABLE vaults (
            id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT, content TEXT,
            document_type TEXT DEFAULT 'vaults', created_at DATETIME, updated_at DATETIME
        );
        CREATE TABLE todo (
            id INTEGER PRIMARY KEY AUTOINCREMENT, content TEXT,
            created_at DATETIME, start_time DATETIME, end_time DATETIME,
            status INTEGER DEFAULT 0, urgency INTEGER DEFAULT 0
        );
        INSERT INTO vaults (title, content, created_at, updated_at)
            VALUES ('日报：9 月 30 日', '# 日报', '2026-09-30 09:00:00', '2026-09-30 09:00:00');
        INSERT INTO todo (content, start_time, status, urgency)
            VALUES ('把导入脚本写完', '2026-09-30T09:00:00.000Z', 0, 2);
        "#,
    )
    .unwrap();

    let shots = dir.join("screenshots");
    std::fs::create_dir_all(&shots).unwrap();
    std::fs::write(
        shots.join("screenshot_monitor_1_20260930_090000_000000.png"),
        b"x",
    )
    .unwrap();
    db_path
}

fn data_dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn import_command(from: PathBuf, data_dir: PathBuf, dry_run: bool, json: bool) -> Command {
    Command::ImportLegacy {
        from,
        data_dir,
        dry_run,
        json,
    }
}

fn count_rows(data_dir: &Path, table: &str) -> i64 {
    let db = mc_storage::Database::open(data_dir.join("data/minecontext.db")).unwrap();
    db.with_read(|conn| conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)))
        .unwrap()
}

// ---------------------------------------------------------------- 参数解析

#[test]
fn from_is_required() {
    let error = parse_args(&["import".into(), "legacy".into()]).expect_err("缺 --from 必须报错");
    assert!(error.contains("--from"), "报错要指出缺哪个参数：{error}");
    assert!(error.contains("mc-cli"), "报错要带上用法：{error}");
}

#[test]
fn dry_run_and_json_flags_are_parsed() {
    let command = parse_args(&[
        "import".into(),
        "legacy".into(),
        "--from".into(),
        "/tmp/legacy".into(),
        "--data-dir".into(),
        "/tmp/data".into(),
        "--dry-run".into(),
        "--json".into(),
    ])
    .expect("参数应当合法");

    assert_eq!(
        command,
        Command::ImportLegacy {
            from: PathBuf::from("/tmp/legacy"),
            data_dir: PathBuf::from("/tmp/data"),
            dry_run: true,
            json: true,
        }
    );
}

#[test]
fn unknown_flags_are_rejected() {
    let error = parse_args(&[
        "import".into(),
        "legacy".into(),
        "--from".into(),
        "/tmp/legacy".into(),
        "--force".into(),
    ])
    .expect_err("未知参数必须报错");
    assert!(error.contains("--force"), "{error}");
}

#[test]
fn unknown_subcommand_is_rejected() {
    let error = parse_args(&["import".into(), "everything".into()]).expect_err("必须报错");
    assert!(error.contains("everything"), "{error}");
}

// ---------------------------------------------------------------- 端到端

#[test]
fn imports_and_prints_a_checkable_summary() {
    let legacy = tempfile::tempdir().unwrap();
    write_legacy(legacy.path());
    let data = data_dir();

    let outcome = run(import_command(
        legacy.path().to_path_buf(),
        data.path().to_path_buf(),
        false,
        false,
    ));

    assert_eq!(outcome.exit_code, 0, "{}", outcome.stdout);
    assert!(outcome.stdout.contains("vaults"), "{}", outcome.stdout);
    assert!(outcome.stdout.contains("导入 1 行"), "{}", outcome.stdout);
    assert!(outcome.stdout.contains("截图：1 张"), "{}", outcome.stdout);
    assert!(
        outcome.stdout.contains("不会变成观测"),
        "要解释旧截图的处理方式：{}",
        outcome.stdout
    );
    assert_eq!(count_rows(data.path(), "vaults"), 1);
    assert_eq!(count_rows(data.path(), "todo"), 1);
}

#[test]
fn dry_run_says_so_and_writes_nothing() {
    let legacy = tempfile::tempdir().unwrap();
    write_legacy(legacy.path());
    let data = data_dir();

    let outcome = run(import_command(
        legacy.path().to_path_buf(),
        data.path().to_path_buf(),
        true,
        false,
    ));

    assert_eq!(outcome.exit_code, 0, "{}", outcome.stdout);
    assert!(outcome.stdout.contains("dry-run"), "{}", outcome.stdout);
    assert_eq!(count_rows(data.path(), "vaults"), 0, "dry-run 不能写库");
    assert!(!data.path().join("imported/screenshots").exists());
}

#[test]
fn json_output_is_machine_readable() {
    let legacy = tempfile::tempdir().unwrap();
    write_legacy(legacy.path());
    let data = data_dir();

    let outcome = run(import_command(
        legacy.path().to_path_buf(),
        data.path().to_path_buf(),
        false,
        true,
    ));

    assert_eq!(outcome.exit_code, 0, "{}", outcome.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&outcome.stdout).expect("stdout 要是 JSON");
    assert_eq!(parsed["dry_run"], serde_json::Value::Bool(false));
    assert!(parsed["tables"].is_array());
    assert_eq!(parsed["screenshots"]["copied"], serde_json::Value::from(1));
}

#[test]
fn missing_source_fails_with_the_path_it_looked_for() {
    let data = data_dir();

    let outcome: RunOutcome = run(import_command(
        data.path().join("nope"),
        data.path().to_path_buf(),
        false,
        false,
    ));

    assert_eq!(outcome.exit_code, 1);
    assert!(
        outcome.stdout.contains("persist/sqlite/app.db"),
        "报错要写清找过哪里：{}",
        outcome.stdout
    );
    assert!(
        outcome.stdout.contains("storage_legacy_source_missing"),
        "要带上稳定错误码，便于排查：{}",
        outcome.stdout
    );
}
