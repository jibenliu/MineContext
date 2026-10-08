//! 重放性能。
//!
//! 投影是**全量纯函数**，每次重算都要跑完整个事件历史。
//! 因此它必须比「事件增长速度」快一个数量级，否则重放会从
//! 「随时可以重算」变成「再也不敢碰」。
//! 常跑的那条用 2 万条事件（CI 里几百毫秒），
//! 1M 的那条按验收标准单独跑：
//! ```text
//! cargo test -p mc-domain --test projector_perf -- --ignored --nocapture
//! ```

use std::time::Instant;

use mc_common::time::Timestamp;
use mc_domain::observation::ObservationSummary;
use mc_domain::projector::{project, DomainEvent, ProjectionOptions};
use mc_domain::rules::RuleSet;

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

fn rules() -> RuleSet {
    RuleSet::parse_yaml(
        r#"
version: 1
activities:
  - id: coding
    name: 写代码
    category: 开发
    triggers: { apps: [VSCode] }
  - id: browsing
    name: 看网页
    category: 研究
    triggers: { apps: [Chrome] }
"#,
    )
    .expect("规则")
}

/// `count` 条观测，每 20 秒一条，每 5 分钟换一次应用。
/// 换应用的频率决定活动数量 —— 全用一个应用就测不出合并/切分的成本。
fn events(count: usize) -> Vec<DomainEvent> {
    (0..count)
        .map(|index| {
            let at = Timestamp::from_millis(T0 + index as i64 * 20_000);
            let app = if (index / 15) % 2 == 0 {
                "VSCode"
            } else {
                "Chrome"
            };
            DomainEvent::ObservationRecorded {
                observation: ObservationSummary {
                    id: format!("obs-{index}"),
                    at,
                    app_name: Some(app.to_string()),
                    window_title: Some("main.rs".to_string()),
                    domain: None,
                    text: None,
                },
            }
        })
        .collect()
}

fn elapsed_ms(count: usize) -> (u128, usize) {
    let events = events(count);
    let rules = rules();
    let options = ProjectionOptions::default();

    let started = Instant::now();
    let projection = project(&events, &rules, options);
    let elapsed = started.elapsed().as_millis();

    (elapsed, projection.activities.len())
}

// 3.42（常跑版）—— 回归用：投影复杂度不能悄悄退化成平方
#[ignore = "基准，按需运行：--ignored --nocapture"]
#[test]
fn replaying_twenty_thousand_events_is_fast() {
    let (elapsed, activities) = elapsed_ms(20_000);

    assert!(activities > 10, "应当产出多个活动，实际 {activities}");
    assert!(
        elapsed < 10_000,
        "2 万条事件的投影耗时 {elapsed}ms，超出 10s：复杂度很可能退化了"
    );
}

// 3.42（验收版）—— 1M 事件 60 秒内。默认忽略，避免拖慢 CI。
#[test]
#[ignore = "1M 事件基准，按需运行：--ignored --nocapture"]
fn replay_one_million_events_under_sixty_seconds() {
    let (elapsed, activities) = elapsed_ms(1_000_000);

    println!("1M 事件投影耗时 {elapsed}ms，产出 {activities} 个活动");
    assert!(
        elapsed < 60_000,
        "3.42 的验收标准是 1M 事件 60 秒内，实际 {elapsed}ms"
    );
}
