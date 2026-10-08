//! `mc-domain` — 纯领域层。
//!
//! **不依赖 tokio / rusqlite / reqwest / axum**（`scripts/check-dep-direction.sh` 强制）。
//! 这是「核心逻辑可极速测试」的前提：活动判定、防抖、阶段检测、总结模板
//! 全都是纯函数，因此可以用场景文件驱动，不需要网络、屏幕或模型。

pub mod activity;
pub mod model;
pub mod observation;
pub mod projector;
pub mod rules;
pub mod stage;
