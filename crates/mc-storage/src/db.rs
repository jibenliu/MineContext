//! SQLite 连接与访问策略。
//!
//! 并发模型：
//! **单写者**。所有写入经 `with_write` 串行化；读取走 `with_read`。
//! 多个进程各开一个句柄、pragma 还不一致会引入跨进程锁竞争，
//! 单写者从结构上排除这种形态。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use mc_common::error::{AppError, ErrorCode};
use mc_common::observability::warn;
use rusqlite::{Connection, OpenFlags};

use crate::error::map_sqlite_error;
use crate::migrate;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessMode {
    ReadWrite,
    /// 数据库损坏时的安全模式：只读、不迁移、不写入
    ReadOnlySafe,
}

pub struct Database {
    conn: Mutex<Connection>,
    path: PathBuf,
    mode: AccessMode,
    /// 作业重试上限。可调，因为它直接影响「烧多少钱」。
    pub(crate) job_max_attempts: u32,
}

impl std::fmt::Debug for Database {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Database")
            .field("path", &self.path)
            .field("mode", &self.mode)
            .finish()
    }
}

impl Database {
    /// 打开（或创建）数据库：校验完整性 → 应用 pragma → 执行迁移。
    ///
    /// 完整性校验失败时返回 [`ErrorCode::StorageCorrupt`]，**不会**尝试自动修复。
    /// 调用方应改用 [`Database::open_safe_mode`] 进入只读安全模式并提示用户。
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AppError> {
        let path = path.as_ref().to_path_buf();

        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    AppError::new(
                        ErrorCode::StorageUnavailable,
                        format!("无法创建数据目录 {}: {e}", parent.display()),
                    )
                })?;
            }
        }

        let conn = Connection::open(&path).map_err(map_sqlite_error)?;
        verify_integrity(&conn)?;
        apply_pragmas(&conn)?;

        let mut db = Self {
            conn: Mutex::new(conn),
            path,
            mode: AccessMode::ReadWrite,
            job_max_attempts: 3,
        };
        {
            let conn = db.conn.get_mut().expect("刚构造，锁不可能被污染");
            migrate::run(conn)?;
        }
        Ok(db)
    }

    /// 只读安全模式：用于数据库损坏后仍允许用户查看历史、导出诊断。
    pub fn open_safe_mode(path: impl AsRef<Path>) -> Result<Self, AppError> {
        warn!(
            component = "storage",
            event = "safe_mode",
            "数据库不可写，进入只读安全模式"
        );
        let path = path.as_ref().to_path_buf();
        let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(map_sqlite_error)?;
        Ok(Self {
            conn: Mutex::new(conn),
            path,
            mode: AccessMode::ReadOnlySafe,
            job_max_attempts: 3,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn mode(&self) -> AccessMode {
        self.mode
    }

    pub fn is_read_only(&self) -> bool {
        self.mode == AccessMode::ReadOnlySafe
    }

    pub fn applied_migrations(&self) -> Result<Vec<u32>, AppError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| AppError::new(ErrorCode::StorageUnavailable, "数据库锁被污染"))?;
        migrate::applied(&conn)
    }

    /// schema 指纹：用于「重复打开不应改变 schema」这类断言。
    pub fn schema_fingerprint(&self) -> Result<String, AppError> {
        let joined = self.with_read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT type, name, COALESCE(sql, '') FROM sqlite_master
                 WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(format!(
                    "{}|{}|{}",
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?
                ))
            })?;
            let mut joined = String::new();
            for row in rows {
                joined.push_str(&row?);
                joined.push('\n');
            }
            Ok(joined)
        })?;
        Ok(blake3::hash(joined.as_bytes()).to_hex().to_string())
    }

    /// 尝试一次写入。安全模式下必然失败——用于向 UI 证明「当前是只读的」。
    pub fn write_probe(&self) -> Result<(), AppError> {
        self.with_write(|conn| {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS __mc_write_probe (id INTEGER PRIMARY KEY)",
            )?;
            Ok(())
        })
    }

    /// 供同 crate 的模块使用：需要在一个事务里做多件事（例如「写观测 + 追加事件」）
    /// 时，`with_write` 的 `rusqlite::Result` 约束不够用，需要能返回 `AppError`。
    pub(crate) fn lock_write(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, rusqlite::Connection>, AppError> {
        self.conn
            .lock()
            .map_err(|_| AppError::new(ErrorCode::StorageUnavailable, "数据库锁被污染"))
    }

    pub fn with_read<T>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Result<T, AppError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| AppError::new(ErrorCode::StorageUnavailable, "数据库锁被污染"))?;
        f(&conn).map_err(map_sqlite_error)
    }

    pub fn with_write<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> rusqlite::Result<T>,
    ) -> Result<T, AppError> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| AppError::new(ErrorCode::StorageUnavailable, "数据库锁被污染"))?;
        f(&mut conn).map_err(map_sqlite_error)
    }
}

fn verify_integrity(conn: &Connection) -> Result<(), AppError> {
    match conn.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0)) {
        Ok(result) if result.eq_ignore_ascii_case("ok") => Ok(()),
        Ok(other) => Err(AppError::new(
            ErrorCode::StorageCorrupt,
            format!("PRAGMA integrity_check 返回 {other}"),
        )),
        // 连读都读不了（例如文件不是数据库）→ 一律按损坏处理，
        // 交给调用方决定是否进入安全模式，绝不自动「修复」。
        Err(e) => Err(AppError::new(
            ErrorCode::StorageCorrupt,
            format!("无法读取数据库：{e}"),
        )),
    }
}

fn apply_pragmas(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA busy_timeout = 5000;
         PRAGMA foreign_keys = ON;
         PRAGMA cache_size = -64000;",
    )
    .map_err(map_sqlite_error)
}
