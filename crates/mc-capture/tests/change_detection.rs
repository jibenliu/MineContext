//! 变化检测。
//!
//! 这是整个采集层最该先做扎实的部分：
//! 它决定「要不要为这一帧花钱调 VLM」。的 dHash 参数存在但**没接线**，
//! 于是每 15 秒无脑调用模型 —— 2 小时烧掉 300 万 token 的直接原因之一。
//!
//! 全部是纯函数，不需要屏幕权限、不需要磁盘、不需要网络。

use image::{GrayImage, Luma};
use mc_capture::change::{ChangeDetector, ChangeKind, DHashDetector, HashPolicy, Region};

// ---------------------------------------------------------------- 测试图像构造

fn solid(width: u32, height: u32, luma: u8) -> GrayImage {
    GrayImage::from_pixel(width, height, Luma([luma]))
}

/// 画一个矩形（模拟窗口、菜单栏、光标等）。
fn rect(img: &mut GrayImage, x: u32, y: u32, w: u32, h: u32, luma: u8) {
    for py in y..(y + h).min(img.height()) {
        for px in x..(x + w).min(img.width()) {
            img.put_pixel(px, py, Luma([luma]));
        }
    }
}

/// 给每个像素加一点确定性噪声，模拟有损压缩（WebP/JPEG）造成的像素漂移。
fn add_noise(img: &mut GrayImage, amplitude: i16) {
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    for pixel in img.pixels_mut() {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let delta = ((state >> 33) as i16 % (2 * amplitude + 1)) - amplitude;
        let value = (pixel[0] as i16 + delta).clamp(0, 255) as u8;
        *pixel = Luma([value]);
    }
}

/// 一张「像 IDE 的界面」：深色背景 + 侧栏 + 编辑区 + 顶部菜单栏。
fn ide_screen() -> GrayImage {
    let mut img = solid(320, 200, 30);
    rect(&mut img, 0, 0, 320, 16, 60); // 菜单栏
    rect(&mut img, 0, 16, 60, 184, 45); // 侧栏
    rect(&mut img, 70, 30, 200, 120, 80); // 编辑区
    rect(&mut img, 70, 160, 200, 20, 55); // 终端
    img
}

fn detector() -> DHashDetector {
    DHashDetector::new(HashPolicy::default()).expect("默认策略必须可用")
}

// ---------------------------------------------------------------- 1.1 完全相同

#[test]
fn identical_image_is_unchanged() {
    let det = detector();
    let a = ide_screen();
    let stats_a = det.analyze(&a);
    let stats_b = det.analyze(&a);

    assert_eq!(stats_a.phash, stats_b.phash, "同一张图必须得到相同哈希");
    assert_eq!(
        det.classify(None, &stats_a, false),
        ChangeKind::New,
        "没有前一帧时应当是 New"
    );
    assert_eq!(
        det.classify(Some(&stats_a), &stats_b, false),
        ChangeKind::PixelMinor,
        "像素完全一致应当判为无需分析"
    );
}

// ---------------------------------------------------------------- 1.2 只有光标变化

#[test]
fn cursor_only_change_is_pixel_minor() {
    let det = detector();
    let before = ide_screen();
    let mut after = before.clone();
    rect(&mut after, 180, 90, 3, 8, 240); // 一个很小的光标

    let stats_before = det.analyze(&before);
    let stats_after = det.analyze(&after);

    // 注意：这里刻意**不**断言「哈希一定改变」。
    // 3x8 的光标在 320x200 → 9x8 的降采样中会被平均掉，哈希可能完全不变 ——
    // 这正是我们想要的：光标抖动不该触发模型调用。
    // 「检测器不是瞎的」由 1.3（换窗口 → PixelMajor）保证。
    assert_eq!(
        det.classify(Some(&stats_before), &stats_after, false),
        ChangeKind::PixelMinor,
        "光标级别的差异不应触发 VLM 分析"
    );
}

/// 阈值边界：距离恰好等于阈值 → 视为「没变」；超过 1 位 → 视为「变了」。
/// 这是分类逻辑的精确规格，不依赖任何图像内容。
#[test]
fn classify_respects_hamming_threshold_boundary() {
    let policy = HashPolicy {
        hamming_threshold: 4,
        ignore_regions: vec![],
        ..Default::default()
    };
    let det = DHashDetector::new(policy).unwrap();

    let base = mc_capture::change::FrameStats::synthetic(0b0000_0000, 100.0, 50.0);
    let at_threshold = mc_capture::change::FrameStats::synthetic(0b0000_1111, 100.0, 50.0);
    let over_threshold = mc_capture::change::FrameStats::synthetic(0b0001_1111, 100.0, 50.0);

    assert_eq!(
        det.classify(Some(&base), &at_threshold, false),
        ChangeKind::PixelMinor,
        "距离 == 阈值 应判为无显著变化"
    );
    assert_eq!(
        det.classify(Some(&base), &over_threshold, false),
        ChangeKind::PixelMajor,
        "距离 == 阈值 + 1 应判为显著变化"
    );
}

// ---------------------------------------------------------------- 1.3 换窗口

#[test]
fn new_window_is_pixel_major() {
    let det = detector();
    let ide = ide_screen();
    let browser = {
        let mut img = solid(320, 200, 240);
        rect(&mut img, 0, 0, 320, 30, 200);
        rect(&mut img, 20, 50, 280, 130, 255);
        img
    };

    let stats_ide = det.analyze(&ide);
    let stats_browser = det.analyze(&browser);

    assert_eq!(
        det.classify(Some(&stats_ide), &stats_browser, false),
        ChangeKind::PixelMajor,
        "明显不同的画面必须触发分析"
    );
}

// ---------------------------------------------------------------- 1.4 只有标题变

#[test]
fn metadata_change_skips_pixel_compare() {
    let det = detector();
    let frame = ide_screen();
    let stats = det.analyze(&frame);

    assert_eq!(
        det.classify(Some(&stats), &stats, true),
        ChangeKind::TitleOnly,
        "像素没变但窗口标题变了 → 只需更新元数据，不必分析图像"
    );
}

#[test]
fn metadata_and_pixel_change_is_major() {
    let det = detector();
    let before = ide_screen();
    let mut after = before.clone();
    rect(&mut after, 70, 30, 200, 120, 255);

    let stats_before = det.analyze(&before);
    let stats_after = det.analyze(&after);

    assert_eq!(
        det.classify(Some(&stats_before), &stats_after, true),
        ChangeKind::PixelMajor
    );
}

// ---------------------------------------------------------------- 1.5 忽略区域

#[test]
fn ignore_regions_exclude_menu_bar() {
    // 只忽略顶部 8% 高度的菜单栏（时钟、电量、通知图标常在这里闪）
    let policy = HashPolicy {
        hamming_threshold: 2,
        ignore_regions: vec![Region::new(0.0, 0.0, 1.0, 0.08)],
        ..Default::default()
    };
    let det = DHashDetector::new(policy).unwrap();

    let before = ide_screen();
    let mut after = before.clone();
    rect(&mut after, 280, 2, 30, 12, 255); // 菜单栏里的时钟数字变化

    let stats_before = det.analyze(&before);
    let stats_after = det.analyze(&after);

    assert_eq!(
        det.classify(Some(&stats_before), &stats_after, false),
        ChangeKind::PixelMinor,
        "被忽略区域内的变化不应触发分析"
    );
}

#[test]
fn changes_outside_ignored_regions_still_detected() {
    let policy = HashPolicy {
        hamming_threshold: 2,
        ignore_regions: vec![Region::new(0.0, 0.0, 1.0, 0.08)],
        ..Default::default()
    };
    let det = DHashDetector::new(policy).unwrap();

    let before = ide_screen();
    let mut after = before.clone();
    rect(&mut after, 70, 30, 200, 120, 255); // 编辑区整块变白

    let stats_before = det.analyze(&before);
    let stats_after = det.analyze(&after);

    assert_eq!(
        det.classify(Some(&stats_before), &stats_after, false),
        ChangeKind::PixelMajor,
        "忽略菜单栏不能把编辑区的变化也一起忽略掉"
    );
}

#[test]
fn invalid_region_is_rejected_not_silently_ignored() {
    for region in [
        Region::new(-0.1, 0.0, 1.0, 1.0),
        Region::new(0.0, 0.0, 1.5, 1.0),
        Region::new(0.5, 0.5, 0.0, 0.1),
    ] {
        let policy = HashPolicy {
            hamming_threshold: 2,
            ignore_regions: vec![region],
            ..Default::default()
        };
        assert!(
            DHashDetector::new(policy).is_err(),
            "非法忽略区域必须报错，而不是被静默忽略：{region:?}"
        );
    }
}

// ---------------------------------------------------------------- 1.6 压缩鲁棒性

#[test]
fn phash_is_stable_under_compression_like_noise() {
    let det = detector();
    let original = ide_screen();
    let mut compressed = original.clone();
    add_noise(&mut compressed, 6); // 模拟有损压缩造成的像素漂移

    let stats_original = det.analyze(&original);
    let stats_compressed = det.analyze(&compressed);

    assert_eq!(
        det.classify(Some(&stats_original), &stats_compressed, false),
        ChangeKind::PixelMinor,
        "有损压缩不应让同一画面被判定为「变了」"
    );
}

#[test]
fn phash_distinguishes_genuinely_different_screens() {
    let det = detector();
    let distance = |a: &mc_capture::change::FrameStats, b: &mc_capture::change::FrameStats| {
        (a.phash ^ b.phash).count_ones()
    };

    let ide = det.analyze(&ide_screen());
    let uniform = det.analyze(&solid(320, 200, 10));

    // 有结构 vs 无结构：必须显著不同
    assert!(
        distance(&ide, &uniform) > 4,
        "有内容的画面与纯色画面不应相似：{}",
        distance(&ide, &uniform)
    );

    // 两种不同布局：必须被判定为「变了」。
    //
    // 注意这里用的是 classify 而不是哈希距离：dHash 是**梯度**哈希，
    // 两块纯色区域的边界方向相同（亮→暗）时哈希完全相同。
    // 因此「亮度/面积变化」必须由 cell 均值差来捕捉，见下方 cell 相关测试。
    let mut layout_a = solid(320, 200, 30);
    rect(&mut layout_a, 0, 0, 160, 200, 200);
    let mut layout_b = solid(320, 200, 30);
    rect(&mut layout_b, 0, 0, 320, 100, 200);

    let stats_a = det.analyze(&layout_a);
    let stats_b = det.analyze(&layout_b);

    assert_eq!(
        det.classify(Some(&stats_a), &stats_b, false),
        ChangeKind::PixelMajor,
        "不同布局必须被区分（靠 cell 均值差，而不是梯度哈希）"
    );
}

/// dHash 捕捉不到「整块变亮」这类变化，cell 均值差必须补上这个能力。
#[test]
fn uniform_region_brightness_change_is_detected_via_cells() {
    let det = detector();
    let before = ide_screen();
    let mut after = before.clone();
    rect(&mut after, 70, 30, 200, 120, 255); // 编辑区整块由 80 变 255（梯度方向不变）

    let stats_before = det.analyze(&before);
    let stats_after = det.analyze(&after);

    // dHash 只反映梯度方向，对「整块变亮」这个事实本身并不敏感：
    // 它可能因为区域边界处的插值变化而改变少数几位，但**不能依赖它**。
    // 真正可靠的是 cell 均值差 —— 这正是引入第二个信号的原因。
    assert!(
        stats_before.cell_distance(&stats_after, 8) > 8,
        "cell 均值差必须能发现整块变亮：{}",
        stats_before.cell_distance(&stats_after, 8)
    );
    assert_eq!(
        det.classify(Some(&stats_before), &stats_after, false),
        ChangeKind::PixelMajor,
        "整块变亮必须触发分析"
    );
}

/// 全局轻微亮度漂移（例如夜间色温）不应被当成变化。
#[test]
fn global_brightness_drift_is_not_a_change() {
    let det = detector();
    let before = ide_screen();
    let mut after = before.clone();
    for pixel in after.pixels_mut() {
        *pixel = Luma([(pixel[0] as i16 + 3).clamp(0, 255) as u8]);
    }

    let stats_before = det.analyze(&before);
    let stats_after = det.analyze(&after);

    assert_eq!(
        det.classify(Some(&stats_before), &stats_after, false),
        ChangeKind::PixelMinor,
        "全局 +3 的亮度漂移不应触发分析"
    );
}

/// dHash 的固有性质：纯色画面（无论明暗）都得到全 0 位。
/// 因此「亮度」必须由 mean_luma 承担，不能指望 phash。
/// 明确把它固化成测试，避免以后有人误以为哈希能区分明暗。
#[test]
fn uniform_frames_hash_equally_but_differ_in_mean_luma() {
    let det = detector();
    let dark = det.analyze(&solid(320, 200, 10));
    let light = det.analyze(&solid(320, 200, 250));

    assert_eq!(
        dark.phash, light.phash,
        "纯色图的 dHash 都是全 0（算法固有性质）"
    );
    assert!(
        (light.mean_luma - dark.mean_luma).abs() > 200.0,
        "亮度差异必须体现在 mean_luma 上"
    );
}

// ---------------------------------------------------------------- 1.7 退化尺寸

#[test]
fn phash_never_panics_on_tiny_image() {
    let det = detector();
    for (w, h) in [(1, 1), (1, 8), (8, 1), (2, 3), (9, 8)] {
        let stats = det.analyze(&solid(w, h, 128));
        assert_eq!(stats.width, w);
        assert_eq!(stats.height, h);
    }
}

#[test]
fn phash_never_panics_on_ultrawide_image() {
    let det = detector();
    let stats = det.analyze(&solid(4096, 1, 128));
    assert_eq!(stats.width, 4096);
    assert_eq!(stats.height, 1);
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig { cases: 64, ..Default::default() })]

    // 任意尺寸（含极端长宽比）都不得 panic
    #[test]
    fn phash_never_panics_on_arbitrary_size(width in 1u32..64, height in 1u32..64, luma in 0u8..=255) {
        let det = detector();
        let stats = det.analyze(&solid(width, height, luma));
        proptest::prop_assert_eq!(stats.width, width);
        proptest::prop_assert_eq!(stats.height, height);
    }

    // 哈希必须只依赖内容，不依赖调用次数（无隐藏状态）
    #[test]
    fn phash_is_deterministic(luma in 0u8..=255, seed in 0u64..1_000) {
        let mut img = solid(64, 48, luma);
        rect(&mut img, (seed % 40) as u32, (seed % 30) as u32, 8, 8, luma.wrapping_add(60));
        let det = detector();
        proptest::prop_assert_eq!(det.analyze(&img).phash, det.analyze(&img).phash);
    }
}

// ---------------------------------------------------------------- 1.8 黑帧检测

#[test]
fn black_frame_is_detected_as_capture_failure() {
    let det = detector();

    let black = det.analyze(&solid(320, 200, 0));
    assert!(
        det.is_black_frame(&black),
        "纯黑帧通常意味着屏幕录制权限未生效，必须被识别为采集失败而不是「画面没变」"
    );

    let normal = det.analyze(&ide_screen());
    assert!(!det.is_black_frame(&normal), "正常画面不应被误判为黑帧");

    // 极暗但仍有内容的画面（例如深色主题终端）不应被误判
    let mut dark_terminal = solid(320, 200, 12);
    rect(&mut dark_terminal, 10, 10, 300, 180, 40);
    assert!(
        !det.is_black_frame(&det.analyze(&dark_terminal)),
        "深色主题不应被误判为黑帧（否则用户会看到假的权限错误）"
    );
}

#[test]
fn default_policy_matches_documented_defaults() {
    let policy = HashPolicy::default();
    assert_eq!(
        policy.hamming_threshold, 4,
        "默认阈值应与 config 里的默认值一致"
    );
    assert!(policy.ignore_regions.is_empty());
}
