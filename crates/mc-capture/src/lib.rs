//! `mc-capture` — 采集源与变化检测。
//!
//! 在 macOS 上稳定、低开销地记录「我看到了什么」，并且**不把每一帧都送去分析**。
//!
//! 分两层：
//! - [`change`]：纯函数的变化检测（决定要不要花钱）
//! - `platform`：具体采集实现（macOS 屏幕/窗口），接口用 trait 隔离

pub mod change;
pub mod clipboard;
pub mod composite;
pub mod geometry;
pub mod permission_resolve;
pub mod platform;
pub mod scheduler;
pub mod source;
pub mod thumbnail;
pub mod window_filter;
