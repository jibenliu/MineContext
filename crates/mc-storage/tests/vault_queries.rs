//! 笔记树（`vaults`）的读写。
//!
//! 渲染层的 `VaultTree` 走 `database:get-all-vaults` 等渠道，现在由 daemon 提供，行形状必须**逐列对齐**：
//! - `is_folder` / `is_deleted` 是 **0/1 数字**（不是布尔的 `true/false`）；
//! - `tags` 是逗号分隔字符串（不是 JSON 数组）；
//! - `parent_id` 允许为 `null`（根节点）；
//! - 默认查询**排除已删除**（带 `is_deleted = 0`）。
//!
//! 不这样对齐的话，笔记树会显示成一棵空树或所有节点都是文件夹。

use mc_common::time::Timestamp;
use mc_storage::vaults::{VaultPatch, VaultQuery, VaultUpsert};
use mc_storage::Database;

fn open() -> (tempfile::TempDir, Database) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("minecontext.db");
    let db = Database::open(&path).unwrap();
    (dir, db)
}

fn ms(value: i64) -> Timestamp {
    Timestamp::from_millis(value)
}

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

fn note(title: &str, document_type: &str) -> VaultUpsert {
    VaultUpsert {
        title: title.to_string(),
        summary: format!("{title} 的摘要"),
        content: format!("{title} 的正文"),
        tags: vec!["工作".to_string(), "日报".to_string()],
        parent_id: None,
        is_folder: false,
        document_type: document_type.to_string(),
        sort_order: 0,
    }
}

// ---------------------------------------------------------------- 行形状

#[test]
fn vault_rows_match_the_legacy_column_shape() {
    let (_dir, db) = open();
    db.insert_vault_row(&note("09-30 日报", "DailyReport"), ms(T0))
        .expect("插入笔记");

    let rows = db.query_vault_rows(&VaultQuery::default()).unwrap();
    assert_eq!(rows.len(), 1);

    let row = &rows[0];
    assert_eq!(row.title, "09-30 日报");
    assert_eq!(row.document_type, "DailyReport");
    assert_eq!(row.is_folder, 0, "is_folder 必须是 0/1 数字");
    assert_eq!(row.is_deleted, 0);
    assert_eq!(row.parent_id, None, "根节点的 parent_id 是 null");
    assert_eq!(row.tags, "工作,日报", "tags 是逗号分隔字符串");
    assert!(!row.created_at.is_empty(), "created_at 不能为空");
    assert!(!row.updated_at.is_empty(), "updated_at 不能为空");
}

// 迁移 0001 里已有的写入口（日报归档）写出来的行，也必须能被这张接口读到
#[test]
fn documents_written_by_the_summary_engine_are_visible() {
    let (_dir, db) = open();
    let folder = db.ensure_folder("Summary", ms(T0)).expect("建文件夹");
    db.upsert_vault_document(
        "DailyReport",
        "2026-09-30",
        "摘要",
        "正文",
        &["日报".to_string()],
        Some(folder),
        ms(T0),
    )
    .expect("写日报");

    let rows = db
        .query_vault_rows(&VaultQuery {
            document_type: vec!["DailyReport".to_string()],
            ..VaultQuery::default()
        })
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].parent_id, Some(folder));
}

// ---------------------------------------------------------------- 查询过滤

#[test]
fn queries_filter_by_type_folder_title_and_parent() {
    let (_dir, db) = open();
    let folder = db
        .insert_vault_row(
            &VaultUpsert {
                is_folder: true,
                ..note("Summary", "vaults")
            },
            ms(T0),
        )
        .unwrap();
    let _child = db
        .insert_vault_row(
            &VaultUpsert {
                parent_id: Some(folder),
                ..note("2026-09-30", "DailyReport")
            },
            ms(T0 + 1),
        )
        .unwrap();
    let _other = db
        .insert_vault_row(&note("随手记", "vaults"), ms(T0 + 2))
        .unwrap();

    // 按类型
    let reports = db
        .query_vault_rows(&VaultQuery {
            document_type: vec!["DailyReport".to_string()],
            ..VaultQuery::default()
        })
        .unwrap();
    assert_eq!(reports.len(), 1);

    // 只看文件夹（渲染层 `is_folder=1`）
    let folders = db
        .query_vault_rows(&VaultQuery {
            is_folder: Some(1),
            ..VaultQuery::default()
        })
        .unwrap();
    assert_eq!(folders.len(), 1);
    assert_eq!(folders[0].title, "Summary");

    // 按标题
    let by_title = db
        .query_vault_rows(&VaultQuery {
            title: Some("随手记".to_string()),
            ..VaultQuery::default()
        })
        .unwrap();
    assert_eq!(by_title.len(), 1);

    // 按父节点（含「根节点」这一档：`Some(None)` = parent_id IS NULL）
    let children = db
        .query_vault_rows(&VaultQuery {
            parent_id: Some(Some(folder)),
            ..VaultQuery::default()
        })
        .unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].title, "2026-09-30");

    let roots = db
        .query_vault_rows(&VaultQuery {
            parent_id: Some(None),
            ..VaultQuery::default()
        })
        .unwrap();
    assert_eq!(roots.len(), 2, "根节点是 Summary 与随手记");
}

// ---------------------------------------------------------------- 单行读取

#[test]
fn vault_by_id_returns_the_row_and_skips_deleted() {
    let (_dir, db) = open();
    let id = db
        .insert_vault_row(&note("随手记", "vaults"), ms(T0))
        .unwrap();

    let row = db.vault_row_by_id(id).unwrap().expect("必须能查到");
    assert_eq!(row.id, id);

    db.soft_delete_vault_row(id, ms(T0 + 1)).unwrap();
    assert!(
        db.vault_row_by_id(id).unwrap().is_none(),
        "已软删除的行不该被单行读取拿到（要带 is_deleted = 0）"
    );
    assert!(db
        .query_vault_rows(&VaultQuery::default())
        .unwrap()
        .is_empty());
}

#[test]
fn missing_id_is_none_not_an_error() {
    let (_dir, db) = open();
    assert!(db.vault_row_by_id(4242).unwrap().is_none());
}

// ---------------------------------------------------------------- 写入与更新

#[test]
fn insert_assigns_an_id_and_sensible_defaults() {
    let (_dir, db) = open();
    let id = db
        .insert_vault_row(
            &VaultUpsert {
                summary: String::new(),
                content: String::new(),
                tags: Vec::new(),
                ..note("空笔记", "vaults")
            },
            ms(T0),
        )
        .unwrap();
    assert!(id > 0);

    let row = db.vault_row_by_id(id).unwrap().unwrap();
    assert_eq!(row.is_folder, 0);
    assert_eq!(row.tags, "");
    assert_eq!(row.sort_order, 0);
}

#[test]
fn update_patch_touches_only_the_given_fields() {
    let (_dir, db) = open();
    let id = db
        .insert_vault_row(&note("原标题", "vaults"), ms(T0))
        .unwrap();

    let changes = db
        .update_vault_row(
            id,
            &VaultPatch {
                title: Some("新标题".to_string()),
                content: Some("新正文".to_string()),
                ..VaultPatch::default()
            },
            ms(T0 + 60_000),
        )
        .unwrap();
    assert_eq!(changes, 1);

    let row = db.vault_row_by_id(id).unwrap().unwrap();
    assert_eq!(row.title, "新标题");
    assert_eq!(row.content, "新正文");
    assert_eq!(row.summary, "原标题 的摘要", "没给的字段不能被清空");
    assert_eq!(row.tags, "工作,日报");
    assert!(
        row.updated_at > row.created_at || row.updated_at != row.created_at,
        "更新必须刷新 updated_at（不刷会让按 updated_at 排序错乱）"
    );
}

// 空 patch 是合法的（渲染层在没有字段可更新时直接返回 changes: 0）
#[test]
fn empty_patch_is_a_no_op() {
    let (_dir, db) = open();
    let id = db
        .insert_vault_row(&note("随手记", "vaults"), ms(T0))
        .unwrap();
    let changes = db
        .update_vault_row(id, &VaultPatch::default(), ms(T0 + 1))
        .unwrap();
    assert_eq!(changes, 0);
    assert_eq!(db.vault_row_by_id(id).unwrap().unwrap().title, "随手记");
}

// parent_id 必须能设回 NULL（把笔记拖到根目录）
#[test]
fn parent_can_be_cleared_back_to_null() {
    let (_dir, db) = open();
    let folder = db
        .insert_vault_row(
            &VaultUpsert {
                is_folder: true,
                ..note("文件夹", "vaults")
            },
            ms(T0),
        )
        .unwrap();
    let child = db
        .insert_vault_row(
            &VaultUpsert {
                parent_id: Some(folder),
                ..note("子笔记", "vaults")
            },
            ms(T0 + 1),
        )
        .unwrap();

    db.update_vault_row(
        child,
        &VaultPatch {
            parent_id: Some(None),
            ..VaultPatch::default()
        },
        ms(T0 + 2),
    )
    .unwrap();

    assert_eq!(db.vault_row_by_id(child).unwrap().unwrap().parent_id, None);
}

// ---------------------------------------------------------------- 删除

#[test]
fn soft_delete_and_restore_round_trip() {
    let (_dir, db) = open();
    let id = db
        .insert_vault_row(&note("随手记", "vaults"), ms(T0))
        .unwrap();

    assert_eq!(db.soft_delete_vault_row(id, ms(T0 + 1)).unwrap(), 1);
    assert!(db
        .query_vault_rows(&VaultQuery::default())
        .unwrap()
        .is_empty());

    // 显式查已删除的（渲染层的回收站视图）
    let deleted = db
        .query_vault_rows(&VaultQuery {
            is_deleted: Some(1),
            ..VaultQuery::default()
        })
        .unwrap();
    assert_eq!(deleted.len(), 1);

    assert_eq!(db.restore_vault_row(id, ms(T0 + 2)).unwrap(), 1);
    assert_eq!(
        db.query_vault_rows(&VaultQuery::default()).unwrap().len(),
        1
    );
}

#[test]
fn hard_delete_removes_the_row_for_good() {
    let (_dir, db) = open();
    let id = db
        .insert_vault_row(&note("随手记", "vaults"), ms(T0))
        .unwrap();

    assert_eq!(db.hard_delete_vault_row(id).unwrap(), 1);
    assert_eq!(db.hard_delete_vault_row(id).unwrap(), 0, "重复删除是 0 行");
    assert!(db
        .query_vault_rows(&VaultQuery {
            is_deleted: Some(1),
            ..VaultQuery::default()
        })
        .unwrap()
        .is_empty());
}

// 删除文件夹不该悄悄带走子笔记（裸 DELETE，子行的 parent_id 变成悬空）
#[test]
fn deleting_a_folder_keeps_children_addressed_by_a_stale_id() {
    let (_dir, db) = open();
    let folder = db
        .insert_vault_row(
            &VaultUpsert {
                is_folder: true,
                ..note("文件夹", "vaults")
            },
            ms(T0),
        )
        .unwrap();
    let child = db
        .insert_vault_row(
            &VaultUpsert {
                parent_id: Some(folder),
                ..note("子笔记", "vaults")
            },
            ms(T0 + 1),
        )
        .unwrap();

    db.hard_delete_vault_row(folder).unwrap();

    // 子笔记仍在，parent_id 指向一个不存在的行 —— 这正是的行为。
    // 记录下来是为了让「孤儿笔记」这件事可见，而不是假装它不存在。
    let row = db.vault_row_by_id(child).unwrap().expect("子笔记不该被删");
    assert_eq!(row.parent_id, Some(folder));
}
