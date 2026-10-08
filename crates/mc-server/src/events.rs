//! 控制面事件总线。
//!
//! 前端要「总结刚生成就出现在时间线上」，轮询既慢又费；这里给 daemon 一个
//! 进程内广播：生产者发布，SSE 连接转发。三个取舍：
//!
//! - **广播而不是队列**：慢客户端不该拖住生产者，缓冲满时丢弃最旧的，
//!   掉帧的客户端靠下一次全量拉取补齐；
//! - **事件里带完整对象**：前端收到就能渲染，但受 SSE 帧大小限制只放必要字段；
//! - **发布失败不是错误**：没有订阅者时发布就是空操作。

use serde_json::Value;
use tokio::sync::broadcast;

/// 事件类型。字符串就是 SSE 的 `event:` 名，与前端 `channel-map.ts`
/// 里的订阅表一一对应。
pub const EVENT_SUMMARY_CREATED: &str = "summary:created";
pub const EVENT_ACTIVITY_CREATED: &str = "activity:created";
pub const EVENT_STAGE_CLOSED: &str = "stage:closed";
/// 渲染层既有的推送渠道（沿用名字，避免改动渲染层）
pub const EVENT_PUSH_LATEST_ACTIVITY: &str = "push:latest-activity";
pub const EVENT_PUSH_INIT_CHECK_DATA: &str = "push:init-check-data";
/// 录制开关状态变化（`"running"` / `"stopped"`，**裸字符串**）。
/// 渲染层的屏幕监控页拿它同步「正在录制」；托盘切换录制时页面没有别的信息来源。
pub const EVENT_PUSH_SCREEN_MONITOR_STATUS: &str = "push:screen-monitor-status";

/// 锁屏 / 休眠状态变化。渲染层用它暂停轮询、锁屏时停止轮询；
/// 载荷形状是渲染层定的：`{ eventKey, data }`，`eventKey` 取
/// `power-monitor.ts` 里那四个值（suspend / resume / lock-screen / unlock-screen）。
pub const EVENT_PUSH_POWER_MONITOR: &str = "push:power-monitor";

/// 锁屏与休眠的四个事件名（与渲染层的枚举逐字对齐）。
pub const POWER_MONITOR_LOCK_SCREEN: &str = "lock-screen";
pub const POWER_MONITOR_UNLOCK_SCREEN: &str = "unlock-screen";
pub const POWER_MONITOR_SUSPEND: &str = "suspend";
pub const POWER_MONITOR_RESUME: &str = "resume";

/// 广播缓冲。测试里会塞几十条，真实场景下每条都有订阅者跟读，不会积压。
const BUFFER: usize = 256;

#[derive(Debug, Clone, PartialEq)]
pub struct ServerEvent {
    pub kind: String,
    pub data: Value,
}

#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<ServerEvent>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(BUFFER);
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ServerEvent> {
        self.tx.subscribe()
    }

    /// 发布事件。**没有订阅者时返回 false**，但这不算错误。
    pub fn publish(&self, kind: impl Into<String>, data: Value) -> bool {
        self.tx
            .send(ServerEvent {
                kind: kind.into(),
                data,
            })
            .is_ok()
    }

    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }
}
