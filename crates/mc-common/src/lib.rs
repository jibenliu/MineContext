//! `mc-common` — 跨 crate 的基础类型：UTC 时间戳、错误分类、ID。
//!
//! 设计约束：
//! 领域层唯一允许的时间表示是 [`time::Timestamp`]（UTC 毫秒）。
//! 时区只在「展示 / 日边界计算」处按 IANA 名称转换。
//!
//! 为什么不是别的方案：时间事故的根因是 naive 与
//! timezone-aware 的 `datetime` 在同一个表达式里比较。只要领域层拿不到
//! 第二种时间表示，这类 bug 就在类型层面不可能出现。

pub mod diagnostic_pack;
pub mod disk;
pub mod error;
pub mod fs;
pub mod idle;
pub mod locale;
pub mod lock;
pub mod observability;
pub mod platform;
pub mod redact;
pub mod session;
pub mod time;
