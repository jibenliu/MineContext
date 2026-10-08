//! `mc-testkit` — 测试专用的假实现。
//!
//! 生产 crate 只在 `[dev-dependencies]` 中引用它。
//! 第一个成员是 [`clock::TestClock`]：TDD 在本项目能否成立，
//! 取决于「时间可注入」这条前提。

pub mod capture;
pub mod clock;
pub mod fixtures;
pub mod provider;
pub mod scenario;

pub use clock::TestClock;
