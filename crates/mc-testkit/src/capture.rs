//! 假的采集源。
//!
//! 它让整个采集层的关键语义（权限被拒、无显示器、锁屏、多屏、Retina、顺序）
//! 都能在**任何机器**上确定性测试，不需要屏幕录制权限、不需要真实显示器。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use image::{Rgb, RgbImage};
use mc_capture::source::{
    CaptureContext, CaptureSource, CaptureTarget, PermissionState, RawCapture, SourceCapabilities,
    SourceHealth, SourceKind, TargetKind,
};
use mc_common::error::{AppError, ErrorCode};
use mc_common::time::Timestamp;

#[derive(Default)]
pub struct FakeCaptureSourceBuilder {
    kind: Option<SourceKind>,
    targets: Vec<CaptureTarget>,
    permission: Option<PermissionState>,
    capabilities: Option<SourceCapabilities>,
    image_size: Option<(u32, u32)>,
    changing: bool,
}

impl FakeCaptureSourceBuilder {
    pub fn screen(mut self, id: &str, name: &str, scale_factor: f32) -> Self {
        self.targets
            .push(CaptureTarget::screen(id, name, scale_factor));
        self
    }

    pub fn window(mut self, id: &str, title: &str, app_name: &str) -> Self {
        self.targets
            .push(CaptureTarget::window(id, title, app_name));
        self
    }

    pub fn kind(mut self, kind: SourceKind) -> Self {
        self.kind = Some(kind);
        self
    }

    pub fn permission(mut self, permission: PermissionState) -> Self {
        self.permission = Some(permission);
        self
    }

    pub fn capability(mut self, capabilities: SourceCapabilities) -> Self {
        self.capabilities = Some(capabilities);
        self
    }

    pub fn image_size(mut self, width: u32, height: u32) -> Self {
        self.image_size = Some((width, height));
        self
    }

    /// 每次 poll 产出不同内容，用于验证「有意义的变化会被持久化」。
    pub fn changing(mut self) -> Self {
        self.changing = true;
        self
    }

    pub fn build(self) -> Result<FakeCaptureSource, AppError> {
        let kind = self.kind.unwrap_or(SourceKind::Screen);
        let capabilities = self.capabilities.unwrap_or(match kind {
            SourceKind::Clipboard | SourceKind::File | SourceKind::Browser => {
                SourceCapabilities::TEXT_ONLY
            }
            _ => SourceCapabilities::SCREEN,
        });
        let permission = self
            .permission
            .unwrap_or(if capabilities.needs_permission_granted {
                PermissionState::Granted
            } else {
                PermissionState::NotRequired
            });

        Ok(FakeCaptureSource {
            id: format!("fake:{}", kind.as_str()),
            kind,
            capabilities,
            permission,
            targets: self.targets,
            locked: AtomicBool::new(false),
            image_size: self.image_size.unwrap_or((64, 40)),
            poll_count: AtomicU64::new(0),
            changing: self.changing,
        })
    }
}

pub struct FakeCaptureSource {
    id: String,
    kind: SourceKind,
    capabilities: SourceCapabilities,
    permission: PermissionState,
    targets: Vec<CaptureTarget>,
    locked: AtomicBool,
    image_size: (u32, u32),
    poll_count: AtomicU64,
    changing: bool,
}

impl FakeCaptureSource {
    pub fn builder() -> FakeCaptureSourceBuilder {
        FakeCaptureSourceBuilder::default()
    }

    /// 模拟锁屏/解锁。
    pub fn set_locked(&self, locked: bool) {
        self.locked.store(locked, Ordering::SeqCst);
    }

    /// 每帧生成一张确定性的小图；内容随 target 序号变化，便于断言「不同屏内容不同」。
    fn render(&self, target: &CaptureTarget, poll: u64) -> RgbImage {
        let (width, height) = self.image_size;
        let base = self
            .targets
            .iter()
            .position(|t| t.id == target.id)
            .unwrap_or(0) as u8;
        // 固定内容模式下 poll 不参与，保证「同一屏每帧相同」
        let seed = if self.changing {
            base.wrapping_add((poll % 200) as u8)
        } else {
            base
        };

        RgbImage::from_fn(width, height, |x, y| {
            let stripe = ((x / 8) + (y / 8)) as u8;
            Rgb([seed.wrapping_mul(40).wrapping_add(stripe), 60, 90])
        })
    }
}

#[async_trait::async_trait]
impl CaptureSource for FakeCaptureSource {
    fn id(&self) -> &str {
        &self.id
    }

    fn kind(&self) -> SourceKind {
        self.kind
    }

    fn capabilities(&self) -> SourceCapabilities {
        self.capabilities
    }

    async fn enumerate(&self) -> Result<Vec<CaptureTarget>, AppError> {
        Ok(self.targets.clone())
    }

    async fn poll(&self, ctx: &CaptureContext) -> Result<Vec<RawCapture>, AppError> {
        if self.permission == PermissionState::Denied {
            return Err(AppError::new(
                ErrorCode::CapturePermissionDenied,
                format!("{} 缺少屏幕录制权限", self.id),
            ));
        }

        // 锁屏期间不产出任何观测
        if self.locked.load(Ordering::SeqCst) {
            return Ok(Vec::new());
        }

        // 无显示器是正常状态，不是错误
        let selected: Vec<&CaptureTarget> = self
            .targets
            .iter()
            .filter(|t| t.kind == TargetKind::Screen || t.kind == TargetKind::Window)
            .filter(|t| ctx.wants(&t.id))
            .collect();

        if selected.is_empty() {
            return Ok(Vec::new());
        }

        // 每轮 poll 递增一次；只有 changing 模式下才会影响渲染内容
        let poll = self.poll_count.fetch_add(1, Ordering::SeqCst);

        Ok(selected
            .into_iter()
            .map(|target| {
                RawCapture::from_source(
                    self,
                    target.clone(),
                    ctx.now,
                    self.capabilities
                        .produces_image
                        .then(|| self.render(target, poll)),
                    None,
                )
            })
            .collect())
    }

    async fn health(&self) -> SourceHealth {
        let available =
            self.permission != PermissionState::Denied && !self.locked.load(Ordering::SeqCst);
        SourceHealth {
            available,
            permission: self.permission,
            message: if available {
                None
            } else if self.permission == PermissionState::Denied {
                Some("缺少屏幕录制权限".to_string())
            } else {
                Some("屏幕已锁定，采集已暂停".to_string())
            },
        }
    }
}

/// 便于测试构造时间戳。
pub fn at_ms(ms: i64) -> Timestamp {
    Timestamp::from_millis(ms)
}
