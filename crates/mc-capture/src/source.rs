//! 采集源抽象。
//!
//! 设计目标：**采集来源必须可插拔**，而不是把截图写进核心。
//!
//! 窗口元数据与截图是平级的一等公民：截图的信息密度低，
//! 能用廉价元数据判断的，就不要去分析图像。

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use image::RgbImage;
use mc_common::error::AppError;
use mc_common::time::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Screen,
    Window,
    Clipboard,
    File,
    Browser,
}

impl SourceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Screen => "screen",
            Self::Window => "window",
            Self::Clipboard => "clipboard",
            Self::File => "file",
            Self::Browser => "browser",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    Screen,
    Window,
}

/// 源的能力声明。调度器据此决定「能不能只靠元数据判断」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceCapabilities {
    pub produces_image: bool,
    pub produces_text: bool,
    /// 是否需要用户授予系统权限（屏幕录制等）
    pub needs_permission_granted: bool,
}

impl SourceCapabilities {
    pub const SCREEN: Self = Self {
        produces_image: true,
        produces_text: false,
        needs_permission_granted: true,
    };

    pub const TEXT_ONLY: Self = Self {
        produces_image: false,
        produces_text: true,
        needs_permission_granted: false,
    };

    /// 窗口元数据（应用名 + 标题）：不产图像，但**需要屏幕录制权限** ——
    /// macOS 从 10.15 起把窗口标题也纳入了该权限。
    pub const WINDOW_METADATA: Self = Self {
        produces_image: false,
        produces_text: true,
        needs_permission_granted: true,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionState {
    Granted,
    Denied,
    NotRequired,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceHealth {
    pub available: bool,
    pub permission: PermissionState,
    pub message: Option<String>,
}

/// 目标在**虚拟屏坐标系**里的位置与尺寸。
///
/// 区域采集要用它把「用户给的矩形」换算成显示器内的像素范围。
/// 源拿不到几何信息时为 `None`，调用方据此降级（而不是假设从 (0,0) 开始）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetBounds {
    pub x: i64,
    pub y: i64,
    pub width: u32,
    pub height: u32,
}

impl TargetBounds {
    /// 该目标在虚拟屏坐标系里覆盖的范围（半开区间）。
    pub const fn right(self) -> i64 {
        self.x + self.width as i64
    }

    pub const fn bottom(self) -> i64 {
        self.y + self.height as i64
    }

    pub const fn contains_point(self, x: i64, y: i64) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// 与一块区域的重叠部分（显示器内局部像素范围）。无重叠时为 `None`。
    pub fn intersect(self, region: crate::geometry::Rect) -> Option<(u32, u32, u32, u32)> {
        let left = region.left.max(self.x);
        let top = region.top.max(self.y);
        let right = region.right.min(self.right());
        let bottom = region.bottom.min(self.bottom());
        if right <= left || bottom <= top {
            return None;
        }
        Some((
            (left - self.x) as u32,
            (top - self.y) as u32,
            (right - left) as u32,
            (bottom - top) as u32,
        ))
    }
}

/// 可采集的具体对象（一块屏、一个窗口）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureTarget {
    pub id: String,
    pub name: String,
    pub kind: TargetKind,
    /// Retina / HiDPI 缩放倍率（1.0 = 普通屏）
    pub scale_factor: f32,
    pub display_id: Option<String>,
    pub window_id: Option<u64>,
    pub app_name: Option<String>,
    pub window_title: Option<String>,
    pub is_visible: bool,
    /// 几何信息（源能提供时才有）。区域采集依赖它。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds: Option<TargetBounds>,
}

impl CaptureTarget {
    pub fn screen(id: impl Into<String>, name: impl Into<String>, scale_factor: f32) -> Self {
        let id = id.into();
        Self {
            display_id: Some(id.clone()),
            id,
            name: name.into(),
            kind: TargetKind::Screen,
            scale_factor,
            window_id: None,
            app_name: None,
            window_title: None,
            is_visible: true,
            bounds: None,
        }
    }

    pub fn window(
        id: impl Into<String>,
        title: impl Into<String>,
        app_name: impl Into<String>,
    ) -> Self {
        let title = title.into();
        Self {
            id: id.into(),
            name: title.clone(),
            kind: TargetKind::Window,
            scale_factor: 1.0,
            display_id: None,
            window_id: None,
            app_name: Some(app_name.into()),
            // 窗口标题必须单独保留：规则匹配靠它，
            // 不能只塞进展示用的 name 里。
            window_title: Some(title),
            is_visible: true,
            bounds: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextEvidence {
    pub text: String,
    pub origin: TextOrigin,
    pub confidence: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextOrigin {
    Clipboard,
    Accessibility,
    Ocr,
}

/// 一次采集的原始产出。
///
/// 注意：`image` 是**证据**，不是唯一输入。窗口标题/进程名这类廉价元数据
/// 单独放在 `target` 里，便于调度器在不看图像的情况下做判断。
#[derive(Debug, Clone)]
pub struct RawCapture {
    /// 产出这一帧的源 id 与类型。
    ///
    /// 逐帧记录而不是靠「调用方知道自己在轮询谁」：多个源合并采集时，
    /// 观测里必须能看出「这条是截图还是窗口」，否则出了问题只能靠猜。
    pub source_id: String,
    pub source_kind: SourceKind,
    pub target: CaptureTarget,
    pub captured_at: Timestamp,
    pub image: Option<RgbImage>,
    pub text: Option<TextEvidence>,
}

impl RawCapture {
    /// 由源自己补齐来源字段，避免每个构造点手写两遍。
    pub fn from_source(
        source: &dyn CaptureSource,
        target: CaptureTarget,
        captured_at: Timestamp,
        image: Option<RgbImage>,
        text: Option<TextEvidence>,
    ) -> Self {
        Self {
            source_id: source.id().to_string(),
            source_kind: source.kind(),
            target,
            captured_at,
            image,
            text,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CaptureContext {
    /// 选中的 target id；空表示「全部」
    pub targets: Vec<String>,
    /// **注入的时间**，而不是源内部读时钟 —— 这样防抖/空闲/跨天都能确定性测试
    pub now: Timestamp,
    pub idle: bool,
}

impl CaptureContext {
    pub fn at(now: Timestamp) -> Self {
        Self {
            targets: Vec::new(),
            now,
            idle: false,
        }
    }

    pub fn wants(&self, target_id: &str) -> bool {
        self.targets.is_empty() || self.targets.iter().any(|id| id == target_id)
    }
}

#[async_trait]
pub trait CaptureSource: Send + Sync {
    /// 稳定 id，用于规则匹配、诊断与配置引用（例如 `screen:display-1`）
    fn id(&self) -> &str;
    fn kind(&self) -> SourceKind;
    fn capabilities(&self) -> SourceCapabilities;

    /// 列出当前可采集的对象（UI 的「采集源设置」用它）
    async fn enumerate(&self) -> Result<Vec<CaptureTarget>, AppError>;

    /// 采样一次。允许返回空（无显示器、锁屏、无变化、被隐私规则拦截）。
    async fn poll(&self, ctx: &CaptureContext) -> Result<Vec<RawCapture>, AppError>;

    /// 权限与可用性自检（UI 据此决定是否弹引导）
    async fn health(&self) -> SourceHealth;

    /// 设置页预览缩略图：`target_id → data:image/png;base64,...`。
    ///
    /// 默认空：多数测试假源与不可采集平台不需要；真机屏幕/窗口源覆盖它。
    /// 单张失败时跳过该 id，不要让整份目标列表失败。
    async fn preview_thumbnails(
        &self,
        _max_width: u32,
    ) -> std::collections::HashMap<String, String> {
        std::collections::HashMap::new()
    }
}

/// 采集源注册表。
#[derive(Default)]
pub struct SourceRegistry {
    sources: Vec<Arc<dyn CaptureSource>>,
    last_errors: BTreeMap<String, String>,
}

impl SourceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, source: Arc<dyn CaptureSource>) {
        self.sources.push(source);
    }

    pub fn len(&self) -> usize {
        self.sources.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    pub fn by_kind(&self, kind: SourceKind) -> Option<&Arc<dyn CaptureSource>> {
        self.sources.iter().find(|s| s.kind() == kind)
    }

    pub fn all(&self) -> &[Arc<dyn CaptureSource>] {
        &self.sources
    }

    /// 上一次 `poll_all` 中失败来源及其原因（诊断用）。
    pub fn last_errors(&self) -> &BTreeMap<String, String> {
        &self.last_errors
    }

    /// 轮询所有源并合并结果。
    ///
    /// **单个源失败不影响其它源** —— 否则剪贴板权限问题会连带停掉截图。
    /// 失败原因记录在 `last_errors`，而不是被吞掉。
    pub async fn poll_all(&mut self, ctx: &CaptureContext) -> Vec<RawCapture> {
        self.last_errors.clear();
        let mut merged = Vec::new();

        for source in &self.sources {
            match source.poll(ctx).await {
                Ok(mut captures) => merged.append(&mut captures),
                Err(error) => {
                    self.last_errors
                        .insert(source.id().to_string(), error.detail().to_string());
                }
            }
        }

        merged
    }
}
