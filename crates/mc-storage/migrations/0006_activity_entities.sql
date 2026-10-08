-- Phase 5 / 5.35–5.39 收尾：实体关联落库。
--
-- `0001_init.sql` 里已经建好了 `entities` / `activity_entities` 两张表
-- （设计文档 `03-storage-and-events.md` §实体），但一直没有写入方 ——
-- 线索是每次查询时**重新抽一遍实体**得到的。这里把写入方补上，并补两个缺口：
--
--   1. `entities` 缺一列「用户看到的样子」。归并会把 `apex-389` 与
--      `APEX-389` 视为同一个实体，但展示名不能因此被改写
--      （用户看到的东西必须是原文）；
--   2. 反查「这个实体出现在哪些活动里」需要 `entity_id` 上的索引。
--
-- `activity_entities.role` 的取值：当前只有 `mentioned`
-- （实体在该活动标题里被提到）。预留给将来「主实体 / 被引用」之类的区分。

ALTER TABLE entities ADD COLUMN display_name TEXT;

CREATE INDEX IF NOT EXISTS idx_activity_entities_entity
    ON activity_entities(entity_id, activity_id);
