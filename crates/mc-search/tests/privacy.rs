//! **隐私是不可检索的**（属性测试）。
//!
//! 单条用例只能证明「这个例子里没漏」，而隐私要求的恰恰是「任何情况下都不漏」。
//! 因此这里用属性测试：任意文档集合 × 任意查询文本 × 任意过滤参数，
//! 被拦截的内容都不能出现在结果里。
//!
//! `include_blocked` 这个字段的存在只是为了**让想放开的人明确失败** ——
//! 它不是开关。测试里会显式把它设成 true，验证仍然拿不到被拦截内容。

use mc_common::time::Timestamp;
use mc_domain::activity::Provenance;
use mc_search::{hybrid_search, Document, DocumentKind, ScoreFusion, SearchFilters, SearchQuery};
use mc_testkit::fixtures::FIXTURE_EPOCH_MS;
use proptest::prelude::*;

fn document(id: &str, text: String, blocked: bool, offset: i64) -> Document {
    Document {
        id: id.to_string(),
        kind: DocumentKind::Activity,
        text,
        at: Timestamp::from_millis(FIXTURE_EPOCH_MS + offset * 1000),
        provenance: if offset % 2 == 0 {
            Provenance::Observed
        } else {
            Provenance::Inferred {
                model: "stub".to_string(),
            }
        },
        blocked,
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn blocked_documents_are_never_searchable(
        texts in proptest::collection::vec("[\\PC]{0,30}", 0..8),
        blocked_flags in proptest::collection::vec(any::<bool>(), 0..8),
        query in "[\\PC]{0,12}",
        include_blocked in any::<bool>(),
        observed_only in any::<bool>(),
        limit in 0usize..20,
    ) {
        // 把两个随机长度对齐
        let count = texts.len().min(blocked_flags.len());
        let documents: Vec<Document> = (0..count)
            .map(|index| document(&format!("doc-{index}"), texts[index].clone(), blocked_flags[index], index as i64))
            .collect();
        let blocked_ids: Vec<&str> = documents.iter().filter(|d| d.blocked).map(|d| d.id.as_str()).collect();

        let search = SearchQuery {
            text: query,
            filters: SearchFilters {
                include_blocked,
                observed_only,
                ..SearchFilters::default()
            },
            limit,
        };
        let hits = hybrid_search(&documents, &search, None, ScoreFusion::default());

        for hit in &hits {
            prop_assert!(
                !hit.document.blocked,
                "被拦截的 {} 出现在了结果里（query={:?}）",
                hit.document.id,
                search.text
            );
            prop_assert!(!blocked_ids.contains(&hit.document.id.as_str()));
        }
    }

    #[test]
    fn results_are_a_subset_of_the_input_documents(
        texts in proptest::collection::vec("[\\PC]{0,20}", 1..6),
        query in "[\\PC]{1,8}",
    ) {
        let documents: Vec<Document> = texts
            .iter()
            .enumerate()
            .map(|(index, text)| document(&format!("doc-{index}"), text.clone(), false, index as i64))
            .collect();
        let ids: Vec<&str> = documents.iter().map(|d| d.id.as_str()).collect();

        let hits = hybrid_search(
            &documents,
            &SearchQuery { text: query, filters: SearchFilters::default(), limit: 10 },
            None,
            ScoreFusion::default(),
        );

        // 检索不能凭空造结果
        for hit in &hits {
            prop_assert!(ids.contains(&hit.document.id.as_str()));
        }
        // 分数必须落在 [0, 1]
        for hit in &hits {
            prop_assert!(hit.score >= 0.0 && hit.score <= 1.0, "分数越界：{}", hit.score);
        }
    }
}
