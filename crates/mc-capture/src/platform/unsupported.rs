//! 尚未实现的平台。
//!
//! 接口已经就位，因此补 Windows/Linux 时不需要动上层任何代码
//! （当前只实现 macOS，架构上为其它平台预留）。

use async_trait::async_trait;
use mc_common::error::{AppError, ErrorCode};

use crate::source::{
    CaptureContext, CaptureSource, CaptureTarget, PermissionState, RawCapture, SourceCapabilities,
    SourceHealth, SourceKind,
};

pub struct UnsupportedScreenSource {
    platform: &'static str,
}

impl UnsupportedScreenSource {
    pub fn new(platform: &'static str) -> Self {
        Self { platform }
    }
}

#[async_trait]
impl CaptureSource for UnsupportedScreenSource {
    fn id(&self) -> &str {
        "unsupported:screen"
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Screen
    }

    fn capabilities(&self) -> SourceCapabilities {
        SourceCapabilities::SCREEN
    }

    async fn enumerate(&self) -> Result<Vec<CaptureTarget>, AppError> {
        Ok(Vec::new())
    }

    async fn poll(&self, _ctx: &CaptureContext) -> Result<Vec<RawCapture>, AppError> {
        Err(AppError::new(
            ErrorCode::CaptureUnsupported,
            format!("当前平台（{}）尚未实现屏幕采集", self.platform),
        ))
    }

    async fn health(&self) -> SourceHealth {
        SourceHealth {
            available: false,
            permission: PermissionState::Unknown,
            message: Some(format!("{} 平台尚未实现屏幕采集", self.platform)),
        }
    }
}
