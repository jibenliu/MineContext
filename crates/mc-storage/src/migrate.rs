//! schema 迁移 runner。
//!
//! 特性：
//! - 每个迁移在**独立事务**内执行，失败回滚，不会留下半截 schema
//! - 记录 checksum：历史迁移被改动会被发现（否则各用户的库会悄悄不一致）
//! - 可重复执行（幂等）

use mc_common::error::{AppError, ErrorCode};
use mc_common::time::{Clock, SystemClock};
use rusqlite::{Connection, OptionalExtension};

use crate::error::map_sqlite_error;

struct Migration {
    version: u32,
    name: &'static str,
    sql: &'static str,
}

/// 有序迁移清单。**只允许追加，不允许修改已发布的条目**（checksum 会拦住）。
const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "0001_init",
        sql: include_str!("../migrations/0001_init.sql"),
    },
    Migration {
        version: 2,
        name: "0002_observation_image_refs",
        sql: include_str!("../migrations/0002_observation_image_refs.sql"),
    },
    Migration {
        version: 3,
        name: "0003_activity_original_title",
        sql: include_str!("../migrations/0003_activity_original_title.sql"),
    },
    Migration {
        version: 4,
        name: "0004_stage_activities",
        sql: include_str!("../migrations/0004_stage_activities.sql"),
    },
    Migration {
        version: 5,
        name: "0005_vectors",
        sql: include_str!("../migrations/0005_vectors.sql"),
    },
    Migration {
        version: 6,
        name: "0006_activity_entities",
        sql: include_str!("../migrations/0006_activity_entities.sql"),
    },
    Migration {
        version: 7,
        name: "0007_app_settings",
        sql: include_str!("../migrations/0007_app_settings.sql"),
    },
];

pub fn run(conn: &mut Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
             version    INTEGER PRIMARY KEY,
             applied_at INTEGER NOT NULL,
             checksum   TEXT NOT NULL
         )",
    )
    .map_err(map_sqlite_error)?;

    for migration in MIGRATIONS {
        let checksum = blake3::hash(migration.sql.as_bytes()).to_hex().to_string();

        let recorded: Option<String> = conn
            .query_row(
                "SELECT checksum FROM schema_migrations WHERE version = ?1",
                [migration.version],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_sqlite_error)?;

        match recorded {
            Some(existing) if existing == checksum => continue,
            Some(existing) => {
                return Err(AppError::new(
                    ErrorCode::StorageMigrationFailed,
                    format!(
                        "迁移 {} ({}) 的 checksum 与已应用的记录不一致：\
                         记录为 {existing}，当前为 {checksum}。\
                         已发布的迁移不得修改；请新增一个迁移版本。",
                        migration.version, migration.name
                    ),
                ));
            }
            None => {
                let tx = conn.transaction().map_err(map_sqlite_error)?;
                tx.execute_batch(migration.sql).map_err(|e| {
                    AppError::new(
                        ErrorCode::StorageMigrationFailed,
                        format!(
                            "迁移 {} ({}) 执行失败：{e}",
                            migration.version, migration.name
                        ),
                    )
                })?;
                tx.execute(
                    "INSERT INTO schema_migrations (version, applied_at, checksum) VALUES (?1, ?2, ?3)",
                    rusqlite::params![
                        migration.version,
                        SystemClock.now().as_millis(),
                        checksum
                    ],
                )
                .map_err(map_sqlite_error)?;
                tx.commit().map_err(map_sqlite_error)?;
            }
        }
    }

    Ok(())
}

/// 当前代码里定义的全部迁移版本（升序）。
///
/// 测试用它断言「所有已定义的迁移都已应用」，因此新增迁移时不需要改测试，
/// 但仍然能抓到「迁移没跑」这类问题。
pub fn defined_versions() -> Vec<u32> {
    MIGRATIONS.iter().map(|m| m.version).collect()
}

pub fn applied(conn: &Connection) -> Result<Vec<u32>, AppError> {
    let mut stmt = conn
        .prepare("SELECT version FROM schema_migrations ORDER BY version ASC")
        .map_err(map_sqlite_error)?;
    let rows = stmt
        .query_map([], |row| row.get::<_, i64>(0))
        .map_err(map_sqlite_error)?;
    rows.collect::<Result<Vec<i64>, _>>()
        .map(|v| v.into_iter().map(|x| x as u32).collect())
        .map_err(map_sqlite_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_versions_are_strictly_increasing() {
        for pair in MIGRATIONS.windows(2) {
            assert!(
                pair[0].version < pair[1].version,
                "迁移版本必须递增: {} -> {}",
                pair[0].version,
                pair[1].version
            );
        }
    }
}
