-- Phase 6 前置：通用设置 KV（旧 `electron-store` 的等价面）。
--
-- 渲染层存的**不是**「键值都是字符串」：`todoList-finished` 是布尔、
-- `todoList` 是对象数组、`capture` 是嵌套对象。因此 value 存 JSON 文本，
-- 读出来再解析 —— 按字符串存会把布尔读成 `"true"`，
-- 前端 `if (isFinished)` 于是永远为真（这类错误不报错，只是行为反了）。
--
-- 为什么用表而不是 JSON 文件：单写者 + 事务，两个窗口同时写不会丢更新
-- （旧 `electron-store` 是「读-改-写整个文件」）。

CREATE TABLE IF NOT EXISTS app_settings (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
