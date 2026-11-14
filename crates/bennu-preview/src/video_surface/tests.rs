use super::*;
use crate::video_preview::{VideoPrimaries, VideoTransfer, YuvMatrix};

const BT709_SDR: VideoColor = VideoColor {
    matrix: YuvMatrix::Bt709,
    transfer: VideoTransfer::Sdr,
    primaries: VideoPrimaries::Bt709,
};

#[test]
fn shader_parses_and_validates() {
    let module = naga::front::wgsl::parse_str(include_str!("video.wgsl"))
        .unwrap_or_else(|error| panic!("{}", error.emit_to_string(include_str!("video.wgsl"))));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap_or_else(|error| panic!("{error:?}"));
}

#[test]
fn uniforms_match_the_shader_layout_and_count_minification_taps() {
    let picture = VideoPicture::new(
        1920,
        1080,
        SampleDepth::Eight,
        BT709_SDR,
        vec![0; picture_bytes(1920, 1080, SampleDepth::Eight)],
    );
    let floats = |bytes: Vec<u8>| -> Vec<f32> {
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|chunk| f32::from_ne_bytes(*chunk))
            .collect()
    };

    // 原大显示：一点；放进 400 宽的面板：约 4.8 倍缩小，取到上限 4 点。
    let full = floats(uniform_bytes(&picture, Size::new(1920.0, 1080.0), false));
    assert_eq!(full.len() * 4, UNIFORM_BYTES);
    assert_eq!(&full[16..20], &[1920.0, 1080.0, 1.0, 1.0]);
    assert_eq!(full[15], 0.0);
    let panel = floats(uniform_bytes(&picture, Size::new(400.0, 225.0), true));
    assert_eq!(&panel[18..20], &[4.0, 4.0]);
    assert_eq!(panel[20], 4.8);
    // sRGB 目标要着色器输出线性值。
    assert_eq!(panel[15], 1.0);
}

#[test]
fn software_fallback_converts_nv12_and_p010() {
    // 2×2 的白：亮度 235，色度中点 128。
    let mut nv12 = vec![235; 4];
    nv12.extend([128, 128]);
    let white = VideoPicture::new(2, 2, SampleDepth::Eight, BT709_SDR, nv12);
    assert_eq!(software_rgba(&white), [255; 16]);

    // p010：10 位黑 64、色度中点 512，左移 6 位、小端。
    let sample = |value: u16| (value << 6).to_le_bytes();
    let mut p010 = Vec::new();
    for _ in 0..4 {
        p010.extend(sample(64));
    }
    p010.extend(sample(512));
    p010.extend(sample(512));
    let black = VideoPicture::new(2, 2, SampleDepth::Sixteen, BT709_SDR, p010);
    assert_eq!(
        software_rgba(&black),
        [0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255]
    );
}
