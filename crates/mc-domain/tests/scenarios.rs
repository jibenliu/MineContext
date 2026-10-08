//! 场景回归。
//!
//! 活动的正确性是**时间序列上的涌现结果**，用「一个函数一个断言」说不清。
//! 因此输入做成数据文件（`fixtures/scenarios/activity/*.yaml`），
//! 一份文件同时断言：活动序列、观测归属、噪声、以及 **VLM 调用次数**。
//!
//! 最贵的成本项是 token，所以
//! 「规则命中的场景必须 0 次模型调用」也是这里的硬断言。

use mc_testkit::scenario::{load_all, run, verify};

#[test]
fn every_activity_scenario_passes() {
    let directory = mc_testkit::scenario::activity_scenarios_dir();
    let scenarios = load_all(&directory)
        .unwrap_or_else(|error| panic!("场景目录 {} 读取失败：{error}", directory.display()));

    assert!(
        scenarios.len() >= 10,
        "活动场景至少要 10 个（phase-3 §2.8），实际 {}",
        scenarios.len()
    );

    for (path, scenario) in &scenarios {
        let outcome =
            run(scenario).unwrap_or_else(|error| panic!("{} 运行失败：{error}", path.display()));
        verify(scenario, &outcome)
            .unwrap_or_else(|error| panic!("{} 断言失败：{error}", path.display()));
    }
}
