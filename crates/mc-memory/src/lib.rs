//! `mc-memory` — 记忆的组织层。
//!
//! `mc-summary` 负责「怎么产出一份总结」；
//! 这里负责「把一段时间收成报告」并把它们**归档到用户能找到的地方**
//! （界面上的笔记树）；实体与线索（Thread）也长在这一层。

pub mod entity;
pub mod thread;
pub mod vault_writer;
pub mod weekly;
