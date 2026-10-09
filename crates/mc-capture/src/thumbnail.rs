//! 采集源预览缩略图（设置页「选择录制内容」用）。
//!
//! 产出 `data:image/png;base64,...`，前端 `<img src>` 可直接渲染。
//! 失败时返回 `None`，调用方保持 `thumbnail: null`，不要因此让目标列表失败。

use base64::Engine;
use image::{DynamicImage, RgbaImage};

/// 设置页屏幕格子是 94×60；2× 宽给 Retina 更清晰。
pub const DEFAULT_PREVIEW_WIDTH: u32 = 188;

/// RGBA 帧 → 缩小后的 PNG data URL。
pub fn rgba_to_data_url(image: &RgbaImage, max_width: u32) -> Option<String> {
    let max_width = max_width.max(1);
    let resized = if image.width() > max_width {
        let height = ((image.height() as u64 * max_width as u64) / u64::from(image.width().max(1)))
            .max(1) as u32;
        image::imageops::resize(
            image,
            max_width,
            height,
            image::imageops::FilterType::Triangle,
        )
    } else {
        image.clone()
    };

    let mut png = Vec::new();
    DynamicImage::ImageRgba8(resized)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;

    Some(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    ))
}

/// RGB 帧 → PNG data URL（假源与落库路径共用）。
pub fn rgb_to_data_url(image: &image::RgbImage, max_width: u32) -> Option<String> {
    let rgba = DynamicImage::ImageRgb8(image.clone()).to_rgba8();
    rgba_to_data_url(&rgba, max_width)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    #[test]
    fn rgb_preview_is_png_data_url() {
        let image = RgbImage::from_fn(32, 20, |x, y| Rgb([x as u8, y as u8, 90]));
        let url = rgb_to_data_url(&image, 16).expect("应能编码");
        assert!(
            url.starts_with("data:image/png;base64,"),
            "必须是 PNG data URL: {url}"
        );
        assert!(url.len() > 40, "载荷不应为空");
    }
}
