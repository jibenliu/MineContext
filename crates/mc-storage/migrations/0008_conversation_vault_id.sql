-- 对话归属到笔记树根文件夹（vault），使助手会话与 RAG 按 vault 隔离。
-- NULL = 未绑定（兼容旧数据与「全部」视图）。
ALTER TABLE conversations ADD COLUMN vault_id INTEGER;
