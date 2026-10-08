//! 向量与向量索引。
//!
//! 维度**完全由 provider 返回的向量长度决定** —— 写死维度会在换模型时
//! 直接失败或拿到错位的向量。
//! 因此这里没有常量维度，只有「索引建好之后维度就固定」这一条约束，
//! 且维度不一致时给出**可执行**的错误（重建索引）。
//!
//! 检索方式是**暴力余弦**：对本地规模（几万条文档）足够快，且零依赖；
//! 再大才需要专门的索引。

/// 一条向量。持有维度信息，避免到处传裸 `Vec<f32>`。
#[derive(Debug, Clone, PartialEq)]
pub struct Embedding {
    values: Vec<f32>,
}

impl Embedding {
    pub fn new(values: Vec<f32>) -> Self {
        Self { values }
    }

    pub fn values(&self) -> &[f32] {
        &self.values
    }

    pub fn dimensions(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

/// 向量空间：维度一旦确定就不再变化。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddingSpace {
    dimensions: usize,
}

impl EmbeddingSpace {
    pub const fn new(dimensions: usize) -> Self {
        Self { dimensions }
    }

    /// 从一条真实向量推断维度（**推荐路径**：维度来自 provider）。
    pub fn from_embedding(embedding: &Embedding) -> Self {
        Self::new(embedding.dimensions())
    }

    pub const fn dimensions(self) -> usize {
        self.dimensions
    }
}

/// 向量索引。`insert` / `search` 的维度必须与空间一致。
///
/// `entries` 保持插入顺序（检索顺序只由分数与 id 决定，与插入顺序无关），
/// `positions` 是 id → 下标的哈希索引。**不要**把去重改回线性扫描：
/// 那会让建索引退化成 O(n²)（实测 10 万条要 41 秒）。
#[derive(Debug, Clone, Default)]
pub struct VectorIndex {
    space: Option<EmbeddingSpace>,
    entries: Vec<(String, Embedding)>,
    positions: std::collections::HashMap<String, usize>,
}

impl VectorIndex {
    pub fn new(space: EmbeddingSpace) -> Self {
        Self {
            space: Some(space),
            entries: Vec::new(),
            positions: std::collections::HashMap::new(),
        }
    }

    /// 空索引：维度由第一条插入的向量决定。
    pub fn unbound() -> Self {
        Self::default()
    }

    pub fn space(&self) -> Option<EmbeddingSpace> {
        self.space
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn insert(&mut self, id: &str, embedding: Embedding) -> Result<(), String> {
        if embedding.is_empty() {
            return Err(format!("文档 {id} 的向量为空，无法索引"));
        }

        let space = match self.space {
            Some(space) => space,
            None => {
                let space = EmbeddingSpace::from_embedding(&embedding);
                self.space = Some(space);
                space
            }
        };

        if embedding.dimensions() != space.dimensions() {
            return Err(dimension_error(embedding.dimensions(), space.dimensions()));
        }

        match self.positions.get(id).copied() {
            Some(position) => self.entries[position].1 = embedding,
            None => {
                self.positions.insert(id.to_string(), self.entries.len());
                self.entries.push((id.to_string(), embedding));
            }
        }
        Ok(())
    }

    pub fn remove(&mut self, id: &str) {
        let Some(position) = self.positions.remove(id) else {
            return;
        };
        self.entries.remove(position);
        // 删除会让后面的下标整体前移，哈希索引必须跟着重建。
        // `remove` 不在热路径上（只有活动被合并/删除时才调用），
        // 因此这里用最直白的「整体重建」换正确性。
        self.reindex();
    }

    /// 下标变了之后重建哈希索引。
    fn reindex(&mut self) {
        self.positions.clear();
        for (position, (id, _)) in self.entries.iter().enumerate() {
            self.positions.insert(id.clone(), position);
        }
    }

    /// 按余弦相似度返回 `(id, 相似度)`，已按相似度降序。
    pub fn search(&self, query: &Embedding) -> Result<Vec<(String, f32)>, String> {
        let Some(space) = self.space else {
            return Ok(Vec::new());
        };
        if query.dimensions() != space.dimensions() {
            return Err(dimension_error(query.dimensions(), space.dimensions()));
        }

        let mut scored: Vec<(String, f32)> = self
            .entries
            .iter()
            .map(|(id, embedding)| (id.clone(), cosine(&query.values, &embedding.values)))
            .collect();

        scored.sort_by(|left, right| {
            right
                .1
                .partial_cmp(&left.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.0.cmp(&right.0))
        });
        Ok(scored)
    }
}

/// 维度不一致的报错。**必须可执行**：告诉用户怎么办，而不是只说「不匹配」。
fn dimension_error(actual: usize, expected: usize) -> String {
    format!(
        "向量维度不一致：收到 {actual} 维，索引是 {expected} 维。\
         通常是因为换了 embedding 模型；请在设置里重建向量索引（reindex）。"
    )
}

fn cosine(left: &[f32], right: &[f32]) -> f32 {
    let mut dot = 0.0;
    let mut left_norm = 0.0;
    let mut right_norm = 0.0;
    for (a, b) in left.iter().zip(right.iter()) {
        dot += a * b;
        left_norm += a * a;
        right_norm += b * b;
    }
    if left_norm == 0.0 || right_norm == 0.0 {
        return 0.0;
    }
    dot / (left_norm.sqrt() * right_norm.sqrt())
}
