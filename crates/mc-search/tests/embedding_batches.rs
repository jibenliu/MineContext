//! embedding 批量上限。
//!
//! 为什么值得为「切批」单独写一测：Embedding 端点的 `input` 数组是有上限的
//! （各家不同，常见 64 / 100 / 2048），超限直接 400。
//! 做法是「把整天的活动拼成一个巨大请求」，于是同步失败、
//! 用户看到的是「AI 不可用」而不是「批量太大」。
//!
//! 这里的切批是**纯函数**：给定输入条数与上限，产出确定的批次区间。
//! 上传顺序、断点续传、失败重试都建立在「批次计划是确定的」之上。

use mc_search::plan_batches;

#[test]
fn batching_respects_limit() {
    let batches = plan_batches(130, 64).expect("64 是合法上限");
    assert_eq!(batches.len(), 3);
    assert_eq!(batches[0], 0..64);
    assert_eq!(batches[1], 64..128);
    assert_eq!(batches[2], 128..130);
}

// 5.3b — 整除时不应产生空尾巴（空请求既浪费配额也会被某些端点拒绝）
#[test]
fn exact_multiple_has_no_empty_tail() {
    let batches = plan_batches(128, 64).unwrap();
    assert_eq!(batches.len(), 2);
    assert_eq!(batches[1], 64..128);
}

// 5.3c
#[test]
fn single_batch_when_input_fits() {
    assert_eq!(plan_batches(1, 64).unwrap(), vec![0..1]);
}

// 5.3d — 边界：没有输入就没有请求（而不是发一个空请求）
#[test]
fn empty_input_plans_no_batches() {
    assert!(plan_batches(0, 64).unwrap().is_empty());
}

// 5.3e — 上限为 0 是调用方写错了，必须报错而不是死循环
#[test]
fn zero_limit_is_rejected() {
    let error = plan_batches(10, 0).expect_err("上限为 0 必须报错");
    assert!(error.contains("上限"), "错误要说清是上限问题：{error}");
}

// 5.3f — 计划必须覆盖全部输入且不重不漏（顺序即上传顺序）
#[test]
fn plan_covers_all_inputs_in_order() {
    for total in [1usize, 7, 64, 65, 129, 1000] {
        for limit in [1usize, 3, 64, 100] {
            let batches = plan_batches(total, limit).unwrap();
            let covered: Vec<usize> = batches.iter().flat_map(|r| r.clone()).collect();
            assert_eq!(
                covered,
                (0..total).collect::<Vec<_>>(),
                "total={total} limit={limit} 的批次计划必须覆盖且不重复"
            );
            assert!(batches.iter().all(|r| r.len() <= limit));
            assert!(batches.iter().all(|r| !r.is_empty()));
        }
    }
}

// 5.3g — 同样输入必须得到同样计划（幂等：重试时不会错位）
#[test]
fn plan_is_deterministic() {
    assert_eq!(
        plan_batches(200, 64).unwrap(),
        plan_batches(200, 64).unwrap()
    );
}
