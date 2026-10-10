//! 对话与消息（兼容层 `conversations` / `messages` / `message_thinking`）。
//!
//! 助手界面对这三件事有硬依赖，缺一个就会坏：
//!
//! 1. **服务端负责落库**：客户端发完请求**不会**自己 append 消息，
//!    它靠下一轮 `getMessages` 拿服务器写的结果；
//! 2. `metadata` 是 **JSON 字符串**（客户端 `JSON.parse`），不是对象；
//! 3. 对话有 `page_name`：用它区分「首页的助手」与「笔记页的助手」。

use mc_common::error::AppError;
use mc_common::time::Timestamp;
use serde::{Deserialize, Serialize};

use crate::db::Database;

/// 兼容 `DATETIME` 列：写字符串，读的时候容忍整数毫秒。
fn legacy_at(at: Timestamp) -> String {
    at.to_legacy_datetime()
}

fn read_datetime(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Option<String>> {
    use rusqlite::types::Value;
    Ok(match row.get::<_, Option<Value>>(index)? {
        Some(Value::Text(text)) => Some(text),
        Some(Value::Integer(ms)) => Some(Timestamp::from_millis(ms).to_legacy_datetime()),
        Some(other) => Some(format!("{other:?}")),
        None => None,
    })
}

fn map_conversation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ConversationRow> {
    Ok(ConversationRow {
        id: row.get(0)?,
        title: row.get(1)?,
        page_name: row.get(2)?,
        status: row.get(3)?,
        metadata: row.get(4)?,
        vault_id: row.get(5)?,
        created_at: read_datetime(row, 6)?.unwrap_or_default(),
        updated_at: read_datetime(row, 7)?.unwrap_or_default(),
    })
}

/// 消息状态取值：`pending` / `streaming` / `completed` / `failed` / `cancelled`。
pub const STATUS_STREAMING: &str = "streaming";
pub const STATUS_COMPLETED: &str = "completed";
pub const STATUS_FAILED: &str = "failed";
pub const STATUS_CANCELLED: &str = "cancelled";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationRow {
    pub id: i64,
    pub title: Option<String>,
    pub page_name: String,
    pub status: String,
    /// **JSON 字符串**（客户端 `JSON.parse` 依赖）
    pub metadata: String,
    /// 归属的 vault 根文件夹 id；`None` = 未绑定（全局 / 兼容旧数据）
    pub vault_id: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageRow {
    pub id: i64,
    pub conversation_id: i64,
    /// 这里是字符串（可空）
    pub parent_message_id: Option<String>,
    pub role: String,
    pub content: String,
    pub status: String,
    pub token_count: i64,
    /// **JSON 字符串**
    pub metadata: String,
    pub latency_ms: i64,
    pub error_message: String,
    pub completed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// 思考记录（气泡里展示「正在做什么」）
    pub thinking: Vec<ThinkingRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThinkingRow {
    pub content: String,
    pub stage: Option<String>,
    pub progress: f64,
}

/// 待写入的一条消息。
///
/// 打包成结构体而不是七个参数：调用点读起来是「字段名 → 值」，
/// 也不会因为参数顺序写反而静默错位。
pub struct NewChatMessage<'a> {
    pub conversation_id: i64,
    pub role: &'a str,
    pub content: &'a str,
    pub status: &'a str,
    pub parent_message_id: Option<&'a str>,
    pub token_count: i64,
}

impl Database {
    /// 新建对话。`title` 允许为空 —— 缺省行为是「用第一条提问当标题」。
    /// `vault_id` 绑定到笔记树根文件夹，供会话列表与 RAG 隔离。
    pub fn create_conversation(
        &self,
        title: Option<&str>,
        page_name: &str,
        vault_id: Option<i64>,
        at: Timestamp,
    ) -> Result<i64, AppError> {
        self.with_write(|conn| {
            conn.execute(
                "INSERT INTO conversations (title, user_id, page_name, status, metadata,
                                            vault_id, created_at, updated_at)
                 VALUES (?1, 'local', ?2, 'active', '{}', ?3, ?4, ?4)",
                rusqlite::params![title, page_name, vault_id, legacy_at(at)],
            )?;
            Ok(conn.last_insert_rowid())
        })
    }

    pub fn get_conversation(&self, id: i64) -> Result<Option<ConversationRow>, AppError> {
        self.with_read(|conn| {
            conn.query_row(
                "SELECT id, title, COALESCE(page_name, 'home'), COALESCE(status, 'active'),
                        COALESCE(metadata, '{}'), vault_id, created_at, updated_at
                 FROM conversations WHERE id = ?1",
                rusqlite::params![id],
                map_conversation_row,
            )
            .optional()
        })
    }

    /// 只设置**未设置过**的标题（缺省行为：用第一条提问当标题，之后不再改）。
    pub fn set_conversation_title_if_empty(
        &self,
        id: i64,
        title: &str,
        at: Timestamp,
    ) -> Result<bool, AppError> {
        self.with_write(|conn| {
            let updated = conn.execute(
                "UPDATE conversations SET title = ?1, updated_at = ?2
                 WHERE id = ?3 AND (title IS NULL OR TRIM(title) = '')",
                rusqlite::params![title, legacy_at(at), id],
            )?;
            Ok(updated > 0)
        })
    }

    /// 合并写入对话的 `metadata`（`document_id` 等就放在这里）。
    pub fn update_conversation_metadata(
        &self,
        id: i64,
        patch: &serde_json::Value,
        at: Timestamp,
    ) -> Result<(), AppError> {
        let patch = patch.to_string();
        self.with_write(move |conn| {
            conn.execute(
                "UPDATE conversations
                 SET metadata = json_patch(COALESCE(metadata, '{}'), ?1), updated_at = ?2
                 WHERE id = ?3",
                rusqlite::params![patch, legacy_at(at), id],
            )?;
            Ok(())
        })
    }

    pub fn list_conversations(
        &self,
        page_name: Option<&str>,
        vault_id: Option<i64>,
    ) -> Result<Vec<ConversationRow>, AppError> {
        self.with_read(|conn| {
            let mut conditions =
                vec!["COALESCE(status, 'active') != 'deleted'".to_string()];
            let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
            if let Some(page) = page_name {
                conditions.push(format!("page_name = ?{}", params.len() + 1));
                params.push(Box::new(page.to_string()));
            }
            if let Some(vault) = vault_id {
                conditions.push(format!("vault_id = ?{}", params.len() + 1));
                params.push(Box::new(vault));
            }
            let sql = format!(
                "SELECT id, title, COALESCE(page_name, 'home'), COALESCE(status, 'active'),
                        COALESCE(metadata, '{{}}'), vault_id, created_at, updated_at
                 FROM conversations WHERE {}
                 ORDER BY id DESC",
                conditions.join(" AND ")
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(
                    rusqlite::params_from_iter(params.iter().map(|value| value.as_ref())),
                    map_conversation_row,
                )?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    /// 分页 + 状态筛选的对话列表：**下推到 SQL**。
    ///
    /// 分页与筛选都下推到 SQL：全量读出再在内存里 skip/take 时，对话一多每次列表
    /// 请求都要把整表读出来再丢掉大部分，`status` 筛选也形同虚设。
    /// 返回 (本页行, 满足条件的总数)，`total` 供前端算页数。
    pub fn list_conversations_page(
        &self,
        page_name: Option<&str>,
        status: Option<&str>,
        vault_id: Option<i64>,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<ConversationRow>, i64), AppError> {
        let limit = limit.clamp(1, 200);
        let offset = offset.max(0);
        self.with_read(|conn| {
            // 未指定 status 时按「未删除」过滤；指定了就按指定值过滤。
            let mut conditions: Vec<String> = Vec::new();
            let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
            if let Some(page) = page_name {
                conditions.push(format!("page_name = ?{}", params.len() + 1));
                params.push(Box::new(page.to_string()));
            }
            match status {
                Some(status) => {
                    conditions.push(format!("status = ?{}", params.len() + 1));
                    params.push(Box::new(status.to_string()));
                }
                None => conditions.push("COALESCE(status, 'active') != 'deleted'".to_string()),
            }
            if let Some(vault) = vault_id {
                conditions.push(format!("vault_id = ?{}", params.len() + 1));
                params.push(Box::new(vault));
            }
            let where_sql = conditions.join(" AND ");

            let total: i64 = conn.query_row(
                &format!("SELECT COUNT(*) FROM conversations WHERE {where_sql}"),
                rusqlite::params_from_iter(params.iter().map(|value| value.as_ref())),
                |row| row.get(0),
            )?;

            let sql = format!(
                "SELECT id, title, COALESCE(page_name, 'home'), COALESCE(status, 'active'),
                        COALESCE(metadata, '{{}}'), vault_id, created_at, updated_at
                 FROM conversations WHERE {where_sql}
                 ORDER BY id DESC LIMIT ?{} OFFSET ?{}",
                params.len() + 1,
                params.len() + 2
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut all: Vec<Box<dyn rusqlite::ToSql>> = params;
            all.push(Box::new(limit));
            all.push(Box::new(offset));
            let rows = stmt
                .query_map(
                    rusqlite::params_from_iter(all.iter().map(|value| value.as_ref())),
                    map_conversation_row,
                )?
                .collect::<Result<Vec<_>, _>>()?;
            Ok((rows, total))
        })
    }

    /// 新建消息，返回 id。
    pub fn create_message(
        &self,
        conversation_id: i64,
        role: &str,
        content: &str,
        status: &str,
        at: Timestamp,
    ) -> Result<i64, AppError> {
        self.with_write(|conn| {
            conn.execute(
                "INSERT INTO messages (conversation_id, parent_message_id, role, content, status,
                                       token_count, metadata, latency_ms, error_message,
                                       created_at, updated_at)
                 VALUES (?1, NULL, ?2, ?3, ?4, 0, '{}', 0, '', ?5, ?5)",
                rusqlite::params![conversation_id, role, content, status, legacy_at(at)],
            )?;
            Ok(conn.last_insert_rowid())
        })
    }

    /// 新建消息并记下父消息（请求路径里的 `mid` 就是父消息 id）。
    ///
    /// 与 [`Self::create_message`] 分开而不是加参数：那条路径由服务端自己的
    /// 流式对话使用，父消息永远是 `NULL`，两者语义不同。
    pub fn create_message_with_parent(
        &self,
        message: NewChatMessage<'_>,
        at: Timestamp,
    ) -> Result<i64, AppError> {
        self.with_write(|conn| {
            conn.execute(
                "INSERT INTO messages (conversation_id, parent_message_id, role, content, status,
                                       token_count, metadata, latency_ms, error_message,
                                       created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, '{}', 0, '', ?7, ?7)",
                rusqlite::params![
                    message.conversation_id,
                    message.parent_message_id,
                    message.role,
                    message.content,
                    message.status,
                    message.token_count,
                    legacy_at(at)
                ],
            )?;
            Ok(conn.last_insert_rowid())
        })
    }

    /// 整体替换消息内容（`message/{mid}/update`）。
    ///
    /// 与 [`Self::append_message_content`] 的区别是「替换」而不是「累加」——
    /// 前端拿到最终答案后回填时用它。返回是否真的改到了行（用于 404）。
    pub fn replace_message_content(
        &self,
        message_id: i64,
        new_content: &str,
        status: &str,
        token_count: Option<i64>,
        at: Timestamp,
    ) -> Result<bool, AppError> {
        self.with_write(|conn| {
            let changed = conn.execute(
                "UPDATE messages
                 SET content = ?1,
                     status = ?2,
                     token_count = COALESCE(?3, token_count),
                     updated_at = ?4
                 WHERE id = ?5",
                rusqlite::params![new_content, status, token_count, legacy_at(at), message_id],
            )?;
            Ok(changed > 0)
        })
    }

    /// 改对话标题（`PATCH conversations/{cid}/update`）。
    ///
    /// `updated_at` 必须一起刷新：列表按它排序，只改标题不刷新时间会让
    /// 刚改过标题的对话沉在下面。
    pub fn update_conversation_title(
        &self,
        conversation_id: i64,
        title: &str,
        at: Timestamp,
    ) -> Result<bool, AppError> {
        self.with_write(|conn| {
            let changed = conn.execute(
                "UPDATE conversations SET title = ?1, updated_at = ?2 WHERE id = ?3",
                rusqlite::params![title, legacy_at(at), conversation_id],
            )?;
            Ok(changed > 0)
        })
    }

    /// 软删除对话（`DELETE conversations/{cid}/update`）。
    ///
    /// **不删行、也不删消息**：接口语义就是软删除，硬删会让「删除后仍能
    /// 从消息里翻出历史」变成悬空数据。
    pub fn soft_delete_conversation(
        &self,
        conversation_id: i64,
        at: Timestamp,
    ) -> Result<bool, AppError> {
        self.with_write(|conn| {
            let changed = conn.execute(
                "UPDATE conversations SET status = 'deleted', updated_at = ?1 WHERE id = ?2",
                rusqlite::params![legacy_at(at), conversation_id],
            )?;
            Ok(changed > 0)
        })
    }

    /// 追加流式内容（每个 `stream_chunk` 追加一次）。
    pub fn append_message_content(
        &self,
        message_id: i64,
        chunk: &str,
        tokens: i64,
        at: Timestamp,
    ) -> Result<(), AppError> {
        self.with_write(|conn| {
            conn.execute(
                "UPDATE messages
                 SET content = COALESCE(content, '') || ?1,
                     token_count = COALESCE(token_count, 0) + ?2,
                     updated_at = ?3
                 WHERE id = ?4",
                rusqlite::params![chunk, tokens, legacy_at(at), message_id],
            )?;
            Ok(())
        })
    }

    /// 标记消息结束（`completed` / `failed` / `cancelled`）。
    pub fn mark_message_finished(
        &self,
        message_id: i64,
        status: &str,
        error_message: Option<&str>,
        at: Timestamp,
    ) -> Result<(), AppError> {
        self.with_write(|conn| {
            conn.execute(
                "UPDATE messages SET status = ?1, error_message = COALESCE(?2, ''), completed_at = ?3,
                                     updated_at = ?3
                 WHERE id = ?4",
                rusqlite::params![status, error_message, legacy_at(at), message_id],
            )?;
            Ok(())
        })
    }

    pub fn update_message_metadata(
        &self,
        message_id: i64,
        metadata: &serde_json::Value,
        at: Timestamp,
    ) -> Result<(), AppError> {
        let metadata = metadata.to_string();
        self.with_write(move |conn| {
            conn.execute(
                "UPDATE messages SET metadata = ?1, updated_at = ?2 WHERE id = ?3",
                rusqlite::params![metadata, legacy_at(at), message_id],
            )?;
            Ok(())
        })
    }

    /// 思考过程单独一张表，按消息聚合后在气泡里展示。
    pub fn add_message_thinking(
        &self,
        message_id: i64,
        content: &str,
        stage: Option<&str>,
        progress: f64,
        at: Timestamp,
    ) -> Result<i64, AppError> {
        self.with_write(|conn| {
            let sequence: i64 = conn.query_row(
                "SELECT COALESCE(MAX(sequence), 0) + 1 FROM message_thinking WHERE message_id = ?1",
                rusqlite::params![message_id],
                |row| row.get(0),
            )?;
            conn.execute(
                "INSERT INTO message_thinking (message_id, content, stage, progress, sequence,
                                               metadata, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, '{}', ?6)",
                rusqlite::params![
                    message_id,
                    content,
                    stage,
                    progress,
                    sequence,
                    legacy_at(at)
                ],
            )?;
            Ok(conn.last_insert_rowid())
        })
    }

    pub fn read_messages(&self, conversation_id: i64) -> Result<Vec<MessageRow>, AppError> {
        self.with_read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, conversation_id, parent_message_id, role, COALESCE(content, ''),
                        status, COALESCE(token_count, 0), COALESCE(metadata, '{}'),
                        COALESCE(latency_ms, 0), COALESCE(error_message, ''), completed_at,
                        created_at, updated_at
                 FROM messages WHERE conversation_id = ?1 ORDER BY id ASC",
            )?;
            let rows = stmt.query_map(rusqlite::params![conversation_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, String>(9)?,
                    read_datetime(row, 10)?,
                    read_datetime(row, 11)?.unwrap_or_default(),
                    read_datetime(row, 12)?.unwrap_or_default(),
                ))
            })?;

            let mut messages = Vec::new();
            for row in rows {
                let (
                    id,
                    conversation_id,
                    parent_message_id,
                    role,
                    content,
                    status,
                    token_count,
                    metadata,
                    latency_ms,
                    error_message,
                    completed_at,
                    created_at,
                    updated_at,
                ) = row?;

                let mut thinking_stmt = conn.prepare(
                    "SELECT content, stage, COALESCE(progress, 0.0) FROM message_thinking
                     WHERE message_id = ?1 ORDER BY sequence ASC",
                )?;
                let thinking = thinking_stmt
                    .query_map(rusqlite::params![id], |row| {
                        Ok(ThinkingRow {
                            content: row.get(0)?,
                            stage: row.get(1)?,
                            progress: row.get(2)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;

                messages.push(MessageRow {
                    id,
                    conversation_id,
                    parent_message_id,
                    role,
                    content,
                    status,
                    token_count,
                    metadata,
                    latency_ms,
                    error_message,
                    completed_at,
                    created_at,
                    updated_at,
                    thinking,
                });
            }
            Ok(messages)
        })
    }

    pub fn read_thinking(
        &self,
        message_id: i64,
    ) -> Result<Vec<(String, Option<String>, f64)>, AppError> {
        self.with_read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT content, stage, COALESCE(progress, 0.0) FROM message_thinking
                 WHERE message_id = ?1 ORDER BY sequence ASC",
            )?;
            let rows = stmt.query_map(rusqlite::params![message_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }
}

use rusqlite::OptionalExtension;
