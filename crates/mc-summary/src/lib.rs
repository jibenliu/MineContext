//! `mc-summary` — 阶段总结与任意时段总结。
//!
//! 第一职责不是「总结写得好」，而是**「有阶段必有总结」在结构上成立**：
//! 正常路径（`StageClosed` → LLM → `quality=model`）、失败路径（重试仍失败 →
//! 确定性兜底 → `quality=fallback`）、巡检路径（定时扫描「已关闭但没有总结」→
//! 立刻补一条）三条路都通向一份非空总结。
//!
//! 第二职责是**任意时段总结**：拖选一段时间就能得到总结，与阶段总结共用引擎。
//! 本 crate 只依赖 `mc-common` 与 `mc-domain`：模板、兜底、渲染全是纯函数，
//! 因此可以在没有网络、没有模型的条件下确定性地测试。

pub mod adhoc;
pub mod daily;
pub mod fallback;
pub mod generator;
pub mod model;
pub mod patrol;
pub mod prompts;
pub mod source;
pub mod template;

pub use fallback::should_summarize;
