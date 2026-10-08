//! Thread：同一个实体的活动线索。
//!
//! 用户的问题往往是「这件事到哪了」而不是「昨天几点几分我在干嘛」。
//! Thread 就是把提到同一个实体的活动按时间串起来，并给出一份跨天进展。
//!
//! **provenance 全程保留**（5.39）：线索里哪些是看见的、哪些是猜的
//! 必须能区分 —— 否则用户会把「模型猜的」当成「我真的做过」。

use chrono::NaiveDate;
use mc_common::time::Timestamp;
use mc_domain::activity::Provenance;

use crate::entity::{extract, Entity, EntityRegistry};

#[derive(Debug, Clone, PartialEq)]
pub struct ThreadActivity {
    pub id: String,
    pub title: String,
    pub start: Timestamp,
    pub end: Timestamp,
    pub provenance: Provenance,
}

impl ThreadActivity {
    pub fn is_inferred(&self) -> bool {
        self.provenance.is_inferred()
    }

    fn day(&self, timezone: &str) -> Option<NaiveDate> {
        self.start.to_local_date(timezone).ok()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Thread {
    pub entity: Entity,
    /// 按时间排序
    pub activities: Vec<ThreadActivity>,
    /// 出现过的本地日期（唯一、升序）
    pub days: Vec<NaiveDate>,
    pub timezone: String,
}

impl Thread {
    pub fn observed_count(&self) -> usize {
        self.activities
            .iter()
            .filter(|activity| !activity.is_inferred())
            .count()
    }

    pub fn inferred_count(&self) -> usize {
        self.activities
            .iter()
            .filter(|activity| activity.is_inferred())
            .count()
    }

    pub fn first_at(&self) -> Option<Timestamp> {
        self.activities.first().map(|activity| activity.start)
    }

    pub fn last_at(&self) -> Option<Timestamp> {
        self.activities.last().map(|activity| activity.end)
    }
}

/// 一条「活动 ↔ 实体」关联（来自落库的 `activity_entities`）。
///
/// 落库路径与抽取路径的**区别是权威性**：抽取是「标题里现在有什么」，
/// 关联表是「历史上确实关联过什么」。活动标题被用户改掉之后，
/// 前者会消失，后者不会 —— 线索以后者为准。
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadEntityLink {
    pub activity_id: String,
    pub entity: Entity,
}

/// 按实体把活动串成线索。一个活动可以同时属于多条线索（提到两个实体时）。
pub fn build_threads(activities: &[ThreadActivity], registry: &EntityRegistry) -> Vec<Thread> {
    build_threads_in_timezone(activities, registry, "UTC")
}

/// 用**落库的关联**组装线索（不读标题）。
///
/// 完全不碰`extract`：关联表里没有的活动就不该出现在线索里，
/// 否则「用户改标题」或「隐私规则变化」都会让线索与事实脱节。
pub fn build_threads_from_links(
    activities: &[ThreadActivity],
    links: &[ThreadEntityLink],
    timezone: &str,
) -> Vec<Thread> {
    let by_id: std::collections::HashMap<&str, &ThreadActivity> = activities
        .iter()
        .map(|activity| (activity.id.as_str(), activity))
        .collect();

    let mut by_entity: std::collections::BTreeMap<String, (Entity, Vec<ThreadActivity>)> =
        std::collections::BTreeMap::new();

    for link in links {
        let Some(activity) = by_id.get(link.activity_id.as_str()) else {
            // 关联指向一个不在本次输入里的活动（已被隐私过滤掉 / 已删除）：
            // 跳过而不是报错 —— 线索是派生视图，少一条胜过泄漏一条。
            continue;
        };
        by_entity
            .entry(link.entity.canonical.clone())
            .or_insert_with(|| (link.entity.clone(), Vec::new()))
            .1
            .push((*activity).clone());
    }

    finish_threads(by_entity, timezone)
}

pub fn build_threads_in_timezone(
    activities: &[ThreadActivity],
    registry: &EntityRegistry,
    timezone: &str,
) -> Vec<Thread> {
    // 实体 → 活动。用 BTreeMap 保证输出顺序确定（canonical 升序）
    let mut by_entity: std::collections::BTreeMap<String, (Entity, Vec<ThreadActivity>)> =
        std::collections::BTreeMap::new();

    for activity in activities {
        for entity in extract(&activity.title) {
            let resolved = registry.resolve(&entity.display);
            by_entity
                .entry(resolved.canonical.clone())
                .or_insert_with(|| (resolved.clone(), Vec::new()))
                .1
                .push(activity.clone());
        }
    }

    finish_threads(by_entity, timezone)
}

/// 两条构建路径共用的收尾：排序、算天数、丢弃没有日期的线索。
fn finish_threads(
    by_entity: std::collections::BTreeMap<String, (Entity, Vec<ThreadActivity>)>,
    timezone: &str,
) -> Vec<Thread> {
    by_entity
        .into_values()
        .filter_map(|(entity, mut thread_activities)| {
            thread_activities.sort_by(|left, right| {
                left.start
                    .cmp(&right.start)
                    .then_with(|| left.id.cmp(&right.id))
            });

            let mut days: Vec<NaiveDate> = thread_activities
                .iter()
                .filter_map(|activity| activity.day(timezone))
                .collect();
            days.sort();
            days.dedup();

            if days.is_empty() {
                return None;
            }

            Some(Thread {
                entity,
                activities: thread_activities,
                days,
                timezone: timezone.to_string(),
            })
        })
        .collect()
}

/// 跨天进展摘要。**确定性**（不调模型）：用户问「到哪了」时要的是事实清单，
/// 而不是一段润色过的文字；需要润色时交给总结引擎。
pub fn thread_brief(thread: &Thread) -> String {
    let mut brief = String::new();
    brief.push_str(&format!(
        "{} 的线索：跨 {} 天，共 {} 条活动",
        thread.entity.canonical,
        thread.days.len(),
        thread.activities.len()
    ));
    if thread.inferred_count() > 0 {
        brief.push_str(&format!("（其中 {} 条是推测）", thread.inferred_count()));
    }
    brief.push('\n');

    for day in &thread.days {
        brief.push_str(&format!("\n{day}：\n"));
        for activity in thread
            .activities
            .iter()
            .filter(|activity| activity.day(&thread.timezone) == Some(*day))
        {
            let mark = if activity.is_inferred() {
                "（推测）"
            } else {
                ""
            };
            brief.push_str(&format!("  - {}{mark}\n", activity.title));
        }
    }

    brief
}
