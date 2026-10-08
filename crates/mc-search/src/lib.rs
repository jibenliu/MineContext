//! `mc-search` — 混合检索。
//!
//! 「我今天上午做了什么？」「APEX-389 我昨天查到哪里了？」—— 回答这类问题的
//! 第一步是**找得到**。三条设计约束：
//! 1. **关键词与语义都要用**：精确 ID 靠关键词（模糊匹配会把 `APEX-390` 排进来），
//!    同义改写靠向量；
//! 2. **过滤是一等公民**：时间、provenance、隐私 —— 隐私是**底线**；
//! 3. **降级必须可用**：没有向量后端时纯关键词照常工作。
//!
//! 这一层是纯函数（不碰 IO），没有数据库、网络与模型也能测干净所有分支。

pub mod batch;
pub mod keyword;
pub mod vector;

pub use batch::plan_batches;
pub use vector::{Embedding, EmbeddingSpace, VectorIndex};

use mc_common::time::Timestamp;
use mc_domain::activity::Provenance;
use serde::{Deserialize, Serialize};

/// 可被检索的东西。活动、总结、文档共用一种形状 ——
/// 检索不该因为来源不同而用不同的排序规则。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub id: String,
    pub kind: DocumentKind,
    pub text: String,
    pub at: Timestamp,
    pub provenance: Provenance,
    /// 被隐私规则拦下的内容：**永远不参与检索**
    pub blocked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentKind {
    Activity,
    Summary,
    Observation,
    Document,
}

impl DocumentKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Activity => "activity",
            Self::Summary => "summary",
            Self::Observation => "observation",
            Self::Document => "document",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchFilters {
    pub from: Option<Timestamp>,
    pub to: Option<Timestamp>,
    /// 只看「亲眼所见 / 规则判定」的内容，排除模型推测
    pub observed_only: bool,
    pub kinds: Vec<DocumentKind>,
    /// 保留字段：**不解锁**被拦截内容（隐私是底线，不是开关）。
    /// 它的存在只是为了让「想放开」的调用方明确失败，而不是以为默认就能拿到。
    pub include_blocked: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchQuery {
    pub text: String,
    pub filters: SearchFilters,
    pub limit: usize,
}

/// 融合权重。默认偏向关键词一点点：本地场景下精确匹配更常用。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScoreFusion {
    pub keyword_weight: f32,
    pub semantic_weight: f32,
}

impl Default for ScoreFusion {
    fn default() -> Self {
        Self {
            keyword_weight: 0.6,
            semantic_weight: 0.4,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub document: Document,
    pub score: f32,
    pub keyword_score: f32,
    pub semantic_score: f32,
}

/// 混合检索。
///
/// `vectors` 为 `None` 时退化为纯关键词检索（5.16）。
pub fn hybrid_search(
    documents: &[Document],
    query: &SearchQuery,
    vectors: Option<(&VectorIndex, &Embedding)>,
    fusion: ScoreFusion,
) -> Vec<Hit> {
    let needle = query.text.trim();
    if needle.is_empty() {
        // 空查询返回空而不是全部：把库倒出来既没用又危险
        return Vec::new();
    }

    // 语义分先算出来：没有向量后端时整列都是 0
    let semantic: std::collections::HashMap<String, f32> = match vectors {
        Some((index, embedding)) => index
            .search(embedding)
            .map(|scored| scored.into_iter().collect())
            .unwrap_or_default(),
        None => std::collections::HashMap::new(),
    };

    // 先算出「本次查询能达到的最好档次」：有精确命中时就只给精确命中，
    // 否则要求全词命中，最后才退回部分命中。
    // 这一步是「搜 APEX-389 不会混进 APEX-390」的关键。
    let best_tier = documents
        .iter()
        .filter(|document| !document.blocked)
        .filter(|document| passes_filters(document, &query.filters))
        .map(|document| keyword::classify(needle, &document.text))
        .min()
        .unwrap_or(keyword::MatchTier::None);

    let mut hits: Vec<Hit> = documents
        .iter()
        // 隐私：被拦截的内容在**进入打分之前**就被剔除。
        // 放在后面过滤容易漏（例如某个分支提前返回），放在这里没有例外路径。
        .filter(|document| !document.blocked)
        .filter(|document| passes_filters(document, &query.filters))
        .filter_map(|document| {
            let keyword_score = keyword::score(needle, &document.text);
            let semantic_score = semantic.get(&document.id).copied().unwrap_or(0.0);

            let tier = keyword::classify(needle, &document.text);

            // 两边都没命中 → 不返回（否则「搜什么都有一堆结果」）
            if keyword_score <= 0.0 && semantic_score <= 0.0 {
                return None;
            }
            // 关键词只认「本次能达到的最好档次」；语义命中不受此限
            // （同义改写本来就搜不到关键词，那正是向量的价值）
            if keyword_score > 0.0 && tier != best_tier && semantic_score <= 0.0 {
                return None;
            }

            let score =
                keyword_score * fusion.keyword_weight + semantic_score * fusion.semantic_weight;

            Some(Hit {
                document: document.clone(),
                score,
                keyword_score,
                semantic_score,
            })
        })
        .collect();

    // 排序确定性：分数降序 → 时间降序 → id 升序。
    // 同分时最近发生的更有用；最后用 id 兜底，保证两次调用结果一致。
    hits.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| right.document.at.cmp(&left.document.at))
            .then_with(|| left.document.id.cmp(&right.document.id))
    });

    let limit = if query.limit == 0 { 10 } else { query.limit };
    hits.truncate(limit);
    hits
}

fn passes_filters(document: &Document, filters: &SearchFilters) -> bool {
    if let Some(from) = filters.from {
        if document.at < from {
            return false;
        }
    }
    if let Some(to) = filters.to {
        if document.at >= to {
            return false;
        }
    }
    if filters.observed_only && document.provenance.is_inferred() {
        return false;
    }
    if !filters.kinds.is_empty() && !filters.kinds.contains(&document.kind) {
        return false;
    }
    true
}
