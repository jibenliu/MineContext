//! 实体抽取与别名归并。
//!
//! 「APEX-389 我昨天查到哪里了？」能回答的前提是系统知道**同一个 ticket 在
//! 不同天出现过**，第一步就是从活动标题里认出实体。
//!
//! **确定性纯函数**：不调模型、不查库 —— 实体识别是检索与线索的底座，
//! 底座不该依赖可能失败的远程调用，而且纯函数可以被穷举测试。
//!
//! 认不出来的东西**不猜**：只认「有明确形态」的实体（工单号、文件路径）；
//! 猜出来的实体一旦写进线索，用户看到的「相关记录」就是假的。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    /// 工单 / issue：`APEX-389`
    Issue,
    /// 代码文件：`src/stage.rs`
    FileNote,
}

impl EntityKind {
    /// 落库用的稳定字符串。**改名会让历史实体关联查不出来**，
    /// 因此它与错误码一样属于对外契约（有专门的测试盯着）。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Issue => "issue",
            Self::FileNote => "file_note",
        }
    }

    pub fn from_storage(value: &str) -> Option<Self> {
        match value {
            "issue" => Some(Self::Issue),
            "file_note" => Some(Self::FileNote),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Entity {
    pub kind: EntityKind,
    /// 规范化后的名字（比较、归并用）
    pub canonical: String,
    /// 用户看到的样子（原文，不擅自改写）
    pub display: String,
}

/// 从文本里抽出实体。
pub fn extract(text: &str) -> Vec<Entity> {
    let mut entities: Vec<Entity> = Vec::new();

    for token in text.split(|ch: char| ch.is_whitespace() || "，。；：、（）【】「」".contains(ch))
    {
        let cleaned = token.trim_matches(|ch: char| {
            matches!(
                ch,
                ',' | '.' | ';' | ':' | '(' | ')' | '[' | ']' | '"' | '\'' | '#' | '*'
            )
        });
        if cleaned.len() < 3 {
            continue;
        }

        if let Some(entity) = as_issue(cleaned).or_else(|| as_file(cleaned)) {
            if !entities
                .iter()
                .any(|existing| existing.canonical == entity.canonical)
            {
                entities.push(entity);
            }
        }
    }

    entities
}

/// 工单号：`APEX-389`、`BUG-12`（字母开头 + 数字结尾，中间一个短横线）
fn as_issue(token: &str) -> Option<Entity> {
    let (prefix, suffix) = token.split_once('-')?;
    if prefix.len() < 2 || prefix.len() > 12 || !prefix.chars().all(|ch| ch.is_ascii_alphanumeric())
    {
        return None;
    }
    if !prefix.chars().next()?.is_ascii_uppercase() {
        return None;
    }
    if suffix.is_empty() || suffix.len() > 6 || !suffix.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }

    Some(Entity {
        kind: EntityKind::Issue,
        canonical: format!("{}-{}", prefix.to_ascii_uppercase(), suffix),
        display: token.to_string(),
    })
}

/// 代码文件：带常见扩展名的路径
fn as_file(token: &str) -> Option<Entity> {
    const EXTENSIONS: &[&str] = &[
        "rs", "ts", "tsx", "js", "jsx", "py", "go", "java", "kt", "swift", "c", "cc", "cpp", "h",
        "hpp", "sql", "toml", "yaml", "yml", "json", "md", "sh",
    ];

    let (path, extension) = token.rsplit_once('.')?;
    if path.is_empty() || !EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str()) {
        return None;
    }
    if path.len() < 2 {
        return None;
    }

    Some(Entity {
        kind: EntityKind::FileNote,
        // 文件按**小写路径**归并：macOS 默认大小写不敏感，
        // 同一个文件写成 `Src/Stage.rs` 与 `src/stage.rs` 是同一份
        canonical: token.to_ascii_lowercase(),
        display: token.to_string(),
    })
}

/// 别名表：把各种写法归并到同一个实体。
#[derive(Debug, Clone, Default)]
pub struct EntityRegistry {
    aliases: std::collections::HashMap<String, String>,
}

impl EntityRegistry {
    /// 默认表：把 `APEX-389` 的常见变体（大小写、空格、下划线）归并。
    pub fn with_defaults() -> Self {
        Self::default()
    }

    /// 注册一条别名：`alias` 与 `canonical` 指向同一个实体。
    pub fn merge_alias(mut self, alias: &str, canonical: &str) -> Self {
        self.aliases
            .insert(normalize_key(alias), canonical.to_string());
        self
    }

    /// 规范化一个名字。先查别名表，再退回通用规则（大小写与分隔符）。
    pub fn resolve(&self, name: &str) -> Entity {
        let key = normalize_key(name);

        if let Some(canonical) = self.aliases.get(&key) {
            return Entity {
                kind: kind_of(canonical),
                canonical: canonical.clone(),
                display: name.to_string(),
            };
        }

        // 通用规则：`apex 389` / `apex_389` / `apex-389` → `APEX-389`
        if let Some(canonical) = canonical_form(&key) {
            return Entity {
                kind: EntityKind::Issue,
                canonical,
                display: name.to_string(),
            };
        }

        Entity {
            kind: kind_of(name),
            canonical: key,
            display: name.to_string(),
        }
    }
}

/// 把名字压成比较用的键：小写 + 分隔符统一成 `-`。
fn normalize_key(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.trim().chars() {
        if ch == '_' || ch.is_whitespace() {
            out.push('-');
        } else {
            out.extend(ch.to_lowercase());
        }
    }
    out
}

/// 把 `apex-389` 形态的键还原成 `APEX-389`。
fn canonical_form(key: &str) -> Option<String> {
    let (prefix, suffix) = key.split_once('-')?;
    if prefix.is_empty()
        || suffix.is_empty()
        || !prefix.chars().all(|ch| ch.is_ascii_alphanumeric())
        || !suffix.chars().all(|ch| ch.is_ascii_digit())
    {
        return None;
    }
    Some(format!("{}-{}", prefix.to_ascii_uppercase(), suffix))
}

fn kind_of(name: &str) -> EntityKind {
    if name.contains('.') && as_file(name).is_some() {
        EntityKind::FileNote
    } else {
        EntityKind::Issue
    }
}
