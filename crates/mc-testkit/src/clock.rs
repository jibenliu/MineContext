//! 可注入、可推进的测试时钟。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use mc_common::lock::lock_or_recover;
use mc_common::time::{Clock, Timestamp};

/// 受控时钟。克隆后共享同一时间源，因此可以在一个任务里推进时间、
/// 在另一个任务里观察结果（Stage 状态机、防抖、退避重试都依赖这一点）。
#[derive(Debug, Clone, Default)]
pub struct TestClock {
    now: Arc<Mutex<Timestamp>>,
}

impl TestClock {
    pub fn new(at: Timestamp) -> Self {
        Self {
            now: Arc::new(Mutex::new(at)),
        }
    }

    /// 从 RFC 3339 字面量构造。仅用于测试，因此对非法输入 panic。
    pub fn at(rfc3339: &str) -> Self {
        Self::new(
            Timestamp::parse_rfc3339(rfc3339).unwrap_or_else(|e| {
                panic!("TestClock::at({rfc3339:?}) is not valid RFC 3339: {e}")
            }),
        )
    }

    pub fn from_millis(ms: i64) -> Self {
        Self::new(Timestamp::from_millis(ms))
    }

    /// 向前推进（也接受「向前推进一个负数」用于模拟系统时钟回拨）。
    pub fn advance(&self, delta: Duration) {
        let mut guard = lock_or_recover(&self.now);
        let current = *guard;
        *guard = current.plus_millis(delta.as_millis() as i64);
    }

    pub fn set(&self, at: Timestamp) {
        let mut guard = lock_or_recover(&self.now);
        *guard = at;
    }
}

impl Clock for TestClock {
    fn now(&self) -> Timestamp {
        *lock_or_recover(&self.now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poisoned_clock_can_be_read_set_and_advanced() {
        let clock = TestClock::from_millis(1000);
        let shared = clock.clone();
        assert!(std::thread::spawn(move || {
            let _guard = shared.now.lock().unwrap();
            panic!("poison clock");
        })
        .join()
        .is_err());
        assert_eq!(clock.now().as_millis(), 1000);
        clock.set(Timestamp::from_millis(2000));
        clock.advance(Duration::from_millis(10));
        assert_eq!(clock.now().as_millis(), 2010);
    }
}
