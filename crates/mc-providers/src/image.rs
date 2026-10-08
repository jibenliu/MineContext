//! 图像预处理。
//!
//! 两件事：
//! 1. 超过长边上限时缩放（省 token、省带宽）
//! 2. **保证 MIME 与实际编码一致** —— 否则无论文件是什么都声称
//!    `data:image/png;base64,...`，JPEG 场景会被部分端点直接拒绝。

use mc_common::error::{AppError, ErrorCode};

/// 返回 `(bytes, mime)`。输入已在上限内时原样返回，避免无谓的重编码。
pub fn prepare(
    bytes: &[u8],
    mime: &str,
    max_edge: Option<u32>,
) -> Result<(Vec<u8>, String), AppError> {
    let Some(max_edge) = max_edge else {
        return Ok((bytes.to_vec(), mime.to_string()));
    };

    let decoded = image::load_from_memory(bytes).map_err(|e| {
        AppError::new(
            ErrorCode::ProviderInvalidResponse,
            format!("无法解码待上传图像: {e}"),
        )
    })?;

    let (width, height) = (decoded.width(), decoded.height());
    let longest = width.max(height);

    if longest <= max_edge {
        // 不需要缩放：保持原始字节与 MIME（避免 JPEG 重编码造成质量损失）
        return Ok((bytes.to_vec(), mime.to_string()));
    }

    let scale = max_edge as f32 / longest as f32;
    let new_width = ((width as f32 * scale).round() as u32).max(1);
    let new_height = ((height as f32 * scale).round() as u32).max(1);

    let resized =
        decoded.resize_exact(new_width, new_height, image::imageops::FilterType::Triangle);

    // 统一编码为 JPEG：缩放后已经是重新编码，没必要再用无损格式占带宽
    let mut buffer = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buffer, 85)
        .encode(
            resized.to_rgb8().as_raw(),
            new_width,
            new_height,
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| {
            AppError::new(
                ErrorCode::ProviderInvalidResponse,
                format!("图像重编码失败: {e}"),
            )
        })?;

    Ok((buffer, "image/jpeg".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageEncoder;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbImage::from_pixel(width, height, image::Rgb([10, 20, 30]));
        let mut buffer = Vec::new();
        image::codecs::png::PngEncoder::new(&mut buffer)
            .write_image(
                image.as_raw(),
                width,
                height,
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
        buffer
    }

    #[test]
    fn small_images_are_not_reencoded() {
        let original = png(64, 48);
        let (bytes, mime) = prepare(&original, "image/png", Some(1024)).unwrap();
        assert_eq!(bytes, original, "未超限时不应重编码（避免质量损失）");
        assert_eq!(mime, "image/png");
    }

    #[test]
    fn large_images_are_downscaled_and_mime_updated() {
        let original = png(2048, 1024);
        let (bytes, mime) = prepare(&original, "image/png", Some(1024)).unwrap();

        assert_eq!(mime, "image/jpeg", "重编码后 MIME 必须跟着变");
        assert!(bytes.len() < original.len(), "缩放后体积应当变小");

        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!(decoded.width().max(decoded.height()), 1024);
    }

    #[test]
    fn no_limit_returns_input_unchanged() {
        let original = png(4000, 4000);
        let (bytes, mime) = prepare(&original, "image/png", None).unwrap();
        assert_eq!(bytes, original);
        assert_eq!(mime, "image/png");
    }

    #[test]
    fn garbage_input_is_a_typed_error() {
        let error = prepare(b"not an image", "image/png", Some(512)).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ProviderInvalidResponse);
    }
}
