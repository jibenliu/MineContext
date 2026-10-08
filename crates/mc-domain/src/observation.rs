//! 活动引擎的输入：一条观测的**摘要**。
//!
//! 刻意不复用 `mc-capture` 的 `RawCapture`：
//! 领域层（层 1）不应依赖采集层（层 2），而且活动判定只需要元数据与文本，
//! 不需要像素。这也让「靠元数据判断、不调模型」在类型层面就成立
//! （标题与进程名往往比像素更能说明「在做什么」）。

use mc_common::time::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObservationSummary {
    pub id: String,
    pub at: Timestamp,
    pub app_name: Option<String>,
    pub window_title: Option<String>,
    /// 从窗口标题或浏览器地址提取的域名
    pub domain: Option<String>,
    /// OCR / 无障碍 / 剪贴板文本
    pub text: Option<String>,
}

impl ObservationSummary {
    /// 全部可匹配文本（标题 + 正文），小写化一次避免重复分配。
    pub fn haystack(&self) -> String {
        let mut out = String::new();
        if let Some(title) = &self.window_title {
            out.push_str(title);
            out.push('\n');
        }
        if let Some(text) = &self.text {
            out.push_str(text);
        }
        out
    }

    pub fn app(&self) -> &str {
        self.app_name.as_deref().unwrap_or("")
    }
}
