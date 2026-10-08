use mc_common::time::Timestamp;
use mc_domain::activity::Provenance;
use mc_search::{hybrid_search, Document, DocumentKind, ScoreFusion, SearchFilters, SearchQuery};

fn document(id: &str, text: &str, at: i64) -> Document {
    Document {
        id: id.into(),
        text: text.into(),
        at: Timestamp::from_millis(at),
        kind: DocumentKind::Activity,
        provenance: Provenance::Observed,
        blocked: false,
    }
}

#[test]
fn excluded_exact_matches_do_not_hide_eligible_matches() {
    let eligible = document("eligible", "alpha separate beta", 100);
    for boundary in [0, 200] {
        let excluded = document("excluded", "alpha beta", boundary);
        assert_eligible(
            &[excluded, eligible.clone()],
            SearchFilters {
                from: Some(Timestamp::from_millis(100)),
                to: Some(Timestamp::from_millis(200)),
                ..Default::default()
            },
        );
    }
    let mut wrong_kind = document("wrong-kind", "alpha beta", 100);
    wrong_kind.kind = DocumentKind::Summary;
    assert_eligible(
        &[wrong_kind, eligible.clone()],
        SearchFilters {
            kinds: vec![DocumentKind::Activity],
            ..Default::default()
        },
    );
    let mut inferred = document("inferred", "alpha beta", 100);
    inferred.provenance = Provenance::Inferred {
        model: "test".into(),
    };
    assert_eligible(
        &[inferred, eligible.clone()],
        SearchFilters {
            observed_only: true,
            ..Default::default()
        },
    );
    let mut blocked = document("blocked", "alpha beta", 100);
    blocked.blocked = true;
    assert_eligible(
        &[blocked, eligible],
        SearchFilters {
            include_blocked: true,
            ..Default::default()
        },
    );
}

fn assert_eligible(documents: &[Document], filters: SearchFilters) {
    let hits = hybrid_search(
        documents,
        &SearchQuery {
            text: "alpha beta".into(),
            filters,
            limit: 1,
        },
        None,
        ScoreFusion::default(),
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].document.id, "eligible");
}
