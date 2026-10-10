-- 迁移 0003：活动补一列 original_title。
--
-- 用户改名后界面既要显示用户起的名字，也要能回答「算法原本认为这是什么」。
-- 没有这一列，改名就等于把算法的判断永久覆盖掉，事后无法复盘。

ALTER TABLE activities ADD COLUMN original_title TEXT;
