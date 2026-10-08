//! 旧版本数据导入。
//!
//! 旧版布局与新版不同：`<CONTEXT_PATH>/persist/sqlite/app.db` + `screenshots/`
//! → `<data_dir>/data/minecontext.db` + `imported/screenshots/`。
//!
//! 三条取舍：**按列交集复制**（旧库多几列不该让导入失败）、**幂等**
//! （`INSERT OR IGNORE`，跑两遍是常态）、**旧截图只当文件保留**
//! （旧截图没有可靠的窗口/时间元数据，塞进观测时间线等于编造历史）。
//! 整份导入在一个事务里；`--dry-run` 走同一个事务但回滚，因此报告里的数字与
//! 真跑一次完全一致。

use std::path::{Path, PathBuf};

use mc_common::error::{AppError, ErrorCode};
use mc_common::time::Timestamp;
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;

use crate::db::Database;

/// 旧库相对 `<CONTEXT_PATH>` 的路径（旧配置里 `sqlite.config.path` 的默认值）。
const LEGACY_DB_RELATIVE: &str = "persist/sqlite/app.db";
/// 旧截图目录相对 `<CONTEXT_PATH>` 的路径。
const LEGACY_SCREENSHOTS_RELATIVE: &str = "screenshots";
/// 导入后截图落在数据目录内的位置。
const IMPORTED_SCREENSHOTS_RELATIVE: &str = "imported/screenshots";

/// 要导入的表，顺序即依赖顺序（先主表后引用表，外键才成立）。
///
/// 只列旧版真实存在、且新版兼容层有对应形状的表；旧库里有而新版没有的
/// （例如 `monitoring_*` 的一部分）会被报告为 skipped，而不是让导入失败。
const IMPORTABLE_TABLES: &[&str] = &[
    "vaults",
    "todo",
    "tips",
    "activity",
    "conversations",
    "messages",
    "message_thinking",
    "monitoring_token_usage",
    "monitoring_stage_timing",
    "monitoring_data_stats",
];

#[derive(Debug, Clone)]
pub struct ImportOptions {
    /// 旧数据目录（含 `persist/sqlite/app.db`），或直接指向旧库文件。
    pub from: PathBuf,
    /// 新数据目录（`minecontext.db` 的父目录）。
    pub data_dir: PathBuf,
    /// 只报告不写盘。迁移工具最危险的动作是「先跑一下看看」。
    pub dry_run: bool,
    /// 报告里的时间戳（注入而不是读时钟，便于测试）。
    pub now: Timestamp,
}

#[derive(Debug, Clone, Serialize)]
pub struct TableReport {
    pub table: String,
    /// 本次真正插入的行数。
    pub imported: u64,
    /// 主键冲突被忽略的行数（说明之前导入过）。
    pub duplicates: u64,
    /// 跳过的原因；`None` 表示这张表按预期处理了。
    pub skipped_reason: Option<String>,
    /// 旧库里有、新版没有的列：不致命，但要说清楚。
    pub ignored_columns: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScreenshotReport {
    pub copied: u64,
    /// 目标目录里已经有同名文件（第二次运行时就是它）。
    pub already_present: u64,
    pub destination: PathBuf,
    /// 说明为什么旧截图不会变成观测 —— 报告要能自我解释。
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportReport {
    pub source: PathBuf,
    pub dry_run: bool,
    pub started_at: Timestamp,
    pub tables: Vec<TableReport>,
    pub screenshots: ScreenshotReport,
}

impl ImportReport {
    pub fn table(&self, name: &str) -> Option<&TableReport> {
        self.tables.iter().find(|t| t.table == name)
    }

    pub fn total_imported(&self) -> u64 {
        self.tables.iter().map(|t| t.imported).sum()
    }

    pub fn total_duplicates(&self) -> u64 {
        self.tables.iter().map(|t| t.duplicates).sum()
    }
}

/// 把旧数据导入到已打开的新库。
///
/// `db` 是**新**库（调用方用 [`Database::open`] 打开，迁移已应用）；
/// 旧库以只读方式单独打开 —— 绝不改动用户的原数据。
pub fn import_legacy(db: &Database, options: &ImportOptions) -> Result<ImportReport, AppError> {
    let (legacy_db, screenshots_dir) = locate_legacy(&options.from)?;
    let legacy = Connection::open_with_flags(&legacy_db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| legacy_source_missing(&legacy_db, &error.to_string()))?;

    let destination = options.data_dir.join(IMPORTED_SCREENSHOTS_RELATIVE);

    // 整份导入在一个事务里：要么都进去，要么都不进。
    // dry-run 用同一个事务但回滚 —— 这样报告里的数字与真跑一次完全一致。
    let tables = db.with_write(|conn| {
        let tx = conn.transaction()?;
        let mut tables = Vec::new();

        for table in IMPORTABLE_TABLES {
            tables.push(import_table(&legacy, &tx, table)?);
        }

        if options.dry_run {
            tx.rollback()?;
        } else {
            tx.commit()?;
        }

        Ok(tables)
    })?;

    // 文件复制不参与 SQL 事务（SQLite 管不到文件系统），放在提交之后：
    // 复制失败不该把已经成功入库的行回滚掉。
    let screenshots = if options.dry_run {
        plan_screenshots(&screenshots_dir, &destination)
    } else {
        copy_screenshots(&screenshots_dir, &destination)?
    };

    Ok(ImportReport {
        source: legacy_db,
        dry_run: options.dry_run,
        started_at: options.now,
        tables,
        screenshots,
    })
}

/// 解析 `--from`：目录按旧布局找库与截图；直接给库文件时按旧目录约定
/// 从它的位置反推截图目录（用户可能改过 `sqlite.config.path`）。
fn locate_legacy(from: &Path) -> Result<(PathBuf, PathBuf), AppError> {
    if from.is_file() {
        let screenshots = from
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .map(|root| root.join(LEGACY_SCREENSHOTS_RELATIVE))
            .unwrap_or_else(|| PathBuf::from(LEGACY_SCREENSHOTS_RELATIVE));
        return Ok((from.to_path_buf(), screenshots));
    }

    let db = from.join(LEGACY_DB_RELATIVE);
    if db.is_file() {
        return Ok((db, from.join(LEGACY_SCREENSHOTS_RELATIVE)));
    }

    Err(legacy_source_missing(
        &db,
        &format!(
            "在 {} 下没有找到旧版数据库（找过 {}）",
            from.display(),
            db.display()
        ),
    ))
}

fn legacy_source_missing(db: &Path, detail: &str) -> AppError {
    AppError::new(
        ErrorCode::StorageLegacySourceMissing,
        format!("找不到旧版本数据：{detail}（期望路径 {}）", db.display()),
    )
    .with_context("expected_database", db.display().to_string())
}

/// 复制一张表：列取交集，主键冲突忽略。
fn import_table(
    legacy: &Connection,
    tx: &rusqlite::Transaction<'_>,
    table: &str,
) -> rusqlite::Result<TableReport> {
    let legacy_columns = columns_of(legacy, table)?;
    let Some(legacy_columns) = legacy_columns else {
        return Ok(TableReport {
            table: table.to_string(),
            imported: 0,
            duplicates: 0,
            skipped_reason: Some("旧库里没有这张表".to_string()),
            ignored_columns: Vec::new(),
        });
    };

    let new_columns = columns_of(tx, table)?;
    let Some(new_columns) = new_columns else {
        return Ok(TableReport {
            table: table.to_string(),
            imported: 0,
            duplicates: 0,
            skipped_reason: Some("新版结构里没有这张表（旧版专有）".to_string()),
            ignored_columns: legacy_columns,
        });
    };

    let shared: Vec<String> = legacy_columns
        .iter()
        .filter(|c| new_columns.contains(c))
        .cloned()
        .collect();
    let ignored_columns: Vec<String> = legacy_columns
        .iter()
        .filter(|c| !new_columns.contains(c))
        .cloned()
        .collect();

    if shared.is_empty() {
        return Ok(TableReport {
            table: table.to_string(),
            imported: 0,
            duplicates: 0,
            skipped_reason: Some("旧库与新版没有共同的列".to_string()),
            ignored_columns,
        });
    }

    let list = shared.join(", ");
    let placeholders = vec!["?"; shared.len()].join(", ");
    let select = format!("SELECT {list} FROM {table}");
    let insert = format!("INSERT OR IGNORE INTO {table} ({list}) VALUES ({placeholders})");

    let mut read = legacy.prepare(&select)?;
    let mut rows = read.query([])?;
    let mut statement = tx.prepare(&insert)?;

    let (mut imported, mut duplicates) = (0u64, 0u64);
    while let Some(row) = rows.next()? {
        let values: Vec<rusqlite::types::Value> = (0..shared.len())
            .map(|index| row.get::<_, rusqlite::types::Value>(index))
            .collect::<rusqlite::Result<_>>()?;

        // dry-run 也走真正的写入语句（事务随后回滚）：这样「会导入多少、
        // 有多少是重复的」两个数字都不需要第二套估算逻辑。
        let changed = statement.execute(rusqlite::params_from_iter(values.iter()))?;
        if changed > 0 {
            imported += 1;
        } else {
            duplicates += 1;
        }
    }

    Ok(TableReport {
        table: table.to_string(),
        imported,
        duplicates,
        skipped_reason: None,
        ignored_columns,
    })
}

/// 表存在时返回列名；不存在返回 `None`。
fn columns_of(conn: &Connection, table: &str) -> rusqlite::Result<Option<Vec<String>>> {
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table],
        |row| row.get(0),
    )?;
    if exists == 0 {
        return Ok(None);
    }

    let mut statement = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(Some(names))
}

fn screenshot_report(destination: PathBuf, copied: u64, already_present: u64) -> ScreenshotReport {
    ScreenshotReport {
        copied,
        already_present,
        destination,
        note: "旧截图没有可靠的窗口/时间元数据，因此只作为文件保留，\
               不会变成观测、也不会进检索。"
            .to_string(),
    }
}

/// 只看不复制：报告「会复制几个」。
fn plan_screenshots(source: &Path, destination: &Path) -> ScreenshotReport {
    let mut copied = 0;
    let mut already_present = 0;
    for entry in files_in(source) {
        if destination.join(&entry).exists() {
            already_present += 1;
        } else {
            copied += 1;
        }
    }
    screenshot_report(destination.to_path_buf(), copied, already_present)
}

/// 复制截图；同名文件已存在就跳过（幂等）。
fn copy_screenshots(source: &Path, destination: &Path) -> Result<ScreenshotReport, AppError> {
    let mut copied = 0;
    let mut already_present = 0;
    let names = files_in(source);

    if !names.is_empty() {
        std::fs::create_dir_all(destination).map_err(|error| {
            AppError::new(
                ErrorCode::StorageUnavailable,
                format!("无法创建截图导入目录 {}: {error}", destination.display()),
            )
        })?;
    }

    for name in names {
        let target = destination.join(&name);
        if target.exists() {
            already_present += 1;
            continue;
        }
        // 单张复制失败不中断整体：报告里的 copied 会少于总数，用户能看出来
        if std::fs::copy(source.join(&name), &target).is_ok() {
            copied += 1;
        }
    }

    Ok(screenshot_report(
        destination.to_path_buf(),
        copied,
        already_present,
    ))
}

/// 目录里的常规文件名（目录不存在时为空 —— 旧用户可能没开截图保存）。
fn files_in(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_file())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    names
}
