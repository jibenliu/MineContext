//! `mc-storage` — 事件存储（唯一真源）、SQLite schema/迁移、兼容层。
//!
//! 设计要点：
//! - 原始事实（events/observations）不可变，派生数据（activities/stages/summaries）可重建
//! - 兼容层列形状与调用方预期一致，现有界面零改动
//! - 单写者串行化，避免跨进程锁竞争

pub mod blob;
pub mod chat;
pub mod db;
pub mod entities;
pub mod error;
pub mod events;
pub mod failures;
pub mod heatmap;
pub mod import;
pub mod jobs;
pub mod migrate;
pub mod monitoring;
pub mod observations;
pub mod projectors;
pub mod provider_calls;
pub mod retention;
pub mod settings;
pub mod todos;
pub mod vaults;
pub mod vectors;

pub use db::{AccessMode, Database};
pub use events::{EventEnvelope, NewEvent, EVENT_SCHEMA_VERSION};
