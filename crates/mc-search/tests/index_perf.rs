//! 索引构建必须能撑住真实规模。
//!
//! 基准（`tests/bench.rs`）量到一件计划外的事：10 万条 × 768 维的**建索引**
//! 一次构建用了 41 秒，比它自己的检索 p50（145ms）离谱得多。原因是
//! `VectorIndex::insert` 每次都用线性扫描找同 id 的旧向量 —— 整体 O(n²)。
//!
//! 这不只是"慢"：索引构建跑在后台任务里，40 秒的纯 CPU 会让
//! 「刚开机就索引一整年历史」变成一个明显卡顿的事件。
//! 修法是把去重从线性扫描换成哈希索引，检索顺序（分数降序 + id 升序）
//! 必须保持不变 —— 那部分是确定性要求，不能因为换数据结构而漂移。

use std::time::{Duration, Instant};

use mc_search::{Embedding, EmbeddingSpace, VectorIndex};

fn vector(seed: usize) -> Vec<f32> {
    (0..64)
        .map(|i| ((seed * 31 + i) % 97) as f32 / 97.0)
        .collect()
}

// 插入 2 万条必须远远快于 O(n²)。
//
// 实测：O(n²) 版本在 1 万条时已经要 448ms，2 万条约 1.8s
// （`decisions/vector-scale.md` 有原始数字）。哈希去重是毫秒级，
// 因此 500ms 的门槛留了 ~50 倍余量：机器再忙也不会误报，
// 而一旦有人把去重改回线性扫描就会立刻失败。
#[ignore = "基准，按需运行：--ignored --nocapture"]
#[test]
fn inserting_many_vectors_is_not_quadratic() {
    let mut index = VectorIndex::new(EmbeddingSpace::new(64));

    let started = Instant::now();
    for doc in 0..20_000 {
        index
            .insert(&format!("doc-{doc}"), Embedding::new(vector(doc)))
            .expect("维度一致");
    }
    let elapsed = started.elapsed();

    assert_eq!(index.len(), 20_000);
    assert!(
        elapsed < Duration::from_millis(500),
        "插入 2 万条耗时 {elapsed:?}：索引插入退化成 O(n²) 了（见 decisions/vector-scale.md）"
    );
}

// 覆盖写：同一个 id 再插入是替换，不是追加，且**顺序不变**
#[ignore = "基准，按需运行：--ignored --nocapture"]
#[test]
fn replacing_a_vector_keeps_a_single_entry() {
    let mut index = VectorIndex::new(EmbeddingSpace::new(2));
    index.insert("a", Embedding::new(vec![1.0, 0.0])).unwrap();
    index.insert("b", Embedding::new(vec![0.0, 1.0])).unwrap();
    index.insert("a", Embedding::new(vec![0.0, 1.0])).unwrap();

    assert_eq!(index.len(), 2);
    let hits = index.search(&Embedding::new(vec![0.0, 1.0])).unwrap();
    // a 与 b 现在同分 → 按 id 升序
    assert_eq!(hits[0].0, "a");
    assert_eq!(hits[1].0, "b");
    assert!((hits[0].1 - 1.0).abs() < 1e-6);
}

// 删除后再插入同一个 id 必须恢复且只出现一次（哈希索引最容易在这里出错）
#[ignore = "基准，按需运行：--ignored --nocapture"]
#[test]
fn remove_then_reinsert_yields_one_entry() {
    let mut index = VectorIndex::new(EmbeddingSpace::new(2));
    index.insert("a", Embedding::new(vec![1.0, 0.0])).unwrap();
    index.remove("a");
    assert!(index.is_empty());

    index.insert("a", Embedding::new(vec![0.0, 1.0])).unwrap();
    assert_eq!(index.len(), 1);

    let hits = index.search(&Embedding::new(vec![0.0, 1.0])).unwrap();
    assert_eq!(hits.len(), 1);
    assert!((hits[0].1 - 1.0).abs() < 1e-6);
}

// 顺序无关性：同样的集合，无论插入顺序如何，检索结果（含并列）都必须一致
#[ignore = "基准，按需运行：--ignored --nocapture"]
#[test]
fn search_order_does_not_depend_on_insert_order() {
    let ids = ["c", "a", "b", "d"];
    let values = |id: &str| match id {
        // c 与 d 故意同分
        "c" | "d" => vec![1.0, 0.0],
        _ => vec![0.0, 1.0],
    };

    let mut forward = VectorIndex::new(EmbeddingSpace::new(2));
    for id in ids {
        forward.insert(id, Embedding::new(values(id))).unwrap();
    }

    let mut backward = VectorIndex::new(EmbeddingSpace::new(2));
    for id in ids.iter().rev() {
        backward.insert(id, Embedding::new(values(id))).unwrap();
    }

    let query = Embedding::new(vec![1.0, 0.0]);
    assert_eq!(
        forward.search(&query).unwrap(),
        backward.search(&query).unwrap()
    );
}
