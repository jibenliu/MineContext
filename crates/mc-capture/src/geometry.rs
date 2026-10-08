//! 几何：区域矩形与坐标系换算。
//!
//! 单独成模块是因为「区域采集」的边界条件（跨屏、越界、空区域）最容易出错、
//! 也最值得穷举测试，而它与平台无关。

use serde::{Deserialize, Serialize};

/// 一块矩形区域，**虚拟屏坐标系**（左上角为原点，单位像素）。
///
/// 与配置项 `screenshot_region` 的 `[left, top, right, bottom]` 一致：
/// 沿用同一组边界值，用户不需要换算。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub left: i64,
    pub top: i64,
    pub right: i64,
    pub bottom: i64,
}

impl Rect {
    /// 从 `[left, top, right, bottom]` 构造。
    ///
    /// 顺序颠倒（`right <= left` 或 `bottom <= top`）视为无效：
    /// 手写配置很容易写反，静默接受只会得到一张空图。
    pub fn new(left: i64, top: i64, right: i64, bottom: i64) -> Option<Self> {
        if right <= left || bottom <= top {
            return None;
        }
        Some(Self {
            left,
            top,
            right,
            bottom,
        })
    }

    /// 从数组形式构造（配置文件里的 `[left, top, right, bottom]`）。
    pub fn from_array(values: [i64; 4]) -> Option<Self> {
        Self::new(values[0], values[1], values[2], values[3])
    }

    pub const fn width(self) -> u32 {
        (self.right - self.left) as u32
    }

    pub const fn height(self) -> u32 {
        (self.bottom - self.top) as u32
    }

    pub const fn contains_point(self, x: i64, y: i64) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_inverted_rectangles() {
        assert!(Rect::new(100, 0, 100, 50).is_none(), "零宽无效");
        assert!(Rect::new(0, 100, 50, 0).is_none(), "顺序写反无效");
        assert!(Rect::new(0, 0, 10, 10).is_some());
    }
}
