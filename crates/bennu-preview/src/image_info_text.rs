//! 图片信息层的各行：尺寸、格式、色深、透明、颜色配置、大小，有拍摄参数时再加拍摄时间、
//! 相机、镜头、焦距、光圈、快门、ISO、位置。读不到的行不写。

use bennu_localization::{format_file_size, tr};

use crate::info_overlay::InfoRow;
use crate::preview::{ImageInfo, ImagePreviewContent};

pub(crate) fn image_info_rows(content: &ImagePreviewContent, info: &ImageInfo) -> Vec<InfoRow> {
    let facts = &info.facts;
    let mut rows = Vec::new();
    let mut push = |label: String, value: String| rows.push(InfoRow { label, value });
    let (width, height) = content.dimensions();
    push(tr!("Dimensions"), dimensions_text(width, height));
    if let Some(format) = &facts.format {
        push(tr!("Format"), format.clone());
    }
    if let Some(bits) = facts.bits_per_channel {
        push(tr!("Bit depth"), tr!("%1-bit", bits));
    }
    if matches!(
        content,
        ImagePreviewContent::OriginalRaster {
            has_alpha: true,
            ..
        }
    ) {
        push(tr!("Transparency"), tr!("Has transparent areas"));
    }
    if let Some(profile) = &facts.color_profile {
        push(tr!("Color profile"), profile.clone());
    }
    push(tr!("Size"), format_file_size(info.byte_len));
    if let Some(taken_at) = &facts.taken_at {
        push(tr!("Taken"), taken_at.clone());
    }
    if let Some(camera) = &facts.camera {
        push(tr!("Camera"), camera.clone());
    }
    if let Some(lens) = &facts.lens {
        push(tr!("Lens"), lens.clone());
    }
    if let Some(focal_length) = facts.focal_length_mm {
        let length = trimmed_number(focal_length);
        push(
            tr!("Focal length"),
            match facts.focal_length_35mm {
                Some(equivalent) if equivalent.to_string() != length => {
                    tr!("%1 mm (%2 mm equivalent)", length, equivalent)
                }
                _ => tr!("%1 mm", length),
            },
        );
    }
    if let Some(f_number) = facts.f_number {
        push(tr!("Aperture"), format!("f/{}", trimmed_number(f_number)));
    }
    if let Some(exposure) = facts.exposure {
        push(tr!("Shutter speed"), exposure_text(exposure));
    }
    if let Some(iso) = facts.iso {
        push("ISO".to_owned(), iso.to_string());
    }
    if let Some((latitude, longitude)) = facts.location {
        push(tr!("Location"), location_text(latitude, longitude));
    }
    rows
}

/// 「4032 × 3024 · 12.2 MP」；不到一百万像素不写像素数。
fn dimensions_text(width: u32, height: u32) -> String {
    let megapixels = f64::from(width) * f64::from(height) / 1_000_000.0;
    if megapixels >= 1.0 {
        format!("{width} × {height} · {megapixels:.1} MP")
    } else {
        format!("{width} × {height}")
    }
}

/// 最多一位小数，去掉末尾的 0（f/2.8、50 mm、5.1 mm）。
fn trimmed_number(value: f32) -> String {
    let text = format!("{value:.1}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// 不到一秒写成「1/250 s」，一秒以上写小数「2.5 s」。
fn exposure_text((numerator, denominator): (u32, u32)) -> String {
    if numerator < denominator {
        let reciprocal = (f64::from(denominator) / f64::from(numerator)).round();
        format!("1/{reciprocal} s")
    } else {
        format!(
            "{} s",
            trimmed_number((f64::from(numerator) / f64::from(denominator)) as f32)
        )
    }
}

/// 「39.90000° N, 116.50000° W」：五位小数约一米。
fn location_text(latitude: f64, longitude: f64) -> String {
    let hemisphere = |value: f64, positive: &str, negative: &str| {
        let side = if value < 0.0 { negative } else { positive };
        format!("{:.5}° {side}", value.abs())
    };
    format!(
        "{}, {}",
        hemisphere(latitude, "N", "S"),
        hemisphere(longitude, "E", "W")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_read_like_a_camera_writes_them() {
        assert_eq!(exposure_text((10, 2500)), "1/250 s");
        assert_eq!(exposure_text((5, 2)), "2.5 s");
        assert_eq!(exposure_text((1, 1)), "1 s");
        assert_eq!(trimmed_number(2.8), "2.8");
        assert_eq!(trimmed_number(50.0), "50");
        assert_eq!(dimensions_text(4032, 3024), "4032 × 3024 · 12.2 MP");
        assert_eq!(dimensions_text(640, 480), "640 × 480");
        assert_eq!(location_text(39.9, -116.5), "39.90000° N, 116.50000° W");
    }
}
