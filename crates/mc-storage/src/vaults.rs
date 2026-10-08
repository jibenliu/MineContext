//! 笔记树（兼容层 `vaults` 表）。
//!
//! 笔记树直接查这张表：
//! `SELECT * FROM vaults WHERE document_type IN ('DailyReport', 'vaults')
//!  AND is_deleted = 0 ORDER BY id DESC`。
//!
//! 因此形状必须逐字对齐：`document_type` 用 `'DailyReport'` / `'vaults'`，
//! 日报归档在标题为 `Summary` 的文件夹下（`VaultTitle.Summary`）——
//! 对齐了这些，日报就能**零改动**出现在笔记树里；对不齐就是白做。

use mc_common::error::AppError;
use mc_common::time::Timestamp;
use serde::{Deserialize, Serialize};

use crate::db::Database;

/// 日报的 `document_type`（与 `VaultDocumentType.DailyReport` 对应）。
pub const DOCUMENT_TYPE_DAILY_REPORT: &str = "DailyReport";
/// 普通笔记的 `document_type`（与 `VaultDocumentType.Vaults` 对应）。
pub const DOCUMENT_TYPE_VAULTS: &str = "vaults";
/// 归档日报的文件夹名（与 `VaultTitle.Summary` 对应）。
pub const FOLDER_SUMMARY: &str = "Summary";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VaultDocument {
    pub id: i64,
    pub title: String,
    pub summary: Option<String>,
    pub content: Option<String>,
    /// 逗号分隔的标签（不是 JSON）
    pub tags: Option<String>,
    pub parent_id: Option<i64>,
    pub is_folder: bool,
    pub document_type: String,
}

impl Database {
    /// 确保某个文件夹存在，返回它的 id。
    ///
    /// 幂等：文件夹按标题复用 —— 每次写日报都新建一个 `Summary` 文件夹，
    /// 用户的笔记树里就会堆出一串同名文件夹。
    pub fn ensure_folder(&self, title: &str, at: Timestamp) -> Result<i64, AppError> {
        self.with_write(|conn| {
            let existing: Option<i64> = conn
                .query_row(
                    "SELECT id FROM vaults
                     WHERE title = ?1 AND is_folder = 1 AND COALESCE(is_deleted, 0) = 0
                     ORDER BY id LIMIT 1",
                    rusqlite::params![title],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(id) = existing {
                return Ok(id);
            }

            conn.execute(
                "INSERT INTO vaults (title, summary, content, tags, parent_id, is_folder,
                                     is_deleted, created_at, updated_at, document_type, sort_order)
                 VALUES (?1, '', '', '', NULL, 1, 0, ?2, ?2, ?3, 0)",
                rusqlite::params![title, at.to_legacy_datetime(), DOCUMENT_TYPE_VAULTS],
            )?;
            Ok(conn.last_insert_rowid())
        })
    }

    /// 按（标题 + 类型）写入或更新一篇文档。返回行 id。
    ///
    /// **幂等**：同一天重复生成日报只会更新同一行。
    /// 否则用户会看到「09-30 日报」出现三遍。
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_vault_document(
        &self,
        document_type: &str,
        title: &str,
        summary: &str,
        content: &str,
        tags: &[String],
        parent_id: Option<i64>,
        at: Timestamp,
    ) -> Result<i64, AppError> {
        let tags = tags.join(",");
        self.with_write(move |conn| {
            let existing: Option<i64> = conn
                .query_row(
                    "SELECT id FROM vaults
                     WHERE title = ?1 AND document_type = ?2 AND COALESCE(is_folder, 0) = 0
                       AND COALESCE(is_deleted, 0) = 0
                     ORDER BY id LIMIT 1",
                    rusqlite::params![title, document_type],
                    |row| row.get(0),
                )
                .optional()?;

            match existing {
                Some(id) => {
                    conn.execute(
                        "UPDATE vaults SET summary = ?1, content = ?2, tags = ?3,
                                           parent_id = ?4, updated_at = ?5
                         WHERE id = ?6",
                        rusqlite::params![summary, content, tags, parent_id, at.as_millis(), id],
                    )?;
                    Ok(id)
                }
                None => {
                    conn.execute(
                        "INSERT INTO vaults (title, summary, content, tags, parent_id, is_folder,
                                             is_deleted, created_at, updated_at, document_type,
                                             sort_order)
                         VALUES (?1, ?2, ?3, ?4, ?5, 0, 0, ?6, ?6, ?7, 0)",
                        rusqlite::params![
                            title,
                            summary,
                            content,
                            tags,
                            parent_id,
                            at.to_legacy_datetime(),
                            document_type
                        ],
                    )?;
                    Ok(conn.last_insert_rowid())
                }
            }
        })
    }

    /// 笔记树的查询（`document_type IN (...)`，按 id 倒序）。
    pub fn read_vault_documents(
        &self,
        document_types: &[&str],
    ) -> Result<Vec<VaultDocument>, AppError> {
        let placeholders = document_types
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT id, title, summary, content, tags, parent_id, is_folder, document_type
             FROM vaults
             WHERE document_type IN ({placeholders}) AND COALESCE(is_deleted, 0) = 0
             ORDER BY id DESC"
        );
        let values: Vec<String> = document_types
            .iter()
            .map(|value| value.to_string())
            .collect();

        self.with_read(|conn| {
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(values.iter()), |row| {
                Ok(VaultDocument {
                    id: row.get("id")?,
                    title: row.get::<_, Option<String>>("title")?.unwrap_or_default(),
                    summary: row.get("summary")?,
                    content: row.get("content")?,
                    tags: row.get("tags")?,
                    parent_id: row.get("parent_id")?,
                    is_folder: row.get::<_, Option<i64>>("is_folder")?.unwrap_or(0) != 0,
                    document_type: row
                        .get::<_, Option<String>>("document_type")?
                        .unwrap_or_else(|| DOCUMENT_TYPE_VAULTS.to_string()),
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }
}

// ------------------------------------------------------------------ 表格读取层
//
// 笔记树与笔记编辑器直接读 `vaults` 表的行：列必须逐列对齐初始化 schema 里的定义，
// 返回形状也必须与 `SELECT *` / `INSERT` / `UPDATE` 一致，否则笔记树会空。
//

/// `vaults` 的一行，列与初始化 SQL `0001_init.sql` 一一对应。
///
/// `is_folder` / `is_deleted` 是 **0/1 数字**：调用方按数字比较
/// （`is_folder === 1`），返回布尔的 `true` 会让笔记树把所有节点当普通笔记。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VaultRow {
    pub id: i64,
    pub title: String,
    pub summary: String,
    pub content: String,
    /// 逗号分隔的标签
    pub tags: String,
    pub parent_id: Option<i64>,
    pub is_folder: i64,
    pub is_deleted: i64,
    pub created_at: String,
    pub updated_at: String,
    pub document_type: String,
    pub sort_order: i64,
}

/// 查询条件。字段为 `None` 表示「不过滤」。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VaultQuery {
    /// `document_type IN (...)`
    pub document_type: Vec<String>,
    /// `None` = 不过滤；`Some(None)` = 只看根节点（`parent_id IS NULL`）；
    /// `Some(Some(id))` = 只看该文件夹下的节点
    pub parent_id: Option<Option<i64>>,
    pub title: Option<String>,
    /// `Some(1)` 只看文件夹，`Some(0)` 只看文件
    pub is_folder: Option<i64>,
    /// `None` 时**默认排除已删除**（内部条件 `is_deleted = 0`）
    pub is_deleted: Option<i64>,
}

/// 新建一行所需的最小信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultUpsert {
    pub title: String,
    pub summary: String,
    pub content: String,
    pub tags: Vec<String>,
    pub parent_id: Option<i64>,
    pub is_folder: bool,
    pub document_type: String,
    pub sort_order: i64,
}

/// 局部更新。**没给的字段保持不变**（`PATCH` 语义）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VaultPatch {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub content: Option<String>,
    pub tags: Option<Vec<String>>,
    /// `Some(None)` = 设回 NULL（把笔记拖到根目录）
    pub parent_id: Option<Option<i64>>,
    pub is_folder: Option<bool>,
    pub is_deleted: Option<bool>,
    pub document_type: Option<String>,
    pub sort_order: Option<i64>,
}

impl VaultPatch {
    /// 有没有需要写入的字段。空 patch 是合法的（返回 0 行受影响）。
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.summary.is_none()
            && self.content.is_none()
            && self.tags.is_none()
            && self.parent_id.is_none()
            && self.is_folder.is_none()
            && self.is_deleted.is_none()
            && self.document_type.is_none()
            && self.sort_order.is_none()
    }
}

const VAULT_COLUMNS: &str = "id, title, summary, content, tags, parent_id, is_folder, \
                             is_deleted, created_at, updated_at, document_type, sort_order";

fn row_to_vault(row: &rusqlite::Row<'_>) -> rusqlite::Result<VaultRow> {
    Ok(VaultRow {
        id: row.get(0)?,
        title: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
        summary: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
        content: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
        tags: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
        parent_id: row.get(5)?,
        is_folder: row.get::<_, Option<i64>>(6)?.unwrap_or(0),
        is_deleted: row.get::<_, Option<i64>>(7)?.unwrap_or(0),
        created_at: row.get::<_, Option<String>>(8)?.unwrap_or_default(),
        updated_at: row.get::<_, Option<String>>(9)?.unwrap_or_default(),
        document_type: row
            .get::<_, Option<String>>(10)?
            .unwrap_or_else(|| DOCUMENT_TYPE_VAULTS.to_string()),
        sort_order: row.get::<_, Option<i64>>(11)?.unwrap_or(0),
    })
}

impl Database {
    /// 按条件查笔记树。排序：`updated_at DESC, id DESC`。
    pub fn query_vault_rows(&self, query: &VaultQuery) -> Result<Vec<VaultRow>, AppError> {
        let mut conditions: Vec<String> = Vec::new();
        let mut params: Vec<rusqlite::types::Value> = Vec::new();

        if !query.document_type.is_empty() {
            let placeholders = vec!["?"; query.document_type.len()].join(", ");
            conditions.push(format!("document_type IN ({placeholders})"));
            for value in &query.document_type {
                params.push(rusqlite::types::Value::Text(value.clone()));
            }
        }

        match query.parent_id {
            Some(None) => conditions.push("parent_id IS NULL".to_string()),
            Some(Some(id)) => {
                conditions.push("parent_id = ?".to_string());
                params.push(rusqlite::types::Value::Integer(id));
            }
            None => {}
        }

        if let Some(title) = &query.title {
            conditions.push("title = ?".to_string());
            params.push(rusqlite::types::Value::Text(title.clone()));
        }

        if let Some(is_folder) = query.is_folder {
            conditions.push("is_folder = ?".to_string());
            params.push(rusqlite::types::Value::Integer(is_folder));
        }

        // 不传 is_deleted 时排除已删除：主树查询从不显式传它
        match query.is_deleted {
            Some(value) => {
                conditions.push("is_deleted = ?".to_string());
                params.push(rusqlite::types::Value::Integer(value));
            }
            None => conditions.push("COALESCE(is_deleted, 0) = 0".to_string()),
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };
        let sql = format!(
            "SELECT {VAULT_COLUMNS} FROM vaults {where_clause} ORDER BY updated_at DESC, id DESC"
        );

        self.with_read(|conn| {
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), row_to_vault)?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    /// 单行读取。**已软删除的行查不到**（查询带 `is_deleted = 0`）。
    pub fn vault_row_by_id(&self, id: i64) -> Result<Option<VaultRow>, AppError> {
        self.with_read(|conn| {
            conn.query_row(
                &format!(
                    "SELECT {VAULT_COLUMNS} FROM vaults WHERE id = ?1 AND COALESCE(is_deleted, 0) = 0"
                ),
                [id],
                row_to_vault,
            )
            .optional()
        })
    }

    /// 新建一行（笔记或文件夹），返回自增 id。
    pub fn insert_vault_row(&self, vault: &VaultUpsert, at: Timestamp) -> Result<i64, AppError> {
        let timestamp = at.to_legacy_datetime();
        let tags = vault.tags.join(",");
        self.with_write(|conn| {
            conn.execute(
                "INSERT INTO vaults (title, summary, content, tags, parent_id, is_folder,
                                     is_deleted, created_at, updated_at, document_type, sort_order)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?7, ?8, ?9)",
                rusqlite::params![
                    vault.title,
                    vault.summary,
                    vault.content,
                    tags,
                    vault.parent_id,
                    vault.is_folder as i64,
                    timestamp,
                    vault.document_type,
                    vault.sort_order,
                ],
            )?;
            Ok(conn.last_insert_rowid())
        })
    }

    /// 局部更新，返回受影响行数。
    ///
    /// 会刷新 `updated_at`：列表按 `updated_at DESC` 排序，不刷新会让刚改过的
    /// 笔记排到最后。
    pub fn update_vault_row(
        &self,
        id: i64,
        patch: &VaultPatch,
        at: Timestamp,
    ) -> Result<usize, AppError> {
        if patch.is_empty() {
            return Ok(0);
        }

        let mut assignments: Vec<String> = Vec::new();
        let mut params: Vec<rusqlite::types::Value> = Vec::new();

        if let Some(title) = &patch.title {
            assignments.push("title = ?".to_string());
            params.push(rusqlite::types::Value::Text(title.clone()));
        }
        if let Some(summary) = &patch.summary {
            assignments.push("summary = ?".to_string());
            params.push(rusqlite::types::Value::Text(summary.clone()));
        }
        if let Some(content) = &patch.content {
            assignments.push("content = ?".to_string());
            params.push(rusqlite::types::Value::Text(content.clone()));
        }
        if let Some(tags) = &patch.tags {
            assignments.push("tags = ?".to_string());
            params.push(rusqlite::types::Value::Text(tags.join(",")));
        }
        if let Some(parent) = patch.parent_id {
            assignments.push("parent_id = ?".to_string());
            params.push(match parent {
                Some(value) => rusqlite::types::Value::Integer(value),
                None => rusqlite::types::Value::Null,
            });
        }
        if let Some(is_folder) = patch.is_folder {
            assignments.push("is_folder = ?".to_string());
            params.push(rusqlite::types::Value::Integer(is_folder as i64));
        }
        if let Some(is_deleted) = patch.is_deleted {
            assignments.push("is_deleted = ?".to_string());
            params.push(rusqlite::types::Value::Integer(is_deleted as i64));
        }
        if let Some(document_type) = &patch.document_type {
            assignments.push("document_type = ?".to_string());
            params.push(rusqlite::types::Value::Text(document_type.clone()));
        }
        if let Some(sort_order) = patch.sort_order {
            assignments.push("sort_order = ?".to_string());
            params.push(rusqlite::types::Value::Integer(sort_order));
        }

        assignments.push("updated_at = ?".to_string());
        params.push(rusqlite::types::Value::Text(at.to_legacy_datetime()));
        params.push(rusqlite::types::Value::Integer(id));

        let sql = format!("UPDATE vaults SET {} WHERE id = ?", assignments.join(", "));
        self.with_write(|conn| conn.execute(&sql, rusqlite::params_from_iter(params.iter())))
    }

    pub fn soft_delete_vault_row(&self, id: i64, at: Timestamp) -> Result<usize, AppError> {
        self.update_vault_row(
            id,
            &VaultPatch {
                is_deleted: Some(true),
                ..VaultPatch::default()
            },
            at,
        )
    }

    pub fn restore_vault_row(&self, id: i64, at: Timestamp) -> Result<usize, AppError> {
        self.update_vault_row(
            id,
            &VaultPatch {
                is_deleted: Some(false),
                ..VaultPatch::default()
            },
            at,
        )
    }

    /// 物理删除（`delete-vault-by-id` 与 `hard` 走同一条路径）。
    pub fn hard_delete_vault_row(&self, id: i64) -> Result<usize, AppError> {
        self.with_write(|conn| conn.execute("DELETE FROM vaults WHERE id = ?1", [id]))
    }
}

use rusqlite::OptionalExtension;
