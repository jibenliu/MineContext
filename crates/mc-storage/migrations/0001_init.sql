-- MineContext Rust 核心 · 初始 schema
--
-- 两部分：
--   A. 兼容层：列名/语义与旧版实现的 SQLite 后端一致
--      完全一致，因为现有 React 前端直接依赖这些形状（见 docs/rust-rewrite/03-storage-and-events.md §2.1）。
--      兼容层是**投影**，由 mc-storage 的投影器写入，不作为真源。
--   B. 核心表：事件存储（唯一真源）+ 可重建的派生表 + 可观测性表。
--
-- 时间约定：核心表一律 `*_utc_ms INTEGER`（UTC 毫秒）。
-- 兼容层维持旧的 DATETIME 文本列，由投影器按用户时区格式化。

-- ============================================================
-- A. 兼容层（**不得随意改列名或语义**）
-- ============================================================

CREATE TABLE IF NOT EXISTS vaults (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  title         TEXT,
  summary       TEXT,
  content       TEXT,
  tags          TEXT,
  parent_id     INTEGER,
  is_folder     BOOLEAN DEFAULT 0,
  is_deleted    BOOLEAN DEFAULT 0,
  created_at    DATETIME DEFAULT CURRENT_TIMESTAMP,
  updated_at    DATETIME DEFAULT CURRENT_TIMESTAMP,
  document_type TEXT DEFAULT 'vaults',
  sort_order    INTEGER DEFAULT 0
);

CREATE TABLE IF NOT EXISTS todo (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  content    TEXT,
  created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  start_time DATETIME DEFAULT CURRENT_TIMESTAMP,
  end_time   DATETIME,
  status     INTEGER DEFAULT 0,
  urgency    INTEGER DEFAULT 0,
  assignee   TEXT,
  reason     TEXT
);

CREATE TABLE IF NOT EXISTS tips (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  content    TEXT,
  created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS activity (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  title      TEXT,
  content    TEXT,
  resources  JSON,
  metadata   JSON,
  start_time DATETIME,
  end_time   DATETIME
);

CREATE TABLE IF NOT EXISTS conversations (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  title      TEXT,
  user_id    TEXT,
  page_name  VARCHAR(20) DEFAULT 'home',
  status     VARCHAR(20) DEFAULT 'active',
  metadata   JSON DEFAULT '{}',
  created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS messages (
  id                INTEGER PRIMARY KEY AUTOINCREMENT,
  conversation_id   INTEGER NOT NULL,
  parent_message_id TEXT,
  role              TEXT NOT NULL,
  content           TEXT DEFAULT '',
  status            TEXT NOT NULL DEFAULT 'pending',
  token_count       INTEGER DEFAULT 0,
  metadata          JSON DEFAULT '{}',
  latency_ms        INTEGER DEFAULT 0,
  error_message     TEXT DEFAULT '',
  completed_at      DATETIME,
  created_at        DATETIME DEFAULT CURRENT_TIMESTAMP,
  updated_at        DATETIME DEFAULT CURRENT_TIMESTAMP,
  FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS message_thinking (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  message_id INTEGER NOT NULL,
  content    TEXT NOT NULL,
  stage      TEXT,
  progress   REAL DEFAULT 0.0,
  sequence   INTEGER DEFAULT 0,
  metadata   JSON DEFAULT '{}',
  created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  FOREIGN KEY(message_id) REFERENCES messages(id) ON DELETE CASCADE
);

-- 兼容层索引（与旧库同名同形，便于备份/迁移比对）
CREATE INDEX IF NOT EXISTS idx_vaults_created ON vaults (created_at);
CREATE INDEX IF NOT EXISTS idx_vaults_type ON vaults (document_type);
CREATE INDEX IF NOT EXISTS idx_vaults_folder ON vaults (is_folder);
CREATE INDEX IF NOT EXISTS idx_vaults_deleted ON vaults (is_deleted);
CREATE INDEX IF NOT EXISTS idx_todo_status ON todo (status);
CREATE INDEX IF NOT EXISTS idx_todo_urgency ON todo (urgency);
CREATE INDEX IF NOT EXISTS idx_todo_created ON todo (created_at);
CREATE INDEX IF NOT EXISTS idx_activity_time ON activity (start_time, end_time);
CREATE INDEX IF NOT EXISTS idx_tips_time ON tips (created_at);
CREATE INDEX IF NOT EXISTS idx_messages_created_at ON messages(created_at);
CREATE INDEX IF NOT EXISTS idx_messages_status ON messages(status);
CREATE INDEX IF NOT EXISTS idx_messages_conversation_id ON messages(conversation_id);
CREATE INDEX IF NOT EXISTS idx_conversations_updated_at ON conversations(updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_conversations_user_page ON conversations(user_id, page_name);
CREATE INDEX IF NOT EXISTS idx_message_thinking_message_id ON message_thinking(message_id);
CREATE INDEX IF NOT EXISTS idx_message_thinking_stage ON message_thinking(message_id, stage);
CREATE INDEX IF NOT EXISTS idx_message_thinking_sequence ON message_thinking(message_id, sequence);

-- ============================================================
-- B. 核心表
-- ============================================================

-- 事件存储：唯一真源，append-only
CREATE TABLE IF NOT EXISTS events (
  seq            INTEGER PRIMARY KEY AUTOINCREMENT,
  at_utc_ms      INTEGER NOT NULL,
  kind           TEXT NOT NULL,
  schema_version INTEGER NOT NULL DEFAULT 1,
  payload        TEXT NOT NULL,
  actor          TEXT NOT NULL DEFAULT 'system'
);
CREATE INDEX IF NOT EXISTS idx_events_at   ON events(at_utc_ms);
CREATE INDEX IF NOT EXISTS idx_events_kind ON events(kind, at_utc_ms);

-- 投影位点（与派生表在同一事务内推进）
CREATE TABLE IF NOT EXISTS projection_checkpoints (
  projector  TEXT PRIMARY KEY,
  last_seq   INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL
);

-- 观测（不可变原始事实）
CREATE TABLE IF NOT EXISTS observations (
  id              TEXT PRIMARY KEY,
  ts_utc_ms       INTEGER NOT NULL,
  source_id       TEXT NOT NULL,
  kind            TEXT NOT NULL,
  app_name        TEXT,
  app_bundle_id   TEXT,
  app_pid         INTEGER,
  window_title    TEXT,
  window_rect     TEXT,
  domain          TEXT,
  display_id      TEXT,
  scale_factor    REAL,
  image_path      TEXT,
  image_blob_hash TEXT,
  image_w         INTEGER,
  image_h         INTEGER,
  phash           INTEGER,
  text_content    TEXT,
  text_origin     TEXT,
  text_confidence REAL,
  change_kind     TEXT NOT NULL,
  privacy_verdict TEXT NOT NULL,
  idempotency     TEXT NOT NULL,
  created_at      INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_obs_idem   ON observations(idempotency);
CREATE INDEX IF NOT EXISTS        idx_obs_ts     ON observations(ts_utc_ms DESC);
CREATE INDEX IF NOT EXISTS        idx_obs_phash  ON observations(phash);
CREATE INDEX IF NOT EXISTS        idx_obs_app_ts ON observations(app_name, ts_utc_ms DESC);

-- 分析结果（1:1 于 observation；失败只改这一行，观测永不消失）
CREATE TABLE IF NOT EXISTS analyses (
  observation_id    TEXT PRIMARY KEY REFERENCES observations(id) ON DELETE CASCADE,
  status            TEXT NOT NULL,
  tier              TEXT NOT NULL,
  provider_id       TEXT,
  model             TEXT,
  structured        TEXT,
  raw_response      TEXT,
  skip_reason       TEXT,
  error_code        TEXT,
  error_component   TEXT,
  error_message     TEXT,
  error_retryable   INTEGER,
  attempts          INTEGER NOT NULL DEFAULT 0,
  started_at        INTEGER,
  finished_at       INTEGER,
  prompt_tokens     INTEGER DEFAULT 0,
  completion_tokens INTEGER DEFAULT 0,
  latency_ms        INTEGER
);
CREATE INDEX IF NOT EXISTS idx_analyses_status ON analyses(status, tier);

-- 统一作业表：vision 分析与 summary 作业共用
-- priority: 0=Interactive 1=StageSummary 2=VisionMajor 3=VisionMinor 4=Backfill
CREATE TABLE IF NOT EXISTS jobs (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  kind             TEXT NOT NULL,
  priority         INTEGER NOT NULL DEFAULT 100,
  observation_id   TEXT REFERENCES observations(id) ON DELETE CASCADE,
  stage_id         TEXT,
  adhoc_id         TEXT,
  idempotency      TEXT NOT NULL,
  state            TEXT NOT NULL,
  weight_bytes     INTEGER NOT NULL DEFAULT 0,
  attempts         INTEGER NOT NULL DEFAULT 0,
  not_before       INTEGER NOT NULL DEFAULT 0,
  lease_owner      TEXT,
  lease_expires_at INTEGER,
  skip_reason      TEXT,
  last_error       TEXT,
  created_at       INTEGER NOT NULL,
  updated_at       INTEGER NOT NULL,
  UNIQUE(kind, idempotency)
);
CREATE INDEX IF NOT EXISTS idx_jobs_pick  ON jobs(state, priority, not_before, id);
CREATE INDEX IF NOT EXISTS idx_jobs_obs   ON jobs(observation_id);
CREATE INDEX IF NOT EXISTS idx_jobs_lease ON jobs(state, lease_expires_at);

-- 任意时段总结请求（13-adhoc-summary.md §5）
CREATE TABLE IF NOT EXISTS adhoc_requests (
  id            TEXT PRIMARY KEY,
  range_from_ms INTEGER NOT NULL,
  range_to_ms   INTEGER NOT NULL,
  scope_hash    TEXT NOT NULL,
  summary_id    TEXT,
  state         TEXT NOT NULL,
  event_seq_hi  INTEGER NOT NULL,
  chunks_total  INTEGER NOT NULL DEFAULT 1,
  chunks_done   INTEGER NOT NULL DEFAULT 0,
  created_at    INTEGER NOT NULL,
  finished_at   INTEGER,
  error         TEXT,
  UNIQUE(range_from_ms, range_to_ms, scope_hash)
);
CREATE INDEX IF NOT EXISTS idx_adhoc_state ON adhoc_requests(state, created_at DESC);

-- 派生：活动
CREATE TABLE IF NOT EXISTS activities (
  id               TEXT PRIMARY KEY,
  legacy_id        INTEGER UNIQUE,
  start_utc_ms     INTEGER NOT NULL,
  end_utc_ms       INTEGER,
  title            TEXT NOT NULL,
  category         TEXT,
  origin           TEXT NOT NULL,
  origin_ref       TEXT,
  confidence       REAL,
  evidence         TEXT NOT NULL,
  entity_ids       TEXT,
  derived_from_seq INTEGER NOT NULL,
  superseded_by    TEXT
);
CREATE INDEX IF NOT EXISTS idx_act_time ON activities(start_utc_ms, end_utc_ms);
CREATE INDEX IF NOT EXISTS idx_act_cat  ON activities(category, start_utc_ms DESC);

CREATE TABLE IF NOT EXISTS activity_observations (
  activity_id    TEXT NOT NULL REFERENCES activities(id) ON DELETE CASCADE,
  observation_id TEXT NOT NULL,
  PRIMARY KEY(activity_id, observation_id)
);

-- 派生：阶段
CREATE TABLE IF NOT EXISTS stages (
  id               TEXT PRIMARY KEY,
  activity_id      TEXT,
  start_utc_ms     INTEGER NOT NULL,
  end_utc_ms       INTEGER,
  state            TEXT NOT NULL,
  end_reason       TEXT,
  day              TEXT NOT NULL,
  derived_from_seq INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_stages_day ON stages(day, start_utc_ms);

-- 派生：总结（stage 自动 / adhoc 按需 / daily / weekly 共用）
CREATE TABLE IF NOT EXISTS summaries (
  id               TEXT PRIMARY KEY,
  kind             TEXT NOT NULL DEFAULT 'stage',
  stage_id         TEXT REFERENCES stages(id) ON DELETE CASCADE,
  template_id      TEXT NOT NULL,
  start_utc_ms     INTEGER NOT NULL,
  end_utc_ms       INTEGER NOT NULL,
  title            TEXT NOT NULL,
  fields_json      TEXT NOT NULL,
  body_markdown    TEXT NOT NULL,
  quality          TEXT NOT NULL,
  model            TEXT,
  scope_json       TEXT,
  prompt_tokens    INTEGER DEFAULT 0,
  completion_tokens INTEGER DEFAULT 0,
  vault_id         INTEGER,
  created_at       INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_summaries_stage     ON summaries(stage_id);
CREATE INDEX IF NOT EXISTS idx_summaries_kind_time ON summaries(kind, start_utc_ms DESC);
CREATE INDEX IF NOT EXISTS idx_summaries_range     ON summaries(start_utc_ms, end_utc_ms);

-- 用户覆盖（重放后仍然生效）
CREATE TABLE IF NOT EXISTS user_overrides (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  target_kind TEXT NOT NULL,
  target_id   TEXT NOT NULL,
  patch       TEXT NOT NULL,
  at_utc_ms   INTEGER NOT NULL,
  UNIQUE(target_kind, target_id)
);

-- 失败记录（错误必须可见）
CREATE TABLE IF NOT EXISTS pipeline_failures (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  at_utc_ms   INTEGER NOT NULL,
  component   TEXT NOT NULL,
  error_code  TEXT NOT NULL,
  severity    TEXT NOT NULL,
  message     TEXT NOT NULL,
  remediation TEXT,
  context     TEXT,
  retryable   INTEGER NOT NULL,
  retry_count INTEGER DEFAULT 0,
  resolved_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_fail_at ON pipeline_failures(at_utc_ms DESC);

-- 成本记账
CREATE TABLE IF NOT EXISTS provider_calls (
  id                INTEGER PRIMARY KEY AUTOINCREMENT,
  at_utc_ms         INTEGER NOT NULL,
  provider_id       TEXT NOT NULL,
  model             TEXT NOT NULL,
  purpose           TEXT NOT NULL,
  observation_id    TEXT,
  stage_id          TEXT,
  prompt_tokens     INTEGER DEFAULT 0,
  completion_tokens INTEGER DEFAULT 0,
  latency_ms        INTEGER NOT NULL,
  result            TEXT NOT NULL,
  error_code        TEXT
);
CREATE INDEX IF NOT EXISTS idx_calls_at ON provider_calls(at_utc_ms DESC);

-- 实体
CREATE TABLE IF NOT EXISTS entities (
  id         TEXT PRIMARY KEY,
  kind       TEXT NOT NULL,
  canonical  TEXT NOT NULL,
  aliases    TEXT,
  first_seen INTEGER NOT NULL,
  last_seen  INTEGER NOT NULL,
  origin     TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_entities_canonical ON entities(kind, canonical);

CREATE TABLE IF NOT EXISTS activity_entities (
  activity_id TEXT NOT NULL,
  entity_id   TEXT NOT NULL,
  role        TEXT NOT NULL,
  PRIMARY KEY(activity_id, entity_id, role)
);

-- 配置快照（脱敏，用于诊断）
CREATE TABLE IF NOT EXISTS config_snapshots (
  id        INTEGER PRIMARY KEY AUTOINCREMENT,
  at_utc_ms INTEGER NOT NULL,
  content   TEXT NOT NULL,
  source    TEXT NOT NULL
);
