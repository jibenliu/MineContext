//! 派生表投影器。
//!
//! 每个投影器都是「读事件 → 纯函数算出派生数据 → 在一个事务里覆盖写入 + 推进位点」。
//! 派生数据永远可以丢弃重算，因此这里不需要（也不允许）做增量补丁式更新。

pub mod activities;
pub mod stages;
pub mod summaries;
