use mc_common::time::Timestamp;
use mc_search::{DocumentKind, SearchFilters};
use mc_server::retrieval::retrieve_filtered;
use mc_storage::vaults::VaultUpsert;
use mc_storage::Database;

#[test]
fn filtered_retrieval_does_not_read_unrequested_document_tables() {
    for (kind, omitted_tables) in [
        (
            DocumentKind::Document,
            "DROP TABLE activities; DROP TABLE summaries;",
        ),
        (
            DocumentKind::Activity,
            "DROP TABLE vaults; DROP TABLE summaries;",
        ),
        (
            DocumentKind::Summary,
            "DROP TABLE activities; DROP TABLE vaults;",
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let db = Database::open(directory.path().join("test.db")).unwrap();
        if kind == DocumentKind::Document {
            db.insert_vault_row(
                &VaultUpsert {
                    title: "needle".into(),
                    summary: String::new(),
                    content: "needle notes".into(),
                    tags: vec![],
                    parent_id: None,
                    is_folder: false,
                    document_type: "document".into(),
                    sort_order: 0,
                },
                Timestamp::from_millis(100),
            )
            .unwrap();
        }
        db.with_write(|connection| connection.execute_batch(omitted_tables))
            .unwrap();
        let hits = retrieve_filtered(
            &db,
            "needle",
            10,
            SearchFilters {
                kinds: vec![kind],
                ..Default::default()
            },
            None,
        )
        .expect("excluded document tables must not be read");
        if kind == DocumentKind::Document {
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].document.kind, kind);
        } else {
            assert!(hits.is_empty());
        }
        let selected_table = match kind {
            DocumentKind::Activity => "activities",
            DocumentKind::Summary => "summaries",
            DocumentKind::Document => "vaults",
            DocumentKind::Observation => unreachable!(),
        };
        db.with_write(|connection| {
            connection.execute_batch(&format!("DROP TABLE {selected_table}"))
        })
        .unwrap();
        assert!(retrieve_filtered(
            &db,
            "needle",
            10,
            SearchFilters {
                kinds: vec![kind],
                ..Default::default()
            },
            None,
        )
        .is_err());
    }
}
