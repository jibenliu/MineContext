//! 锁的取用约定：**中毒不放大**。
//!
//! `Mutex` 中毒只说明「另一个线程持锁时 panic 了」，不代表被保护的数据坏了 ——
//! 服务进程里用 `lock().expect(...)` 会把一次 panic 放大成后续所有请求失败。
//! 这里给出统一的替代：拿回守卫并继续（数据是 id 集合、标志位这类简单结构时是安全的）。
//!
//! 需要「中毒即视为不可继续」的地方不该用这个函数 —— 那种场景要在调用点写明理由。

use std::sync::{Mutex, MutexGuard};

/// 取锁；锁中毒时恢复守卫而不是 panic。
pub fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn returns_the_guard_on_a_healthy_lock() {
        let mutex = Mutex::new(vec![1, 2]);
        lock_or_recover(&mutex).push(3);
        assert_eq!(*lock_or_recover(&mutex), vec![1, 2, 3]);
    }

    #[test]
    fn recovers_instead_of_panicking_when_poisoned() {
        let mutex = Arc::new(Mutex::new(vec![1]));
        // 造一个中毒的锁：另一个线程持锁时 panic
        let poisoner = Arc::clone(&mutex);
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.lock().expect("第一次取锁应当成功");
            panic!("持锁时 panic —— 这正是中毒的来源");
        })
        .join();

        assert!(mutex.is_poisoned(), "前置条件：锁确实中毒了");
        // 关键断言：`expect` 会在这里 panic，而 helper 不会
        assert_eq!(*lock_or_recover(&mutex), vec![1]);
    }
}
