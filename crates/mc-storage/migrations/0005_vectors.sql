-- Phase 5 / 5.5–5.7：向量持久化、维度声明与可续重建水位线。
--
-- 三条约束体现在 schema 上：
--   * 向量是**派生数据**：整张表可以随时清空重建，不影响 events/observations。
--   * 维度**不是常量**：`vector_state` 记录当前 provider 的模型与维度，
--     换模型时能立刻发现不一致并提示重建（旧系统写死 1536 维度）。
--   * 重建**可续**：`vector_rebuild_state` 按 kind 记录已索引到的事件序号。

CREATE TABLE IF NOT EXISTS vectors (
    kind        TEXT    NOT NULL,
    doc_id      TEXT    NOT NULL,
    model       TEXT    NOT NULL,
    dimensions  INTEGER NOT NULL CHECK (dimensions > 0),
    -- f32 小端连续排列。SQLite 的 BLOB 读写是无损的（不用 JSON 存浮点，
    -- 否则 0.1 会变成 0.100000001490116 并且体积翻三倍）。
    vector      BLOB    NOT NULL,
    updated_at  TEXT    NOT NULL,
    PRIMARY KEY (kind, doc_id, model)
);

CREATE INDEX IF NOT EXISTS idx_vectors_kind ON vectors(kind);

-- 键值状的状态表：embedding_space / rebuild watermark 都放这里。
CREATE TABLE IF NOT EXISTS vector_state (
    key         TEXT PRIMARY KEY,
    value       TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
