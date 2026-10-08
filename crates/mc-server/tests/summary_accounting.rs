//! 总结路径的成本记账：模型产出的总结要进 `provider_calls`。
//!
//! 这条路径的 token 花得最多（阶段/日报/周报/任意时段都走它），
//! 之前只有语义索引与活动推断记账，报表上「总结花了多少」是零。

use mc_common::time::Timestamp;
use mc_storage::Database;
use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

#[test]
fn model_summaries_are_accounted_as_chat() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path().join("minecontext.db")).unwrap();
    let at = Timestamp::from_millis(T0);

    // 延迟由调用方实测传入：这里给一个具体值，下面断言它真的落库了
    // （此前恒写 0，Chat 组的平均延迟永远是 0，排查慢请求时没有依据）
    mc_server::summary::record_summary_usage(
        &db,
        Some("openai_compatible:qwen3-max"),
        900,
        100,
        1_234,
        at,
    )
    .expect("记账");

    let report = db
        .provider_usage(
            Timestamp::from_millis(T0 - 1_000),
            Timestamp::from_millis(T0 + 1_000),
        )
        .expect("用量报表");

    assert_eq!(report.calls, 1);
    assert_eq!(report.total_tokens(), 1_000);
    assert_eq!(
        report
            .by_purpose
            .iter()
            .map(|p| p.purpose.as_str())
            .collect::<Vec<_>>(),
        vec!["chat"],
        "总结属于 chat 用途，不能混进 vision/embedding"
    );
    assert_eq!(
        report.avg_latency_ms, 1_234,
        "总结的延迟必须如实记录（恒 0 会让 Chat 组的平均延迟失去意义）"
    );
}

#[test]
fn fallback_summaries_do_not_fake_a_model_call() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path().join("minecontext.db")).unwrap();
    let at = Timestamp::from_millis(T0);

    // 兜底总结没有模型名：不该写出一条「花了钱」的记录
    mc_server::summary::record_summary_usage(&db, None, 0, 0, 0, at).expect("记账");

    let report = db
        .provider_usage(
            Timestamp::from_millis(T0 - 1_000),
            Timestamp::from_millis(T0 + 1_000),
        )
        .expect("用量报表");
    assert_eq!(report.calls, 0);
}
