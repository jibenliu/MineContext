//! 向量的持久化、恢复与可续重建。
//!
//! 三条设计约束：
//!
//! 1. **向量是派生数据**：删掉整个向量表不能影响主链路（事件/观测/活动照常），
//!    重新嵌入即可恢复。把向量与 Qdrant 绑死，索引一坏检索就整体不可用。
//! 2. **维度来自 provider，且写在库里**：换模型导致维度变化时必须**明确报错**
//!    并提示重建，而不是让写入悄悄失败、检索悄悄返回空。
//! 3. **重建可续**：几万条向量不可能一次成功，水位线（watermark）必须落库，
//!    中断后从断点继续，重复投递不会产生重复行。

use mc_common::error::ErrorCode;
use mc_common::time::Timestamp;
use mc_storage::observations::{NewObservation, ObservationQuery};
use mc_storage::vectors::{
    clear_vectors, count_vectors, declare_embedding_space, delete_vectors, embedding_space,
    load_vectors, rebuild_watermark, reset_vectors, set_rebuild_watermark, upsert_vectors,
    VectorRecord,
};
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

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as NOW;

fn record(kind: &str, doc_id: &str, model: &str, values: Vec<f32>) -> VectorRecord {
    VectorRecord {
        kind: kind.to_string(),
        doc_id: doc_id.to_string(),
        model: model.to_string(),
        values,
    }
}

/// 往库里放一条观测，用来验证「清空向量不影响主链路」。
fn seed_observation(db: &Database) {
    let outcome = db
        .insert_observation(&NewObservation {
            id: "obs-1".to_string(),
            ts: ms(NOW),
            source_id: "screen".to_string(),
            kind: "screen".to_string(),
            app_name: Some("Editor".to_string()),
            app_bundle_id: None,
            window_title: Some("stage.rs".to_string()),
            domain: None,
            display_id: None,
            scale_factor: None,
            image: None,
            text_content: Some("重构 stage 状态机".to_string()),
            text_origin: None,
            change_kind: "new".to_string(),
            privacy_verdict: "allowed".to_string(),
            phash: None,
            idempotency: "obs-1-key".to_string(),
        })
        .expect("观测必须能落库");
    assert!(outcome.inserted, "首次写入必须是新行");
}

// ---------------------------------------------------------------- 5.5 往返一致

#[test]
fn vectors_persisted_and_retrievable() {
    let (_dir, db) = open();

    let written = upsert_vectors(
        &db,
        &[
            record("activity", "act-1", "embed-small", vec![0.5, 0.25, -1.0]),
            record("activity", "act-2", "embed-small", vec![1.0, 0.0, 0.0]),
        ],
        ms(NOW),
    )
    .expect("写入向量必须成功");
    assert_eq!(written, 2);

    let loaded = load_vectors(&db, "activity").unwrap();
    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0].doc_id, "act-1");
    assert_eq!(loaded[0].model, "embed-small");
    assert_eq!(loaded[0].values, vec![0.5, 0.25, -1.0]);

    // 浮点往返必须是无损的（否则余弦相似度的排序会漂移）
    for original in [[0.1f32, 0.2, 0.3], [1.0 / 3.0, -2.5e-3, 7.0]] {
        upsert_vectors(
            &db,
            &[record("summary", "sum-1", "embed-small", original.to_vec())],
            ms(NOW),
        )
        .unwrap();
        let back = load_vectors(&db, "summary").unwrap();
        assert_eq!(back[0].values, original.to_vec(), "f32 往返必须精确");
    }

    assert_eq!(count_vectors(&db, "activity").unwrap(), 2);
    assert_eq!(count_vectors(&db, "summary").unwrap(), 1);
}

// 5.5b — 同一文档重复嵌入是**覆盖**，不是追加（否则检索会拿到旧向量）
#[test]
fn upsert_replaces_the_same_document_vector() {
    let (_dir, db) = open();

    upsert_vectors(
        &db,
        &[record("activity", "act-1", "embed-small", vec![1.0, 0.0])],
        ms(NOW),
    )
    .unwrap();
    upsert_vectors(
        &db,
        &[record("activity", "act-1", "embed-small", vec![0.0, 1.0])],
        ms(NOW + 1),
    )
    .unwrap();

    let loaded = load_vectors(&db, "activity").unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].values, vec![0.0, 1.0]);
}

// ---------------------------------------------------------------- 5.1/5.2 维度

// 5.1 — 维度由第一条真实向量决定，并落库（绝不硬编码）
#[test]
fn embedding_space_is_declared_by_the_first_write() {
    let (_dir, db) = open();
    assert!(embedding_space(&db).unwrap().is_none(), "空库没有维度声明");

    upsert_vectors(
        &db,
        &[record("activity", "act-1", "embed-small", vec![0.0; 1024])],
        ms(NOW),
    )
    .unwrap();

    let space = embedding_space(&db).unwrap().expect("写入后必须有维度声明");
    assert_eq!(space.dimensions, 1024);
    assert_eq!(space.model, "embed-small");
}

// 5.2 — 换模型导致维度变化：明确报错 + 建议重建，且**不写入半截数据**
#[test]
fn dimension_change_is_rejected_with_rebuild_hint() {
    let (_dir, db) = open();

    upsert_vectors(
        &db,
        &[record("activity", "act-1", "embed-a", vec![0.0; 1024])],
        ms(NOW),
    )
    .unwrap();

    let error = upsert_vectors(
        &db,
        &[record("activity", "act-2", "embed-b", vec![0.0; 768])],
        ms(NOW + 1),
    )
    .expect_err("维度变化必须被拒绝");

    assert_eq!(error.code(), ErrorCode::StorageEmbeddingDimensionMismatch);
    assert!(error.detail().contains("1024"), "详情要带上已存维度");
    assert!(error.detail().contains("768"), "详情要带上新维度");
    assert!(
        error.remediation().is_some(),
        "必须给出「重建索引」的可执行建议"
    );

    // 拒绝了就不能留下半截数据（否则检索会拿到维度错乱的向量）
    assert_eq!(count_vectors(&db, "activity").unwrap(), 1);
}

// 5.2b — 显式声明一个不同维度/模型的向量空间必须被拒绝（要重建就显式 reset）
#[test]
fn declaring_a_different_space_requires_an_explicit_reset() {
    let (_dir, db) = open();
    declare_embedding_space(&db, "embed-a", 1024, ms(NOW)).unwrap();

    let error = declare_embedding_space(&db, "embed-b", 768, ms(NOW + 1))
        .expect_err("改变向量空间必须显式 reset");
    assert_eq!(error.code(), ErrorCode::StorageEmbeddingDimensionMismatch);

    // 同一个空间重复声明是幂等的
    let again = declare_embedding_space(&db, "embed-a", 1024, ms(NOW + 2)).unwrap();
    assert_eq!(again.dimensions, 1024);
}

// ---------------------------------------------------------------- 5.7 可恢复

// 5.7 — 删掉向量存储 → 自动重建即可，主链路不受影响
#[test]
fn vector_store_missing_is_recoverable() {
    let (_dir, db) = open();
    seed_observation(&db);

    upsert_vectors(
        &db,
        &[record("activity", "act-1", "embed-a", vec![0.3, 0.4])],
        ms(NOW),
    )
    .unwrap();
    set_rebuild_watermark(&db, "activity", 42, ms(NOW)).unwrap();

    // 等价于用户删掉了 vectors.db：向量与空间声明都没了
    let removed = reset_vectors(&db).unwrap();
    assert_eq!(removed, 1);
    assert_eq!(count_vectors(&db, "activity").unwrap(), 0);
    assert!(embedding_space(&db).unwrap().is_none());
    assert_eq!(rebuild_watermark(&db, "activity").unwrap(), None);

    // 主链路（观测）完全不受影响
    assert_eq!(db.observation_count().unwrap(), 1);
    assert_eq!(
        db.query_observations(&ObservationQuery::default())
            .unwrap()
            .len(),
        1
    );

    // 重新嵌入时维度从新向量重新声明
    upsert_vectors(
        &db,
        &[record("activity", "act-1", "embed-b", vec![0.0; 8])],
        ms(NOW + 1),
    )
    .unwrap();
    assert_eq!(embedding_space(&db).unwrap().unwrap().dimensions, 8);
}

// 5.7b — 按 kind 清空（活动可重建，总结不动）
#[test]
fn clear_vectors_is_scoped_by_kind() {
    let (_dir, db) = open();
    upsert_vectors(
        &db,
        &[
            record("activity", "act-1", "embed-a", vec![1.0, 0.0]),
            record("summary", "sum-1", "embed-a", vec![0.0, 1.0]),
        ],
        ms(NOW),
    )
    .unwrap();

    assert_eq!(clear_vectors(&db, "activity").unwrap(), 1);
    assert_eq!(count_vectors(&db, "activity").unwrap(), 0);
    assert_eq!(count_vectors(&db, "summary").unwrap(), 1);
}

// 5.7c — 删除单个文档的向量
#[test]
fn delete_vectors_removes_one_document() {
    let (_dir, db) = open();
    upsert_vectors(
        &db,
        &[
            record("activity", "act-1", "embed-a", vec![1.0, 0.0]),
            record("activity", "act-2", "embed-a", vec![0.0, 1.0]),
        ],
        ms(NOW),
    )
    .unwrap();

    assert_eq!(delete_vectors(&db, "activity", "act-1").unwrap(), 1);
    assert_eq!(delete_vectors(&db, "activity", "act-1").unwrap(), 0);
    let loaded = load_vectors(&db, "activity").unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].doc_id, "act-2");
}

// ---------------------------------------------------------------- 5.6 可续重建

// 5.6 — 水位线落库并单调前进；重复投递同一文档不产生重复行
#[test]
fn vector_index_rebuild_is_resumable() {
    let (_dir, db) = open();
    assert_eq!(rebuild_watermark(&db, "activity").unwrap(), None);

    // 第一轮：索引到事件序号 1000 就被中断
    upsert_vectors(
        &db,
        &[record("activity", "act-1", "embed-a", vec![1.0, 0.0])],
        ms(NOW),
    )
    .unwrap();
    set_rebuild_watermark(&db, "activity", 1000, ms(NOW)).unwrap();
    assert_eq!(rebuild_watermark(&db, "activity").unwrap(), Some(1000));

    // 第二轮：从中断处继续 —— 已索引的文档再次出现也不会重复
    upsert_vectors(
        &db,
        &[
            record("activity", "act-1", "embed-a", vec![1.0, 0.0]),
            record("activity", "act-2", "embed-a", vec![0.0, 1.0]),
        ],
        ms(NOW + 1),
    )
    .unwrap();
    set_rebuild_watermark(&db, "activity", 2500, ms(NOW + 1)).unwrap();

    assert_eq!(count_vectors(&db, "activity").unwrap(), 2);
    assert_eq!(rebuild_watermark(&db, "activity").unwrap(), Some(2500));
}

// 5.6b — 水位线按 kind 独立（活动的进度不能被总结的重建覆盖）
#[test]
fn rebuild_watermarks_are_independent_per_kind() {
    let (_dir, db) = open();
    set_rebuild_watermark(&db, "activity", 10, ms(NOW)).unwrap();
    set_rebuild_watermark(&db, "summary", 99, ms(NOW)).unwrap();

    assert_eq!(rebuild_watermark(&db, "activity").unwrap(), Some(10));
    assert_eq!(rebuild_watermark(&db, "summary").unwrap(), Some(99));
}

// ---------------------------------------------------------------- 边界

// 边界：空向量无法比较相似度，写入时必须被拒绝（而不是存一条永远排最后的垃圾）
#[test]
fn empty_vector_is_rejected() {
    let (_dir, db) = open();
    let error = upsert_vectors(
        &db,
        &[record("activity", "act-1", "embed-a", Vec::new())],
        ms(NOW),
    )
    .expect_err("空向量必须被拒绝");
    assert_eq!(error.code(), ErrorCode::DomainInvalidRange);
    assert_eq!(count_vectors(&db, "activity").unwrap(), 0);
}

// 边界：空查询不报错，返回空（检索层会把它当「没有向量」处理）
#[test]
fn empty_store_returns_empty_without_error() {
    let (_dir, db) = open();
    assert!(load_vectors(&db, "activity").unwrap().is_empty());
    assert_eq!(count_vectors(&db, "activity").unwrap(), 0);
    assert_eq!(clear_vectors(&db, "activity").unwrap(), 0);
}
