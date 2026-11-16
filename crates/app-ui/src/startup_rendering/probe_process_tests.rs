use super::*;

fn frame(pixel: [u8; 4], size: Size<u32>) -> Vec<u8> {
    pixel.repeat(size.width as usize * size.height as usize)
}

/// 整帧都是底色才算画出来了；差一个像素、全黑、尺寸对不上、空帧都不算。
#[test]
fn only_a_frame_of_the_background_color_counts() {
    let size = Size::new(4, 3);
    let background = PROBE_BACKGROUND.into_rgba8();
    assert!(renders_background(&frame(background, size), size));

    let rounded = background.map(|channel| channel.saturating_add(PIXEL_TOLERANCE));
    assert!(renders_background(&frame(rounded, size), size));

    let mut one_wrong = frame(background, size);
    one_wrong[5 * 4 + 1] = 0;
    assert!(!renders_background(&one_wrong, size));

    assert!(!renders_background(&frame([0, 0, 0, 0], size), size));
    assert!(!renders_background(
        &frame(background, size),
        Size::new(4, 4)
    ));
    assert!(!renders_background(&[], Size::new(0, 0)));
}
