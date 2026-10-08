//! 分级调度：**这一帧值不值得花钱？**
//!
//! `decide()` 是纯函数 —— 同样的输入永远给同样的输出，没有隐藏状态，
//! 因此可以用表驱动穷举所有分支（见 `tests/schedule.rs`）。

use std::time::Duration;

use mc_capture::change::ChangeKind;

/// 任务的成本档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TaskKind {
    /// 只更新元数据，零模型成本
    L0Metadata,
    /// 轻量文本处理（OCR / 本地规则），成本低
    L1Text,
    /// 视觉模型分析，成本最高
    L2Vision,
}

/// 规则声明的 AI 参与程度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AiPreference {
    /// 规则自己就能决定，不需要模型
    #[default]
    None,
    /// 先用便宜的档位辅助
    Assist,
    /// 规则要求深度分析
    Deep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// 画面没有显著变化
    Unchanged,
    /// 规则已经给出结论
    RuleDecided,
    /// 达到用量上限
    BudgetExceeded,
    /// 下游积压，主动降级
    Backpressure,
    /// 命中隐私规则
    PrivacyBlocked,
}

/// 队列压力档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressureLevel {
    Normal,
    /// 合并冗余画面
    Coalesce,
    /// 只分析重大变化
    Degrade,
    /// 暂停 AI，采集继续
    Shed,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Watermarks {
    pub low: f32,
    pub high: f32,
    pub critical: f32,
}

impl Default for Watermarks {
    fn default() -> Self {
        Self {
            low: 0.50,
            high: 0.80,
            critical: 0.95,
        }
    }
}

impl Watermarks {
    /// 构造并校验顺序。顺序错乱会让降级档位互相覆盖，属于配置错误而非运行时问题。
    pub fn new(low: f32, high: f32, critical: f32) -> Result<Self, String> {
        if !(low > 0.0 && low < high && high < critical && critical <= 1.0) {
            return Err(format!(
                "水位必须满足 0 < low < high < critical <= 1，实际 {low}/{high}/{critical}"
            ));
        }
        Ok(Self {
            low,
            high,
            critical,
        })
    }

    pub fn level(&self, pressure: f32) -> PressureLevel {
        if pressure >= self.critical {
            PressureLevel::Shed
        } else if pressure >= self.high {
            PressureLevel::Degrade
        } else if pressure >= self.low {
            PressureLevel::Coalesce
        } else {
            PressureLevel::Normal
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SchedulingInput {
    pub change_kind: ChangeKind,
    /// 是否有用户规则命中
    pub rule_matched: bool,
    pub ai_preference: AiPreference,
    /// 队列压力 0.0–1.0
    pub pressure: f32,
    pub budget_exceeded: bool,
    pub privacy_blocked: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SchedulingDecision {
    pub tasks: Vec<TaskKind>,
    pub skip_reason: Option<SkipReason>,
    pub pressure_level: PressureLevel,
}

impl SchedulingDecision {
    pub fn needs_vision(&self) -> bool {
        self.tasks.contains(&TaskKind::L2Vision)
    }

    pub fn is_skipped(&self) -> bool {
        self.tasks.is_empty()
    }
}

/// 纯决策函数。
///
/// 判定顺序是有讲究的：
/// 1. 隐私优先 —— 拦截的内容不产生任何作业
/// 2. 没有变化就不花钱 —— 这是省 token 的主要来源
/// 3. 过载 / 超预算降级 —— 但**保留廉价元数据**，降级不是停摆
/// 4. 规则命中且声明不需要 AI —— 规则的结论优先于模型
pub fn decide(input: &SchedulingInput, watermarks: &Watermarks) -> SchedulingDecision {
    let pressure_level = watermarks.level(input.pressure);

    let decision = |tasks: Vec<TaskKind>, skip_reason: Option<SkipReason>| SchedulingDecision {
        tasks,
        skip_reason,
        pressure_level,
    };

    if input.privacy_blocked {
        return decision(Vec::new(), Some(SkipReason::PrivacyBlocked));
    }

    match input.change_kind {
        ChangeKind::PixelMinor | ChangeKind::Idle | ChangeKind::Unknown => {
            // 画面没变：连元数据都不用写（观测本身已经落库了）
            return decision(Vec::new(), Some(SkipReason::Unchanged));
        }
        ChangeKind::TitleOnly => {
            return decision(vec![TaskKind::L0Metadata], None);
        }
        ChangeKind::New | ChangeKind::PixelMajor => {}
    }

    // 过载时先丢最贵的
    if pressure_level == PressureLevel::Shed {
        return decision(vec![TaskKind::L0Metadata], Some(SkipReason::Backpressure));
    }
    if input.budget_exceeded {
        return decision(vec![TaskKind::L0Metadata], Some(SkipReason::BudgetExceeded));
    }

    // 规则命中且规则声明不需要 AI
    if input.rule_matched && input.ai_preference == AiPreference::None {
        return decision(vec![TaskKind::L0Metadata], Some(SkipReason::RuleDecided));
    }

    let mut tasks = vec![TaskKind::L0Metadata];

    match (input.rule_matched, input.ai_preference) {
        // 规则要求深度分析
        (true, AiPreference::Deep) => tasks.push(TaskKind::L2Vision),
        // 规则只要辅助：先走便宜的 L1，不直接上最贵的
        (true, AiPreference::Assist) => tasks.push(TaskKind::L1Text),
        // 没有规则 → 交给视觉模型
        _ => tasks.push(TaskKind::L2Vision),
    }

    decision(tasks, None)
}

/// 指数退避 + 确定性抖动。
///
/// 抖动用 `attempt` 派生（而不是真随机），因此**同一 attempt 结果可复现** ——
/// 否则重试相关的测试会随机失败，排查时也无从复现。
pub fn backoff_delay(base: Duration, max: Duration, attempt: u32, jitter: f32) -> Duration {
    let attempt = attempt.max(1);
    let shift = (attempt - 1).min(20);
    let nominal_ms = (base.as_millis() as f64) * 2f64.powi(shift as i32);
    let nominal_ms = nominal_ms.min(max.as_millis() as f64);

    let jitter = f64::from(jitter.clamp(0.0, 1.0));
    // unit ∈ [0, 1) → factor ∈ [1 - j/2, 1 + j/2)
    let unit = deterministic_unit(attempt);
    let factor = (1.0 - jitter / 2.0) + unit * jitter;

    // 先加抖动再封顶：反过来会让「最大延迟」被抖动突破
    let max_ms = max.as_millis() as f64;
    let ms = (nominal_ms * factor).floor().min(max_ms).max(1.0);
    Duration::from_millis(ms as u64)
}

fn deterministic_unit(attempt: u32) -> f64 {
    let mut x = (attempt as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    // 取低 53 位映射到 [0, 1)
    (x >> 11) as f64 / (1u64 << 53) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_is_deterministic_for_the_same_attempt() {
        let a = backoff_delay(Duration::from_millis(500), Duration::from_secs(30), 3, 0.5);
        let b = backoff_delay(Duration::from_millis(500), Duration::from_secs(30), 3, 0.5);
        assert_eq!(a, b, "抖动必须可复现，否则重试测试会随机失败");
    }

    #[test]
    fn backoff_grows_with_attempt() {
        let base = Duration::from_millis(100);
        let max = Duration::from_secs(60);
        let first = backoff_delay(base, max, 1, 0.0);
        let third = backoff_delay(base, max, 3, 0.0);
        assert!(third > first);
    }

    #[test]
    fn zero_jitter_is_exact() {
        assert_eq!(
            backoff_delay(Duration::from_millis(500), Duration::from_secs(30), 1, 0.0),
            Duration::from_millis(500)
        );
    }
}
