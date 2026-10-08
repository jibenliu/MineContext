//! stage 与 summary 场景回归。
//!
//! 场景文件（`fixtures/scenarios/stage/*.yaml`、`summary/*.yaml`）把
//! 「一段时间轴上发生了什么」固化成数据：阶段划分的正确性用
//! 「一个函数一个断言」说不清，而**结束原因**（切走 / 空闲 / 锁屏 / 跨天）
//! 恰恰是最容易记错的东西。
//!
//! 总结断言走**确定性兜底**渲染，不调用模型：
//! 因此「有阶段必有总结」这条不变量能在 CI 里精确验证。

use mc_testkit::scenario::{load_all, run, verify};

fn run_directory(directory: std::path::PathBuf, minimum: usize) {
    let scenarios = load_all(&directory)
        .unwrap_or_else(|error| panic!("场景目录 {} 读取失败：{error}", directory.display()));

    assert!(
        scenarios.len() >= minimum,
        "{} 至少需要 {minimum} 个场景，实际 {}",
        directory.display(),
        scenarios.len()
    );

    for (path, scenario) in &scenarios {
        let outcome =
            run(scenario).unwrap_or_else(|error| panic!("{} 运行失败：{error}", path.display()));
        verify(scenario, &outcome)
            .unwrap_or_else(|error| panic!("{} 断言失败：{error}", path.display()));
    }
}

#[test]
fn every_stage_scenario_passes() {
    run_directory(mc_testkit::scenario::stage_scenarios_dir(), 7);
}

#[test]
fn every_summary_scenario_passes() {
    run_directory(mc_testkit::scenario::summary_scenarios_dir(), 3);
}
