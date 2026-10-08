-- 迁移 0004：阶段与活动的关联表。
--
-- stages.activity_id 只能记一个活动，而一个阶段本来就包含多个活动
-- （阶段是「一段时间里做的事」，期间通常换过好几次应用）。
-- 只留一个 activity_id 会逼着总结侧从别处猜阶段里发生了什么 ——
-- 那正是「有阶段但总结说不清做了什么」的来源。
--
-- 与 activity_observations 同构：关联单独一张表，投影时整份重写。

CREATE TABLE IF NOT EXISTS stage_activities (
  stage_id    TEXT NOT NULL REFERENCES stages(id) ON DELETE CASCADE,
  activity_id TEXT NOT NULL,
  position    INTEGER NOT NULL,
  PRIMARY KEY (stage_id, activity_id)
);

CREATE INDEX IF NOT EXISTS idx_stage_activities_activity ON stage_activities(activity_id);
