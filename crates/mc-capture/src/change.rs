//! 画面变化检测。
//!
//! 职责：决定「这一帧值不值得花钱分析」。
//!
//! 两级判定的第一级（元数据级，窗口/标题未变）在 [`crate::scheduler`] 里；
//! 这里是第二级：像素级 dHash。
//!
//! 为什么不用「每帧都调 VLM」：连续写代码时绝大多数画面是
//! 重复的，无差别调用正是 2 小时烧掉 300 万 token 的直接原因。

use image::imageops::{resize, FilterType};
use image::{GrayImage, Luma};
use mc_common::error::{AppError, ErrorCode};

/// 归一化坐标的矩形区域（0.0–1.0），用于忽略固定的 UI 区域。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Region {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Region {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// 校验是否落在画布内。非法区域必须报错 —— 静默忽略会让用户以为
    /// 「我明明配了忽略菜单栏，怎么还在抖」，却查不出原因。
    pub fn validate(&self) -> Result<(), String> {
        let finite = |v: f32| v.is_finite();
        if !finite(self.x) || !finite(self.y) || !finite(self.width) || !finite(self.height) {
            return Err(format!("区域包含非有限数值: {self:?}"));
        }
        if self.x < 0.0 || self.y < 0.0 {
            return Err(format!("区域起点不能为负: {self:?}"));
        }
        if self.width <= 0.0 || self.height <= 0.0 {
            return Err(format!("区域宽高必须为正: {self:?}"));
        }
        if self.x + self.width > 1.0 + f32::EPSILON || self.y + self.height > 1.0 + f32::EPSILON {
            return Err(format!("区域超出画布范围: {self:?}"));
        }
        Ok(())
    }

    /// 转成像素坐标 `[x0, y0)` 半开区间，并夹在图像边界内。
    fn to_pixels(self, width: u32, height: u32) -> (u32, u32, u32, u32) {
        let x0 = (self.x * width as f32).floor().max(0.0) as u32;
        let y0 = (self.y * height as f32).floor().max(0.0) as u32;
        let x1 = ((self.x + self.width) * width as f32)
            .ceil()
            .min(width as f32) as u32;
        let y1 = ((self.y + self.height) * height as f32)
            .ceil()
            .min(height as f32) as u32;
        (x0.min(width), y0.min(height), x1.max(x0), y1.max(y0))
    }

    fn contains_pixel(self, x: u32, y: u32, width: u32, height: u32) -> bool {
        let (x0, y0, x1, y1) = self.to_pixels(width, height);
        x >= x0 && x < x1 && y >= y0 && y < y1
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HashPolicy {
    /// dHash 汉明距离阈值：≤ 阈值视为「梯度方向没有显著变化」
    pub hamming_threshold: u32,
    /// 单个 cell 的均值差超过多少算「这个格子变了」
    pub cell_delta_threshold: u8,
    /// 有多少个格子变了才算「画面显著变化」
    pub changed_cells_threshold: u32,
    /// 需要忽略的固定区域（菜单栏、任务栏、时钟等）
    pub ignore_regions: Vec<Region>,
}

impl Default for HashPolicy {
    fn default() -> Self {
        Self {
            // 与 mc-config 的 capture.phash_hamming_threshold 默认值保持一致
            hamming_threshold: 4,
            cell_delta_threshold: 8,
            changed_cells_threshold: 8,
            ignore_regions: Vec::new(),
        }
    }
}

/// 相对上一帧的变化种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    /// 没有前一帧（新的采集目标或新会话）
    New,
    /// 元数据（窗口/标题）变了，但画面没变 → 只需更新元数据
    TitleOnly,
    /// 画面基本没变（含光标抖动、时钟跳动等）
    PixelMinor,
    /// 画面明显变了 → 值得分析
    PixelMajor,
    /// 用户空闲（无输入且画面不变）
    Idle,
    /// 无法判定
    Unknown,
}

impl ChangeKind {
    /// 是否需要把这一帧送去分析。
    pub const fn needs_analysis(self) -> bool {
        matches!(self, Self::New | Self::PixelMajor)
    }
}

/// 单帧签名。
///
/// 同时保留两类信号，因为单靠任一种都不够：
/// - `phash`（dHash）：对**梯度方向**敏感，对整体亮度不敏感 → 抗色温漂移
/// - `cells`（网格均值）：对**整块亮度/面积变化**敏感 → 补上 dHash 的盲区
///
/// 只保留 dHash 会在「深色主题里一整块面板变白」这类场景下漏判 ——
/// 梯度方向没变，哈希完全相同 —— 只比梯度会漏掉这类变化。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameStats {
    pub phash: u64,
    pub cells: [u8; CELL_COUNT],
    pub mean_luma: f32,
    pub luma_variance: f32,
    pub width: u32,
    pub height: u32,
}

impl FrameStats {
    /// 直接构造统计值（用于精确测试分类逻辑，不依赖图像内容）。
    /// `cells` 全 0 → 格差为 0，分类完全由哈希决定。
    pub const fn synthetic(phash: u64, mean_luma: f32, luma_variance: f32) -> Self {
        Self {
            phash,
            cells: [0u8; CELL_COUNT],
            mean_luma,
            luma_variance,
            width: 0,
            height: 0,
        }
    }

    /// dHash 汉明距离（梯度方向的差异）。
    pub const fn distance(&self, other: &Self) -> u32 {
        (self.phash ^ other.phash).count_ones()
    }

    /// 均值差超过 `threshold` 的格子数量（亮度/面积差异）。
    pub fn cell_distance(&self, other: &Self, threshold: u8) -> u32 {
        self.cells
            .iter()
            .zip(other.cells.iter())
            .filter(|(a, b)| a.abs_diff(**b) > threshold)
            .count() as u32
    }
}

pub trait ChangeDetector: Send + Sync {
    fn analyze(&self, image: &GrayImage) -> FrameStats;
    fn classify(
        &self,
        previous: Option<&FrameStats>,
        current: &FrameStats,
        metadata_changed: bool,
    ) -> ChangeKind;
    fn is_black_frame(&self, stats: &FrameStats) -> bool;
}

/// dHash 实现：缩放到 9×8，比较每行相邻像素。
#[derive(Debug, Clone)]
pub struct DHashDetector {
    policy: HashPolicy,
}

const HASH_WIDTH: u32 = 9;
const HASH_HEIGHT: u32 = 8;

/// 网格签名的分辨率：16×16 = 256 个格子。
pub const CELL_GRID: u32 = 16;
pub const CELL_COUNT: usize = (CELL_GRID * CELL_GRID) as usize;

/// 纯黑判定阈值。权限未生效时 macOS 会返回全黑帧，必须识别为**采集失败**，
/// 而不是「画面没变」—— 否则用户会看到「一切正常但什么都没记录」。
const BLACK_MEAN_LUMA_MAX: f32 = 6.0;
const BLACK_VARIANCE_MAX: f32 = 4.0;

impl DHashDetector {
    pub fn new(policy: HashPolicy) -> Result<Self, AppError> {
        for region in &policy.ignore_regions {
            region.validate().map_err(|reason| {
                AppError::new(ErrorCode::ConfigInvalid, format!("忽略区域非法：{reason}"))
            })?;
        }
        Ok(Self { policy })
    }

    pub fn policy(&self) -> &HashPolicy {
        &self.policy
    }
}

impl ChangeDetector for DHashDetector {
    fn analyze(&self, image: &GrayImage) -> FrameStats {
        let width = image.width();
        let height = image.height();

        let (mean_luma, luma_variance) = luma_stats(image);

        if width == 0 || height == 0 {
            return FrameStats {
                phash: 0,
                cells: [0u8; CELL_COUNT],
                mean_luma,
                luma_variance,
                width,
                height,
            };
        }

        let mut masked = image.clone();
        mask_regions(&mut masked, &self.policy.ignore_regions);

        let cells = cell_signature(&masked);
        let small = resize(&masked, HASH_WIDTH, HASH_HEIGHT, FilterType::Triangle);

        let mut hash = 0u64;
        for y in 0..HASH_HEIGHT {
            for x in 0..HASH_WIDTH - 1 {
                let left = small.get_pixel(x, y)[0];
                let right = small.get_pixel(x + 1, y)[0];
                if right > left {
                    let bit = y * (HASH_WIDTH - 1) + x;
                    hash |= 1u64 << bit;
                }
            }
        }

        FrameStats {
            phash: hash,
            cells,
            mean_luma,
            luma_variance,
            width,
            height,
        }
    }

    fn classify(
        &self,
        previous: Option<&FrameStats>,
        current: &FrameStats,
        metadata_changed: bool,
    ) -> ChangeKind {
        let Some(previous) = previous else {
            return ChangeKind::New;
        };

        let hash_ok = previous.distance(current) <= self.policy.hamming_threshold;
        let cells_ok = previous.cell_distance(current, self.policy.cell_delta_threshold)
            <= self.policy.changed_cells_threshold;
        let within_threshold = hash_ok && cells_ok;

        match (metadata_changed, within_threshold) {
            (true, true) => ChangeKind::TitleOnly,
            (true, false) | (false, false) => ChangeKind::PixelMajor,
            (false, true) => ChangeKind::PixelMinor,
        }
    }

    fn is_black_frame(&self, stats: &FrameStats) -> bool {
        stats.mean_luma <= BLACK_MEAN_LUMA_MAX && stats.luma_variance <= BLACK_VARIANCE_MAX
    }
}

/// 忽略区域用「未忽略像素的均值」填充。
///
/// 为什么用均值而不是 0：填 0 会在区域边界制造强边缘，反而主导哈希。
/// 填均值后区域内部相邻像素相等 → 该区域的比较位恒为 0，等价于不参与哈希。
fn mask_regions(image: &mut GrayImage, regions: &[Region]) {
    if regions.is_empty() {
        return;
    }
    let width = image.width();
    let height = image.height();

    let mut sum: u64 = 0;
    let mut count: u64 = 0;
    for (x, y, pixel) in image.enumerate_pixels() {
        if !regions
            .iter()
            .any(|r| r.contains_pixel(x, y, width, height))
        {
            sum += pixel[0] as u64;
            count += 1;
        }
    }

    let fill = sum.checked_div(count).unwrap_or(0) as u8;

    for (x, y, pixel) in image.enumerate_pixels_mut() {
        if regions
            .iter()
            .any(|r| r.contains_pixel(x, y, width, height))
        {
            *pixel = Luma([fill]);
        }
    }
}

/// 16×16 网格均值签名。
fn cell_signature(image: &GrayImage) -> [u8; CELL_COUNT] {
    let width = image.width();
    let height = image.height();
    let mut cells = [0u8; CELL_COUNT];

    if width == 0 || height == 0 {
        return cells;
    }

    for row in 0..CELL_GRID {
        for col in 0..CELL_GRID {
            let x0 = col * width / CELL_GRID;
            let x1 = ((col + 1) * width / CELL_GRID).max(x0 + 1).min(width);
            let y0 = row * height / CELL_GRID;
            let y1 = ((row + 1) * height / CELL_GRID).max(y0 + 1).min(height);

            let mut sum = 0u64;
            let mut count = 0u64;
            for y in y0..y1 {
                for x in x0..x1 {
                    sum += image.get_pixel(x, y)[0] as u64;
                    count += 1;
                }
            }

            let index = (row * CELL_GRID + col) as usize;
            cells[index] = sum.checked_div(count).unwrap_or(0) as u8;
        }
    }

    cells
}

/// 把彩色帧转成变化检测用的灰度图。
///
/// 用 BT.601 亮度权重（人眼对绿色最敏感），而不是简单平均 ——
/// 否则「红色警告条 vs 灰色条」这类变化会被低估。
pub fn rgb_to_luma(image: &image::RgbImage) -> GrayImage {
    GrayImage::from_fn(image.width(), image.height(), |x, y| {
        let p = image.get_pixel(x, y);
        let luma = (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
            .round()
            .clamp(0.0, 255.0) as u8;
        Luma([luma])
    })
}

fn luma_stats(image: &GrayImage) -> (f32, f32) {
    let count = (image.width() as u64) * (image.height() as u64);
    if count == 0 {
        return (0.0, 0.0);
    }

    let mut sum = 0f64;
    for pixel in image.pixels() {
        sum += pixel[0] as f64;
    }
    let mean = sum / count as f64;

    let mut variance = 0f64;
    for pixel in image.pixels() {
        let delta = pixel[0] as f64 - mean;
        variance += delta * delta;
    }
    variance /= count as f64;

    (mean as f32, variance as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_pixel_bounds_are_clamped_to_image() {
        let region = Region::new(0.0, 0.0, 1.0, 0.08);
        assert_eq!(region.to_pixels(320, 200), (0, 0, 320, 16));
    }

    #[test]
    fn region_validation_accepts_full_canvas() {
        assert!(Region::new(0.0, 0.0, 1.0, 1.0).validate().is_ok());
    }

    #[test]
    fn change_kind_needs_analysis_matches_scheduling_policy() {
        assert!(ChangeKind::New.needs_analysis());
        assert!(ChangeKind::PixelMajor.needs_analysis());
        assert!(!ChangeKind::PixelMinor.needs_analysis());
        assert!(!ChangeKind::TitleOnly.needs_analysis());
        assert!(!ChangeKind::Idle.needs_analysis());
    }

    #[test]
    fn zero_sized_image_is_handled() {
        let det = DHashDetector::new(HashPolicy::default()).unwrap();
        let empty = GrayImage::new(0, 0);
        let stats = det.analyze(&empty);
        assert_eq!(stats.width, 0);
        assert_eq!(stats.height, 0);
        assert_eq!(stats.phash, 0);
    }
}
