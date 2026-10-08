-- 观测的图片引用补全。
--
-- 原 schema 只有 image_path / image_blob_hash。实际落地时还需要：
--   thumbnail_path —— UI 时间线用缩略图，不该加载原图
--   image_bytes    —— 保留策略与诊断需要知道单条观测占多少字节
--
-- 第二条迁移同时验证迁移机制本身：只允许追加、checksum 校验生效。

ALTER TABLE observations ADD COLUMN thumbnail_path TEXT;
ALTER TABLE observations ADD COLUMN image_bytes INTEGER;
