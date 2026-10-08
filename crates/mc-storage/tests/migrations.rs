//! SQLite 迁移、事件存储与并发写。
//!
//! 这里验证的是存储层的地基：
//! 兼容层列形状与旧库一致、迁移可重复执行、事件 seq 单调、WAL 生效、
//! 损坏库进入只读安全模式、并发追加由单写者串行化。

use mc_common::error::ErrorCode;
use mc_storage::Database;

fn temp_db_path() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("minecontext.db");
    (dir, path)
}

fn table_columns(db: &Database, table: &str) -> Vec<String> {
    let mut cols: Vec<String> = db
        .with_read(|conn| {
            let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .expect("table_info must succeed");
    cols.sort();
    cols
}

fn scalar_string(db: &Database, sql: &str) -> String {
    db.with_read(|conn| {
        let value: rusqlite::types::Value = conn.query_row(sql, [], |row| row.get(0))?;
        Ok(match value {
            rusqlite::types::Value::Text(s) => s,
            rusqlite::types::Value::Integer(i) => i.to_string(),
            other => format!("{other:?}"),
        })
    })
    .expect("pragma query must succeed")
}

fn scalar_i64(db: &Database, sql: &str) -> i64 {
    db.with_read(|conn| conn.query_row(sql, [], |row| row.get::<_, i64>(0)))
        .expect("pragma query must succeed")
}

#[test]
fn migrations_apply_from_empty() {
    let (_dir, path) = temp_db_path();
    let db = Database::open(&path).expect("fresh database must open");

    for table in [
        // 兼容层
        "vaults",
        "todo",
        "tips",
        "activity",
        "conversations",
        "messages",
        "message_thinking",
        // 核心表
        "events",
        "projection_checkpoints",
        "observations",
        "analyses",
        "jobs",
        "adhoc_requests",
        "activities",
        "activity_observations",
        "stage_activities",
        "stages",
        "summaries",
        "user_overrides",
        "pipeline_failures",
        "provider_calls",
        "entities",
        "activity_entities",
        "config_snapshots",
        "schema_migrations",
    ] {
        assert!(!table_columns(&db, table).is_empty(), "缺少表 {table}");
    }

    assert_eq!(
        db.applied_migrations().unwrap(),
        mc_storage::migrate::defined_versions()
    );
}

// 0.15b —— 增量迁移：第二条迁移新增的列必须存在（验证迁移机制可追加）
#[test]
fn incremental_migration_adds_observation_image_columns() {
    let (_dir, path) = temp_db_path();
    let db = Database::open(&path).unwrap();

    let columns = table_columns(&db, "observations");
    for column in ["thumbnail_path", "image_bytes"] {
        assert!(
            columns.contains(&column.to_string()),
            "第二条迁移应当新增列 {column}，实际列：{columns:?}"
        );
    }
}

// 0.15c —— 迁移 0003：活动表补 original_title（用户改名后仍能回答「算法原本认为什么」）
#[test]
fn incremental_migration_adds_activity_original_title() {
    let (_dir, path) = temp_db_path();
    let db = Database::open(&path).unwrap();

    let columns = table_columns(&db, "activities");
    assert!(
        columns.contains(&"original_title".to_string()),
        "迁移 0003 应当给 activities 补上 original_title，实际列：{columns:?}"
    );
}

// 0.15d —— 迁移 0004：阶段与活动的关联表（一个阶段本来就包含多个活动）
#[test]
fn incremental_migration_adds_stage_activities() {
    let (_dir, path) = temp_db_path();
    let db = Database::open(&path).unwrap();

    let columns = table_columns(&db, "stage_activities");
    assert_eq!(
        columns,
        vec![
            "activity_id".to_string(),
            "position".to_string(),
            "stage_id".to_string()
        ],
        "关联表要能表达「阶段里活动的顺序」"
    );
}

#[test]
fn migrations_are_idempotent() {
    let (_dir, path) = temp_db_path();

    let db = Database::open(&path).unwrap();
    let before = db.schema_fingerprint().unwrap();
    drop(db);

    // 第二次打开同一个库：迁移应被识别为已应用，不重复执行
    let db = Database::open(&path).expect("reopening must succeed");
    assert_eq!(
        db.applied_migrations().unwrap(),
        mc_storage::migrate::defined_versions()
    );
    assert_eq!(
        db.schema_fingerprint().unwrap(),
        before,
        "重复打开不应改变 schema"
    );
}

// 0.17 — 兼容层列形状必须与旧版后端一致，否则现有前端会坏
#[test]
fn compat_tables_match_legacy_columns() {
    let (_dir, path) = temp_db_path();
    let db = Database::open(&path).unwrap();

    let expected: &[(&str, &[&str])] = &[
        (
            "vaults",
            &[
                "id",
                "title",
                "summary",
                "content",
                "tags",
                "parent_id",
                "is_folder",
                "is_deleted",
                "created_at",
                "updated_at",
                "document_type",
                "sort_order",
            ],
        ),
        (
            "todo",
            &[
                "id",
                "content",
                "created_at",
                "start_time",
                "end_time",
                "status",
                "urgency",
                "assignee",
                "reason",
            ],
        ),
        ("tips", &["id", "content", "created_at"]),
        (
            "activity",
            &[
                "id",
                "title",
                "content",
                "resources",
                "metadata",
                "start_time",
                "end_time",
            ],
        ),
        (
            "conversations",
            &[
                "id",
                "title",
                "user_id",
                "page_name",
                "status",
                "metadata",
                "created_at",
                "updated_at",
            ],
        ),
        (
            "messages",
            &[
                "id",
                "conversation_id",
                "parent_message_id",
                "role",
                "content",
                "status",
                "token_count",
                "metadata",
                "latency_ms",
                "error_message",
                "completed_at",
                "created_at",
                "updated_at",
            ],
        ),
        (
            "message_thinking",
            &[
                "id",
                "message_id",
                "content",
                "stage",
                "progress",
                "sequence",
                "metadata",
                "created_at",
            ],
        ),
    ];

    for (table, expected_cols) in expected {
        let mut want: Vec<String> = expected_cols.iter().map(|s| s.to_string()).collect();
        want.sort();
        assert_eq!(
            table_columns(&db, table),
            want,
            "表 {table} 的列与旧库不一致（前端依赖这些形状）"
        );
    }
}

#[test]
fn wal_mode_and_pragmas_applied() {
    let (_dir, path) = temp_db_path();
    let db = Database::open(&path).unwrap();

    assert_eq!(
        scalar_string(&db, "PRAGMA journal_mode").to_lowercase(),
        "wal",
        "必须启用 WAL，否则读写会互相阻塞"
    );
    assert_eq!(scalar_i64(&db, "PRAGMA foreign_keys"), 1);
    assert_eq!(scalar_i64(&db, "PRAGMA busy_timeout"), 5000);
    // synchronous = NORMAL(1)
    assert_eq!(scalar_i64(&db, "PRAGMA synchronous"), 1);
}

// 0.21 — 损坏的库必须报错并进入只读安全模式，绝不自动「修复」
#[test]
fn corrupt_database_is_detected_and_safe_mode_is_readonly() {
    let (_dir, path) = temp_db_path();
    std::fs::write(
        &path,
        b"this is definitely not a sqlite database file at all",
    )
    .unwrap();

    let err = Database::open(&path).expect_err("损坏的库必须被拒绝");
    assert_eq!(err.code(), ErrorCode::StorageCorrupt);
    assert!(
        err.remediation().is_some(),
        "StorageCorrupt 必须给出可执行建议（从备份恢复 / 导出诊断）"
    );

    let safe = Database::open_safe_mode(&path).expect("安全模式应当可用（只读）");
    assert!(safe.is_read_only());

    let write_err = safe.write_probe().expect_err("安全模式下不允许写入");
    assert!(
        matches!(
            write_err.code(),
            ErrorCode::StorageUnavailable | ErrorCode::StorageCorrupt
        ),
        "安全模式写入应报存储类错误，实际: {:?}",
        write_err.code()
    );
}

// 0.21b — 安全模式下读取也要给出明确错误，而不是 panic
#[test]
fn safe_mode_read_failure_is_a_typed_error() {
    let (_dir, path) = temp_db_path();
    std::fs::write(&path, b"garbage").unwrap();
    let safe = Database::open_safe_mode(&path).unwrap();

    let err = safe
        .with_read(|conn| conn.query_row("SELECT count(*) FROM events", [], |r| r.get::<_, i64>(0)))
        .expect_err("在垃圾文件上查询必须返回错误");

    assert!(matches!(
        err.code(),
        ErrorCode::StorageCorrupt | ErrorCode::StorageUnavailable
    ));
}

// 0.15b — 文件缺失时 open 会创建它（首次启动路径）
#[test]
fn open_creates_parent_directories() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/deeper/minecontext.db");

    let db = Database::open(&path).expect("应自动创建父目录");
    assert!(path.exists());
    assert!(!db.is_read_only());
}

// 0.16b — 迁移 checksum 变化必须被发现（防止改历史迁移导致各库不一致）
#[test]
fn migration_checksum_mismatch_is_detected() {
    let (_dir, path) = temp_db_path();
    let db = Database::open(&path).unwrap();
    db.with_write(|conn| {
        conn.execute(
            "UPDATE schema_migrations SET checksum = 'tampered' WHERE version = 1",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    drop(db);

    let err = Database::open(&path).expect_err("checksum 不匹配必须报错");
    assert_eq!(err.code(), ErrorCode::StorageMigrationFailed);
}
