//! 混合检索（关键词 + 向量 + 过滤）。
//!
//! 检索是「从记录到能问」的第一跳。做错的代价很具体：
//! - 查不到（`APEX-389` 这种精确 ID 都搜不到）；
//! - 查出一堆无关的（只按关键词，语义相近的漏掉）；
//! - **把隐私拦截的内容搜出来**（这是硬性阻断项）。

use mc_common::time::Timestamp;
use mc_domain::activity::Provenance;
use mc_search::{
    hybrid_search, Document, DocumentKind, Embedding, EmbeddingSpace, ScoreFusion, SearchFilters,
    SearchQuery, VectorIndex,
};

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

fn at(offset_secs: i64) -> Timestamp {
    Timestamp::from_millis(T0 + offset_secs * 1_000)
}

fn doc(id: &str, kind: DocumentKind, text: &str, offset_secs: i64) -> Document {
    Document {
        id: id.to_string(),
        kind,
        text: text.to_string(),
        at: at(offset_secs),
        provenance: Provenance::Observed,
        blocked: false,
    }
}

fn corpus() -> Vec<Document> {
    vec![
        doc(
            "act-1",
            DocumentKind::Activity,
            "排查 APEX-389 的验收标准",
            0,
        ),
        doc(
            "act-2",
            DocumentKind::Activity,
            "修复登录接口的超时问题",
            3600,
        ),
        doc(
            "sum-1",
            DocumentKind::Summary,
            "上午主要在重构活动引擎与缓存层",
            7200,
        ),
        doc(
            "act-3",
            DocumentKind::Activity,
            "APEX-390 的需求澄清会议",
            10_800,
        ),
    ]
}

fn query(text: &str) -> SearchQuery {
    SearchQuery {
        text: text.to_string(),
        filters: SearchFilters::default(),
        limit: 10,
    }
}

// 精确 ID 必须一次命中
#[test]
fn keyword_search_finds_exact_issue_id() {
    let hits = hybrid_search(&corpus(), &query("APEX-389"), None, ScoreFusion::default());

    assert!(!hits.is_empty(), "精确 ID 必须能搜到");
    assert_eq!(hits[0].document.id, "act-1");
    assert!(
        hits.iter()
            .all(|hit| hit.document.text.contains("APEX-389")),
        "不该把无关文档排在前面：{hits:?}"
    );
}

// 语义近似（同义改写）靠向量命中
#[test]
fn semantic_search_finds_a_paraphrase() {
    let documents = corpus();
    let space = EmbeddingSpace::new(3);
    let mut index = VectorIndex::new(space);

    // 用手指定的「语义空间」表示：前两个维度是「引擎/重构」与「会议/需求」
    index
        .insert("sum-1", Embedding::new(vec![0.95, 0.05, 0.0]))
        .unwrap();
    index
        .insert("act-3", Embedding::new(vec![0.05, 0.95, 0.0]))
        .unwrap();

    let query_vector = Embedding::new(vec![0.9, 0.1, 0.0]);
    let hits = hybrid_search(
        &documents,
        &query("重写核心逻辑"),
        Some((&index, &query_vector)),
        ScoreFusion::default(),
    );

    assert_eq!(
        hits[0].document.id, "sum-1",
        "语义最接近的那条要排第一（关键词一个都没命中）：{hits:?}"
    );
    assert!(
        hits[0].semantic_score > hits[0].keyword_score,
        "这一条应当主要靠语义分进来"
    );
}

// 融合：关键词与语义都要影响排序
#[test]
fn hybrid_merges_and_reranks() {
    let documents = corpus();
    let index = {
        let mut index = VectorIndex::new(EmbeddingSpace::new(2));
        index
            .insert("sum-1", Embedding::new(vec![0.9, 0.1]))
            .unwrap();
        index
            .insert("act-2", Embedding::new(vec![0.8, 0.2]))
            .unwrap();
        index
    };
    let query_vector = Embedding::new(vec![0.9, 0.1]);

    let keyword_only = hybrid_search(&documents, &query("重构"), None, ScoreFusion::default());
    let hybrid = hybrid_search(
        &documents,
        &query("重构"),
        Some((&index, &query_vector)),
        ScoreFusion::default(),
    );

    assert_eq!(
        keyword_only[0].document.id, "sum-1",
        "只按关键词时 sum-1 命中「重构」"
    );
    assert!(
        hybrid.iter().any(|hit| hit.document.id == "act-2"),
        "融合之后语义相近的 act-2 也该出现：{hybrid:?}"
    );
    assert_eq!(hybrid[0].document.id, "sum-1", "两边都强的仍然排第一");
}

// 时间过滤
#[test]
fn time_filter_is_applied() {
    let mut search = query("APEX");
    search.filters.from = Some(at(10_000));

    let hits = hybrid_search(&corpus(), &search, None, ScoreFusion::default());

    assert_eq!(hits.len(), 1, "只该剩下下午那条：{hits:?}");
    assert_eq!(hits[0].document.id, "act-3");
}

// provenance 过滤：只看「亲眼所见」或「包含推测」
#[test]
fn provenance_filter_works() {
    let mut documents = corpus();
    documents[0].provenance = Provenance::Inferred {
        model: "stub".to_string(),
    };

    let mut only_observed = query("APEX");
    only_observed.filters.observed_only = true;

    let hits = hybrid_search(&documents, &only_observed, None, ScoreFusion::default());
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].document.id, "act-3");

    let all = hybrid_search(&documents, &query("APEX"), None, ScoreFusion::default());
    assert_eq!(all.len(), 2, "默认包含推测出来的内容：{all:?}");
}

// 隐私：被拦截的内容不可检索（硬性阻断项）
#[test]
fn search_never_returns_blocked_content() {
    let mut documents = corpus();
    documents[0].blocked = true;

    let hits = hybrid_search(&corpus()[..0], &query("APEX"), None, ScoreFusion::default());
    assert!(hits.is_empty(), "空语料返回空");

    let hits = hybrid_search(&documents, &query("APEX-389"), None, ScoreFusion::default());
    assert!(
        hits.iter().all(|hit| hit.document.id != "act-1"),
        "被隐私规则拦下的内容绝不能出现在结果里：{hits:?}"
    );

    // 即使调用方显式要求「包含被拦截内容」也不放开 —— 这不是开关，是底线
    let mut explicit = query("APEX-389");
    explicit.filters.include_blocked = true;
    let hits = hybrid_search(&documents, &explicit, None, ScoreFusion::default());
    assert!(
        hits.iter().all(|hit| !hit.document.blocked),
        "没有任何参数能让被拦截内容被检索到：{hits:?}"
    );
}

// 没有向量后端时降级为纯关键词，主链路仍可用
#[test]
fn search_degrades_to_keyword_without_vectors() {
    let hits = hybrid_search(&corpus(), &query("APEX-389"), None, ScoreFusion::default());

    assert_eq!(hits.len(), 1);
    assert!(hits[0].semantic_score == 0.0, "没有向量时语义分为 0");
    assert!(hits[0].score > 0.0);
}

// 空查询不报错，也不返回全部（那等于把库倒出来）
#[test]
fn empty_query_returns_empty_not_everything() {
    for text in ["", "   ", "\n\t"] {
        let hits = hybrid_search(&corpus(), &query(text), None, ScoreFusion::default());
        assert!(hits.is_empty(), "空查询 `{text:?}` 应当返回空：{hits:?}");
    }
}

// 维度不匹配要给出明确错误与重建建议（旧 bug 回归：不允许硬编码维度）
#[test]
fn embedding_dimension_mismatch_is_rejected_with_clear_error() {
    let mut index = VectorIndex::new(EmbeddingSpace::new(3));
    index
        .insert("a", Embedding::new(vec![1.0, 0.0, 0.0]))
        .unwrap();

    let error = index
        .insert("b", Embedding::new(vec![1.0, 0.0]))
        .expect_err("维度不一致必须报错");
    assert!(error.contains("2"), "报错要说明实际维度：{error}");
    assert!(error.contains("3"), "报错要说明索引维度：{error}");
    assert!(
        error.contains("重建") || error.contains("reindex"),
        "要给出可执行建议：{error}"
    );

    let error = index
        .search(&Embedding::new(vec![1.0]))
        .expect_err("查询向量维度不一致也要报错");
    assert!(
        error.contains("重建") || error.contains("reindex"),
        "{error}"
    );
}

// 维度来自 provider 返回的向量长度，不硬编码
#[test]
fn embedding_dimension_comes_from_the_vector_itself() {
    for dimensions in [3usize, 64, 1536] {
        let embedding = Embedding::new(vec![0.1; dimensions]);
        assert_eq!(embedding.dimensions(), dimensions);
        assert_eq!(
            EmbeddingSpace::from_embedding(&embedding).dimensions(),
            dimensions
        );
    }
}

// 排序必须确定性：同分数时按时间倒序（最近发生的更有用），再按 id
#[test]
fn ordering_is_deterministic_and_favours_recent() {
    let documents = vec![
        doc("old", DocumentKind::Activity, "APEX-389 上午", 0),
        doc("new", DocumentKind::Activity, "APEX-389 下午", 7200),
    ];

    let first = hybrid_search(&documents, &query("APEX-389"), None, ScoreFusion::default());
    let second = hybrid_search(&documents, &query("APEX-389"), None, ScoreFusion::default());

    assert_eq!(first, second, "同样输入必须同样输出");
    assert_eq!(first[0].document.id, "new", "同分时最近发生的排前面");
}
