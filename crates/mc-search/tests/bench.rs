//! 暴力向量检索的规模基准。
//!
//! **默认 `#[ignore]`**：它会真的分配几百 MB 内存并跑几十秒，不该出现在每次提交的门禁里。
//!
//! ```bash
//! cargo test -p mc-search --test bench --release -- --ignored --nocapture
//! ```
//!
//! 结论写进 `docs/decisions/vector-scale.md`。之所以坚持先量再选型：`sqlite-vec` 要编译 C 扩展、
//! 处理平台差异与加载失败路径，如果暴力检索在本地规模上够用，这些复杂度就是纯负收益。

use std::time::Instant;

use mc_search::{Embedding, EmbeddingSpace, VectorIndex};

/// 确定性伪随机（LCG）。基准必须可复现：同样的规模必须给出同样的向量。
fn lcg(seed: &mut u64) -> f32 {
    *seed = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    // 取高 24 位映射到 [-1, 1)
    let value = ((*seed >> 40) & 0xFF_FFFF) as f32 / 8_388_608.0;
    value - 1.0
}

fn vector(dimensions: usize, seed: &mut u64) -> Vec<f32> {
    (0..dimensions).map(|_| lcg(seed)).collect()
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let index = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[index]
}

fn bench(size: usize, dimensions: usize, queries: usize) {
    let mut seed = 0x5EED_1234_5678_9ABC_u64;
    let mut index = VectorIndex::new(EmbeddingSpace::new(dimensions));

    let build_start = Instant::now();
    for doc in 0..size {
        index
            .insert(
                &format!("doc-{doc}"),
                Embedding::new(vector(dimensions, &mut seed)),
            )
            .expect("维度一致");
    }
    let build_ms = build_start.elapsed().as_millis();

    let query_vectors: Vec<Embedding> = (0..queries)
        .map(|_| Embedding::new(vector(dimensions, &mut seed)))
        .collect();

    let mut latencies = Vec::with_capacity(queries);
    for query in &query_vectors {
        let started = Instant::now();
        let hits = index.search(query).expect("维度一致");
        latencies.push(started.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(hits.len(), size, "暴力检索必须覆盖全部候选");
    }

    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!(
        "规模 {size:>9} × {dimensions:>4} 维：建索引 {build_ms:>6} ms · \
         p50 {:>7.2} ms · p95 {:>7.2} ms · max {:>7.2} ms",
        percentile(&latencies, 0.5),
        percentile(&latencies, 0.95),
        latencies.last().copied().unwrap_or_default(),
    );
}

// 三个规模档：本地一年量级、触发门槛、以及门槛之上
#[test]
#[ignore = "规模基准：需要几百 MB 内存，用 --release -- --ignored --nocapture 手动运行"]
fn bench_vector_search() {
    // 768 维是主流 embedding 的常见维度
    bench(10_000, 768, 20);
    bench(100_000, 768, 20);
    // 50 万 × 768 维需要约 1.5 GB，超出一般开发机的余量，
    // 因此用 384 维（约 0.77 GB）验证「线性增长」这个假设本身
    bench(500_000, 384, 10);
}
