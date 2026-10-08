//! 旧数据导入（`mc-cli import legacy`）。
//!
//! 更早的数据布局与现在不同，用户升级后不能只迁移配置：
//! `<CONTEXT_PATH>/persist/sqlite/app.db` + `<CONTEXT_PATH>/screenshots/` →
//! `<data_dir>/data/minecontext.db` + `<data_dir>/blobs/`。
//! 四件事，每一件都对应一类会伤到用户的事故：
//! 1. **能导入的都要导入**（列按交集取，多出来的列不会让导入失败）；
//! 2. **重复执行不出重复行**（幂等靠主键冲突忽略）；3. **`--dry-run` 一个字节都不写**；
//! 4. **原始截图只当作文件保留**，不伪造成观测（它们没有可靠的窗口/时间元数据）。

use std::path::{Path, PathBuf};

use mc_common::time::Timestamp;
use mc_storage::import::{import_legacy, ImportOptions};
use rusqlite::Connection;

fn write_legacy_db(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        r#"
        CREATE TABLE vaults (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            title TEXT, summary TEXT, content TEXT, tags TEXT, parent_id INTEGER,
            is_folder BOOLEAN DEFAULT 0, is_deleted BOOLEAN DEFAULT 0,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            document_type TEXT DEFAULT 'vaults', sort_order INTEGER DEFAULT 0
        );
        CREATE TABLE todo (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            content TEXT, created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            start_time DATETIME DEFAULT CURRENT_TIMESTAMP, end_time DATETIME,
            status INTEGER DEFAULT 0, urgency INTEGER DEFAULT 0,
            assignee TEXT, reason TEXT
        );
        CREATE TABLE tips (id INTEGER PRIMARY KEY AUTOINCREMENT, content TEXT, created_at DATETIME);
        CREATE TABLE conversations (
            id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT, user_id TEXT,
            page_name VARCHAR(20) DEFAULT 'home', status VARCHAR(20) DEFAULT 'active',
            metadata JSON DEFAULT '{}', created_at DATETIME, updated_at DATETIME
        );
        CREATE TABLE messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT, conversation_id INTEGER NOT NULL,
            parent_message_id TEXT, role TEXT NOT NULL, content TEXT DEFAULT '',
            status TEXT NOT NULL DEFAULT 'pending', token_count INTEGER DEFAULT 0,
            metadata JSON DEFAULT '{}', latency_ms INTEGER DEFAULT 0,
            error_message TEXT DEFAULT '', completed_at DATETIME,
            created_at DATETIME, updated_at DATETIME
        );
        -- 旧库里有、新库没有的表：必须被报告为 skipped，而不是让导入崩掉
        CREATE TABLE monitoring_token_usage (id INTEGER PRIMARY KEY, model TEXT, total_tokens INTEGER);

        INSERT INTO vaults (title, content, document_type, created_at, updated_at)
            VALUES ('日报：9 月 30 日', '# 日报', 'DailyReport',
                    '2026-09-30 09:00:00', '2026-09-30 09:00:00');
        INSERT INTO todo (content, start_time, status, urgency)
            VALUES ('把导入脚本写完', '2026-09-30T09:00:00.000Z', 0, 2);
        INSERT INTO tips (content, created_at) VALUES ('记得喝水', '2026-09-30 09:05:00');
        INSERT INTO conversations (title, user_id, page_name, created_at, updated_at)
            VALUES ('我在做什么', 'local', 'home', '2026-09-30 09:10:00', '2026-09-30 09:10:00');
        INSERT INTO messages (conversation_id, role, content, status, created_at, updated_at)
            VALUES (1, 'user', '我今天上午做了什么？', 'completed',
                    '2026-09-30 09:10:00', '2026-09-30 09:10:00');
        INSERT INTO monitoring_token_usage (model, total_tokens) VALUES ('gpt-4o', 1234);
        "#,
    )
    .unwrap();
}

fn legacy_dir_with_screenshots() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write_legacy_db(&dir.path().join("persist/sqlite/app.db"));

    let shots = dir.path().join("screenshots");
    std::fs::create_dir_all(&shots).unwrap();
    std::fs::write(
        shots.join("screenshot_monitor_1_20260930_090000_000000.png"),
        b"fake-png-1",
    )
    .unwrap();
    std::fs::write(
        shots.join("screenshot_monitor_1_20260930_090015_000000.png"),
        b"fake-png-2",
    )
    .unwrap();

    dir
}

struct Harness {
    /// 两个临时目录都要**持有**：`TempDir` 一 drop，目录连同内容就被删了。
    _data: tempfile::TempDir,
    _legacy: tempfile::TempDir,
    legacy: PathBuf,
    data_dir: PathBuf,
}

fn harness() -> (Harness, mc_storage::Database) {
    let legacy = legacy_dir_with_screenshots();
    let data = tempfile::tempdir().unwrap();
    let data_dir = data.path().to_path_buf();
    let db = mc_storage::Database::open(data_dir.join("data/minecontext.db")).unwrap();

    let harness = Harness {
        legacy: legacy.path().to_path_buf(),
        data_dir,
        _data: data,
        _legacy: legacy,
    };
    (harness, db)
}

fn options(harness: &Harness, dry_run: bool) -> ImportOptions {
    ImportOptions {
        from: harness.legacy.clone(),
        data_dir: harness.data_dir.clone(),
        dry_run,
        now: Timestamp::from_millis(mc_testkit::fixtures::FIXTURE_EPOCH_MS),
    }
}

fn count(db: &mc_storage::Database, table: &str) -> i64 {
    db.with_read(|conn| conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)))
        .unwrap()
}

#[test]
fn imports_every_shared_table_and_reports_counts() {
    let (harness, db) = harness();

    let report = import_legacy(&db, &options(&harness, false)).expect("导入应当成功");

    assert_eq!(count(&db, "vaults"), 1);
    assert_eq!(count(&db, "todo"), 1);
    assert_eq!(count(&db, "tips"), 1);
    assert_eq!(count(&db, "conversations"), 1);
    assert_eq!(count(&db, "messages"), 1);

    let vaults = report.table("vaults").expect("报告里要有 vaults");
    assert_eq!(vaults.imported, 1);
    assert_eq!(report.total_imported(), 5);
    assert!(report.screenshots.copied == 2, "两张截图要复制过去");
    assert!(
        report
            .screenshots
            .destination
            .ends_with("imported/screenshots"),
        "截图落在 imported/screenshots：{:?}",
        report.screenshots.destination
    );
}

#[test]
fn second_run_imports_nothing_new() {
    let (harness, db) = harness();

    let first = import_legacy(&db, &options(&harness, false)).expect("第一次导入");
    assert_eq!(first.total_imported(), 5);

    let second = import_legacy(&db, &options(&harness, false)).expect("第二次导入");
    assert_eq!(second.total_imported(), 0, "重复执行不能再插一遍");
    assert_eq!(second.total_duplicates(), 5);
    assert_eq!(count(&db, "vaults"), 1);
    assert_eq!(count(&db, "messages"), 1);
}

#[test]
fn dry_run_writes_nothing() {
    let (harness, db) = harness();

    let report = import_legacy(&db, &options(&harness, true)).expect("dry-run 应当成功");

    assert_eq!(report.total_imported(), 5, "dry-run 也要如实报告会导入什么");
    assert_eq!(count(&db, "vaults"), 0, "dry-run 不能写库");
    assert_eq!(count(&db, "messages"), 0);
    assert!(
        !harness.data_dir.join("imported/screenshots").exists(),
        "dry-run 不能复制文件"
    );
}

#[test]
fn tables_missing_in_the_new_schema_are_reported_not_fatal() {
    let (harness, db) = harness();

    let report = import_legacy(&db, &options(&harness, false)).expect("导入应当成功");

    let skipped = report
        .tables
        .iter()
        .find(|t| t.table == "monitoring_token_usage")
        .expect("旧库里有、新库没有的表也要出现在报告里");
    assert!(
        skipped.skipped_reason.is_some(),
        "被跳过的表必须写明原因：{skipped:?}"
    );
}

#[test]
fn legacy_activity_json_survives_verbatim() {
    let (harness, db) = harness();
    // 旧库的 activity.metadata 是 JSON 字符串，导入必须逐字保留
    {
        let legacy = Connection::open(harness.legacy.join("persist/sqlite/app.db")).unwrap();
        legacy
            .execute_batch(
                r#"
                CREATE TABLE activity (
                    id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT, content TEXT,
                    resources JSON, metadata JSON, start_time DATETIME, end_time DATETIME
                );
                INSERT INTO activity (title, resources, metadata, start_time, end_time)
                VALUES ('写导入脚本', '[{"app":"Code"}]', '{"category":"开发"}',
                        '2026-09-30 09:00:00', '2026-09-30 09:30:00');
                "#,
            )
            .unwrap();
    }

    import_legacy(&db, &options(&harness, false)).expect("导入应当成功");

    let (resources, metadata): (String, String) = db
        .with_read(|conn| {
            conn.query_row(
                "SELECT resources, metadata FROM activity WHERE title = '写导入脚本'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
        })
        .unwrap();
    assert_eq!(resources, r#"[{"app":"Code"}]"#);
    assert_eq!(metadata, r#"{"category":"开发"}"#);
}

#[test]
fn legacy_screenshots_do_not_become_observations() {
    let (harness, db) = harness();

    import_legacy(&db, &options(&harness, false)).expect("导入应当成功");

    // 旧截图没有可靠的窗口/时间元数据：把它们塞进时间线等于编造历史
    assert_eq!(count(&db, "observations"), 0);
    assert_eq!(count(&db, "events"), 0);

    let copied: Vec<_> = std::fs::read_dir(harness.data_dir.join("imported/screenshots"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(copied.len(), 2, "文件本身要保留：{copied:?}");
}

#[test]
fn missing_legacy_database_lists_where_it_looked() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().to_path_buf();
    let db = mc_storage::Database::open(data_dir.join("data/minecontext.db")).unwrap();

    let error = import_legacy(
        &db,
        &ImportOptions {
            from: tmp.path().to_path_buf(),
            data_dir,
            dry_run: false,
            now: Timestamp::from_millis(mc_testkit::fixtures::FIXTURE_EPOCH_MS),
        },
    )
    .expect_err("找不到旧库必须报错");

    let detail = error.detail().to_string();
    assert!(
        detail.contains("persist/sqlite/app.db"),
        "错误里要写清找的是哪个路径，实际：{detail}"
    );
}

#[test]
fn token_is_accepted_as_a_direct_database_path() {
    // 有些用户把库放在别处（旧配置里 path 可改），因此 `--from` 也可以
    // 直接指向那个文件（此时截图目录按旧默认约定从它的父目录推断）。
    let legacy = legacy_dir_with_screenshots();
    let tmp = tempfile::tempdir().unwrap();
    let db = mc_storage::Database::open(tmp.path().join("data/minecontext.db")).unwrap();

    let report = import_legacy(
        &db,
        &ImportOptions {
            from: legacy.path().join("persist/sqlite/app.db"),
            data_dir: tmp.path().to_path_buf(),
            dry_run: false,
            now: Timestamp::from_millis(mc_testkit::fixtures::FIXTURE_EPOCH_MS),
        },
    )
    .expect("直接把库文件当 --from 也要能用");

    assert_eq!(report.total_imported(), 5);
}

#[test]
fn report_is_json_serializable_for_scripting() {
    // 迁移要能进 CI / 进用户的升级脚本，因此报告必须是稳定的 JSON。
    let (harness, db) = harness();
    let report = import_legacy(&db, &options(&harness, false)).expect("导入应当成功");

    let json = serde_json::to_value(&report).expect("报告要能序列化");
    assert!(json["tables"].is_array());
    assert!(json["screenshots"]["copied"].is_number());
    assert_eq!(json["dry_run"], serde_json::Value::Bool(false));
    assert!(json.get("token").is_none(), "报告里不能出现鉴权 token");
}
