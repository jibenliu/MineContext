//! 把派生数据变成可检索文档。
//!
//! 检索层（`mc-search`）是纯函数；这里负责把数据库里的东西喂给它，
//! 并把结果转成对话用的**引用**。
//!
//! **隐私在进入检索之前就把关**（fail closed）：只要一条活动的证据里
//! 含有被隐私规则拦截的观测，整条活动就不参与检索。
//! 逐条过滤是不够的 —— 活动标题往往就带着应用名，
//! 几条拼起来照样能推断出被拦截的内容。

use std::collections::HashSet;

use mc_common::error::AppError;
use mc_common::observability::debug;
use mc_domain::activity::Provenance;
use mc_search::{
    hybrid_search, Document, DocumentKind, Embedding, Hit, ScoreFusion, SearchFilters, SearchQuery,
    VectorIndex,
};
use mc_storage::Database;

use crate::chat::Citation;

/// 可检索文档：活动 + 总结。
pub fn documents(db: &Database) -> Result<Vec<Document>, AppError> {
    documents_for_filters(db, &SearchFilters::default())
}

fn documents_for_filters(
    db: &Database,
    filters: &SearchFilters,
) -> Result<Vec<Document>, AppError> {
    let includes = |kind| filters.kinds.is_empty() || filters.kinds.contains(&kind);
    // vault 作用域：只检索该根文件夹子树内的笔记，不混入活动 / 总结，避免跨 vault 污染。
    let vault_scoped = filters.vault_id.is_some();
    let vault_note_ids = match filters.vault_id {
        Some(vault_id) => Some(db.vault_subtree_ids(vault_id)?),
        None => None,
    };

    let (activities, blocked) = if !vault_scoped && includes(DocumentKind::Activity) {
        (
            mc_storage::projectors::activities::read_all(db)?,
            db.blocked_observation_ids()?
                .into_iter()
                .collect::<HashSet<String>>(),
        )
    } else {
        (Vec::new(), HashSet::new())
    };

    let mut documents = Vec::new();

    for activity in activities {
        // 失败即关闭：证据里沾了被拦截的观测，整条不参与检索
        if activity
            .evidence
            .iter()
            .any(|observation| blocked.contains(observation))
        {
            continue;
        }

        let mut text = activity.title.clone();
        if let Some(category) = &activity.category {
            text.push(' ');
            text.push_str(category);
        }

        documents.push(Document {
            id: activity.id.clone(),
            kind: DocumentKind::Activity,
            text,
            at: activity.start,
            provenance: activity.origin.clone(),
            blocked: false,
        });
    }

    // 笔记（vaults）：用户自己写下的内容，是最该被问答命中的一类。
    // 只看文件（文件夹没有正文），默认排除已删除（`VaultQuery` 的语义）。
    let notes = if includes(DocumentKind::Document) {
        db.query_vault_rows(&mc_storage::vaults::VaultQuery {
            document_type: Vec::new(),
            parent_id: None,
            title: None,
            is_folder: Some(0),
            is_deleted: None,
        })?
    } else {
        Vec::new()
    };
    for note in notes {
        if let Some(allowed) = &vault_note_ids {
            if !allowed.contains(&note.id) {
                continue;
            }
        }
        let mut text = note.title.clone();
        if !note.summary.trim().is_empty() {
            text.push(' ');
            text.push_str(note.summary.trim());
        }
        let content = note.content.trim();
        if !content.is_empty() {
            text.push(' ');
            // 单篇笔记可能很长：检索语料按前 4000 字符截断（够搜到，也不至于把
            // 索引和提示词撑爆）；超出部分检索不到属已知边界。
            text.push_str(&content.chars().take(4000).collect::<String>());
        }
        if text.trim().is_empty() {
            continue;
        }
        // 时间戳：库里是文本形态，解析不出来就退到「没有时间」的极值，
        // 只影响时间过滤，不影响全文命中。
        let at = mc_common::time::Timestamp::parse_flexible(&note.updated_at)
            .unwrap_or_else(|_| mc_common::time::Timestamp::from_millis(0));
        documents.push(Document {
            id: format!("note-{}", note.id),
            kind: DocumentKind::Document,
            text,
            at,
            // 笔记是用户写的**记录**，不是系统推测
            provenance: Provenance::Observed,
            blocked: false,
        });
    }

    let summaries = if !vault_scoped && includes(DocumentKind::Summary) {
        db.read_summaries_in_range(None, None, filters.from, filters.to)?
    } else {
        Vec::new()
    };
    for summary in summaries {
        documents.push(Document {
            id: summary.id.clone(),
            kind: DocumentKind::Summary,
            // 标题 + 正文：总结里往往含有「做了什么」的原话，正是最该被搜到的
            text: format!("{} {}", summary.title, summary.body_markdown),
            at: summary.start,
            // 总结是**记录**而不是推测：它是系统写下来的东西
            provenance: Provenance::Observed,
            blocked: false,
        });
    }

    Ok(documents)
}

/// 检索。没有向量后端时自动降级为纯关键词（`mc-search` 的语义）。
pub fn retrieve(
    db: &Database,
    query: &str,
    limit: usize,
    observed_only: bool,
) -> Result<Vec<Hit>, AppError> {
    retrieve_with_vectors(db, query, limit, observed_only, None)
}

/// 带向量的检索。`vectors` 为 `None` 时等价于纯关键词。
///
/// 把向量作为**可选参数**而不是内部去查库：检索是同步纯函数，
/// 而查询向量需要一次网络调用（异步）。分开之后，
/// 「检索」这件事在测试里永远是确定性的，网络只影响「有没有向量」。
pub fn retrieve_with_vectors(
    db: &Database,
    query: &str,
    limit: usize,
    observed_only: bool,
    vectors: Option<(&VectorIndex, &Embedding)>,
) -> Result<Vec<Hit>, AppError> {
    retrieve_filtered(
        db,
        query,
        limit,
        SearchFilters {
            observed_only,
            ..SearchFilters::default()
        },
        vectors,
    )
}

/// 在排名与数量截断之前应用筛选，避免范围外的命中挤掉范围内的结果。
pub fn retrieve_filtered(
    db: &Database,
    query: &str,
    limit: usize,
    filters: SearchFilters,
    vectors: Option<(&VectorIndex, &Embedding)>,
) -> Result<Vec<Hit>, AppError> {
    let documents = documents_for_filters(db, &filters)?;
    if documents.is_empty() {
        return Ok(Vec::new());
    }

    let hits = hybrid_search(
        &documents,
        &SearchQuery {
            text: query.to_string(),
            filters,
            limit,
        },
        vectors,
        ScoreFusion::default(),
    );
    // 只记条数：查询词是用户内容，不进日志。
    debug!(
        component = "search",
        event = "query",
        documents = documents.len(),
        results = hits.len(),
        "检索完成"
    );
    Ok(hits)
}

/// 从落库的向量组装检索用的索引。两条**失败即关闭**的规则：
///
/// 1. 只索引「当前可检索文档集」里存在的 id：库里出现陌生 id（活动被删、
///    向量陈旧）时宁可少搜 —— 多出来的条目意味着某处数据不一致；
/// 2. 被隐私规则拦截的活动不在文档集里，自然进不了索引 ——
///    「不参与检索」必须包括**向量检索**，否则绕过关键词就能捞到。
pub fn vector_index(db: &Database) -> Result<Option<VectorIndex>, AppError> {
    let documents = documents(db)?;
    let allowed: HashSet<(&str, &str)> = documents
        .iter()
        .map(|document| (document.kind.as_str(), document.id.as_str()))
        .collect();

    let mut index = VectorIndex::unbound();
    // 活动 / 笔记 / 总结共用同一套索引：漏加载任一类都会让同义改写退化为关键词。
    for kind in ["activity", "document", "summary"] {
        for record in mc_storage::vectors::load_vectors(db, kind)? {
            if !allowed.contains(&(kind, record.doc_id.as_str())) {
                continue;
            }
            // 维度不一致说明索引正在重建（或者用户换了模型）：
            // 这里只跳过，不报错 —— 向量是加分项，不该让检索整体失败。
            let _ = index.insert(&record.doc_id, Embedding::new(record.values));
        }
    }

    Ok((!index.is_empty()).then_some(index))
}

/// 检索结果 → 对话引用（回答里要能说出「依据是这几条」）。
pub fn citations(hits: &[Hit]) -> Vec<Citation> {
    hits.iter()
        .map(|hit| Citation {
            document_id: hit.document.id.clone(),
            title: first_line(&hit.document.text),
            kind: hit.document.kind.as_str().to_string(),
            at: hit.document.at.as_millis(),
        })
        .collect()
}

/// 引用标题只取第一行、截断到 60 字：引用是给人看的，
/// 把整篇总结塞进引用列表会把 UI 撑爆。
fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    line.chars().take(60).collect()
}

/// 把活动还原成线索输入（Thread 只需要标题 + 时间 + provenance）。
pub fn threads(
    db: &Database,
    timezone: &str,
    limit: usize,
) -> Result<Vec<mc_memory::thread::Thread>, AppError> {
    // 顺手同步一次实体关联（幂等且增量，只有变化时才写库）。
    // 这样即使 daemon 还没跑到同步那一步，线索也拿得到结果 ——
    // 「查询依赖后台先跑过一轮」是很容易被忽略的隐性耦合。
    sync_activity_entities(db)?;

    let activities = thread_activities(db)?;
    let links = thread_links(db, &activities)?;

    let mut threads = mc_memory::thread::build_threads_from_links(&activities, &links, timezone);

    // 最近的线索优先：用户问「到哪了」时关心的是还在推进的那些
    threads.sort_by(|left, right| {
        right
            .last_at()
            .cmp(&left.last_at())
            .then_with(|| left.entity.canonical.cmp(&right.entity.canonical))
    });
    threads.truncate(limit.max(1));
    Ok(threads)
}

/// 可参与线索的活动。**与检索同一条隐私规则**：
/// 证据里沾了被拦截观测的活动整条排除。
fn thread_activities(db: &Database) -> Result<Vec<mc_memory::thread::ThreadActivity>, AppError> {
    let blocked: HashSet<String> = db.blocked_observation_ids()?.into_iter().collect();

    Ok(mc_storage::projectors::activities::read_all(db)?
        .into_iter()
        .filter(|activity| {
            !activity
                .evidence
                .iter()
                .any(|observation| blocked.contains(observation))
        })
        .map(|activity| mc_memory::thread::ThreadActivity {
            id: activity.id.clone(),
            title: activity.title.clone(),
            start: activity.start,
            end: activity.end,
            provenance: activity.origin.clone(),
        })
        .collect())
}

/// 落库的实体关联 → 线索输入。
///
/// **只取输入里存在的活动**：关联行可能指向已被隐私过滤或已删除的活动，
/// 让它们留在输入里等于给「被拦截的内容」留一条后路。
fn thread_links(
    db: &Database,
    activities: &[mc_memory::thread::ThreadActivity],
) -> Result<Vec<mc_memory::thread::ThreadEntityLink>, AppError> {
    let allowed: Vec<String> = activities
        .iter()
        .map(|activity| activity.id.clone())
        .collect();

    let mut links = Vec::new();
    for row in db.activity_entities_for_ids(&allowed)? {
        let Some(kind) = mc_memory::entity::EntityKind::from_storage(&row.kind) else {
            continue;
        };
        links.push(mc_memory::thread::ThreadEntityLink {
            activity_id: row.activity_id,
            entity: mc_memory::entity::Entity {
                kind,
                canonical: row.key,
                display: row.display,
            },
        });
    }
    Ok(links)
}

/// 实体关联同步的结果。
///
/// `linked`（建立了关联）与 `cleared`（关联被清掉）刻意分开：
/// 只看「写了几次库」会把两类完全相反的事情混成一个数字。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntitySyncReport {
    /// 写了关联的活动数（新建或内容变化）
    pub linked: usize,
    /// 已经一致、无需写入的活动数
    pub unchanged: usize,
    /// 关联被清掉的活动数（不再提及实体 / 被隐私拦截 / 已删除）
    pub cleared: usize,
}

/// 把「活动标题里的实体」同步进 `activity_entities`。
///
/// 三条规则：
/// - **确定性抽取**（`mc_memory::entity::extract`），不调模型；
/// - **失败即关闭**：证据里沾了被拦截观测的活动不写入，且已有行会被清掉；
/// - **增量**：与库里已有行一致的活动不重写（同步会被线索接口频繁调用）。
pub fn sync_activity_entities(db: &Database) -> Result<EntitySyncReport, AppError> {
    let blocked: HashSet<String> = db.blocked_observation_ids()?.into_iter().collect();
    let activities = mc_storage::projectors::activities::read_all(db)?;

    let mut report = EntitySyncReport::default();
    let mut allowed: HashSet<String> = HashSet::new();
    let registry = mc_memory::entity::EntityRegistry::with_defaults();
    let mut existing =
        std::collections::HashMap::<String, Vec<mc_storage::entities::ActivityEntity>>::new();
    for row in db.all_activity_entities()? {
        existing
            .entry(row.activity_id.clone())
            .or_default()
            .push(row);
    }
    let mut changes = Vec::new();

    for activity in &activities {
        if activity
            .evidence
            .iter()
            .any(|observation| blocked.contains(observation))
        {
            continue;
        }
        allowed.insert(activity.id.clone());

        let mut text = activity.title.clone();
        if let Some(category) = activity.category.as_ref().filter(|c| !c.is_empty()) {
            text.push(' ');
            text.push_str(category);
        }

        let desired: Vec<mc_storage::entities::ActivityEntity> = mc_memory::entity::extract(&text)
            .into_iter()
            .map(|entity| {
                let resolved = registry.resolve(&entity.display);
                mc_storage::entities::ActivityEntity {
                    activity_id: activity.id.clone(),
                    kind: resolved.kind.as_str().to_string(),
                    key: resolved.canonical,
                    display: resolved.display,
                    start: activity.start,
                }
            })
            .collect();

        let current = existing.remove(&activity.id).unwrap_or_default();
        if same_links(&current, &desired) {
            report.unchanged += 1;
            continue;
        }

        // 先删后插：`desired` 为空就等于「这个活动不再关联任何实体」
        if desired.is_empty() {
            report.cleared += 1;
        } else {
            report.linked += 1;
        }
        changes.push((activity.id.clone(), desired));
    }

    if !changes.is_empty() || !existing.is_empty() {
        report.cleared +=
            db.sync_activity_entity_links(&changes, &allowed.into_iter().collect::<Vec<_>>())?;
    }

    Ok(report)
}

/// 关联是否已经完全一致（比较 kind/key/display，顺序无关）。
fn same_links(
    left: &[mc_storage::entities::ActivityEntity],
    right: &[mc_storage::entities::ActivityEntity],
) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let key = |entity: &mc_storage::entities::ActivityEntity| {
        (
            entity.kind.clone(),
            entity.key.clone(),
            entity.display.clone(),
        )
    };
    let mut left: Vec<_> = left.iter().map(key).collect();
    let mut right: Vec<_> = right.iter().map(key).collect();
    left.sort();
    right.sort();
    left == right
}
