//! 对话按 vault 根隔离：创建 / 列表过滤。

use mc_common::time::Timestamp;
use mc_storage::vaults::VaultUpsert;
use mc_storage::Database;

fn open() -> (tempfile::TempDir, Database) {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path().join("minecontext.db")).unwrap();
    (dir, db)
}

fn ms(value: i64) -> Timestamp {
    Timestamp::from_millis(value)
}

fn folder(title: &str) -> VaultUpsert {
    VaultUpsert {
        title: title.to_string(),
        summary: String::new(),
        content: String::new(),
        tags: vec![],
        parent_id: None,
        is_folder: true,
        document_type: "vaults".to_string(),
        sort_order: 0,
    }
}

#[test]
fn conversations_can_be_scoped_to_a_vault_root() {
    let (_dir, db) = open();
    let work = db.insert_vault_row(&folder("Work"), ms(1)).unwrap();
    let personal = db.insert_vault_row(&folder("Personal"), ms(2)).unwrap();

    let work_chat = db
        .create_conversation(Some("工作会话"), "assistant", Some(work), ms(3))
        .unwrap();
    let _personal_chat = db
        .create_conversation(Some("生活会话"), "assistant", Some(personal), ms(4))
        .unwrap();

    let work_only = db
        .list_conversations(Some("assistant"), Some(work))
        .unwrap();
    assert_eq!(work_only.len(), 1);
    assert_eq!(work_only[0].id, work_chat);
    assert_eq!(work_only[0].vault_id, Some(work));

    let all = db.list_conversations(Some("assistant"), None).unwrap();
    assert_eq!(all.len(), 2);
}

#[test]
fn vault_subtree_includes_nested_notes() {
    let (_dir, db) = open();
    let work = db.insert_vault_row(&folder("Work"), ms(1)).unwrap();
    let nested = db
        .insert_vault_row(
            &VaultUpsert {
                title: "nested".into(),
                summary: String::new(),
                content: String::new(),
                tags: vec![],
                parent_id: Some(work),
                is_folder: true,
                document_type: "vaults".to_string(),
                sort_order: 0,
            },
            ms(2),
        )
        .unwrap();
    let note = db
        .insert_vault_row(
            &VaultUpsert {
                title: "spec".into(),
                summary: String::new(),
                content: "work secret".into(),
                tags: vec![],
                parent_id: Some(nested),
                is_folder: false,
                document_type: "vaults".to_string(),
                sort_order: 0,
            },
            ms(3),
        )
        .unwrap();

    let ids = db.vault_subtree_ids(work).unwrap();
    assert!(ids.contains(&work));
    assert!(ids.contains(&nested));
    assert!(ids.contains(&note));
    assert_eq!(db.list_vault_roots().unwrap().len(), 1);
}
