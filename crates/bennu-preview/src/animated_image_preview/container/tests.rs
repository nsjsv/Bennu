use std::time::Duration;

use super::*;

/// 用 image 自带的编码器写一个两帧的 GIF；APNG、WebP 动图手写块（编码器写不了）。
fn write_two_frame_gif(path: &Path) {
    let file = File::create(path).expect("create gif");
    let mut encoder = image::codecs::gif::GifEncoder::new(file);
    let frame = |shade| {
        image::Frame::from_parts(
            image::RgbaImage::from_pixel(4, 4, image::Rgba([shade, 0, 0, 255])),
            0,
            0,
            image::Delay::from_numer_denom_ms(100, 1),
        )
    };
    encoder
        .encode_frames([frame(10), frame(200)])
        .expect("encode gif");
}

fn png_chunk_bytes(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut bytes = (data.len() as u32).to_be_bytes().to_vec();
    bytes.extend_from_slice(kind);
    bytes.extend_from_slice(data);
    bytes.extend([0; 4]);
    bytes
}

fn fake_apng(frames: u32, delay: (u16, u16)) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend(png_chunk_bytes(b"IHDR", &[0; 13]));
    let mut actl = frames.to_be_bytes().to_vec();
    actl.extend(0u32.to_be_bytes());
    bytes.extend(png_chunk_bytes(b"acTL", &actl));
    for sequence in 0..frames {
        let mut fctl = vec![0; 26];
        fctl[..4].copy_from_slice(&sequence.to_be_bytes());
        fctl[20..22].copy_from_slice(&delay.0.to_be_bytes());
        fctl[22..24].copy_from_slice(&delay.1.to_be_bytes());
        bytes.extend(png_chunk_bytes(b"fcTL", &fctl));
        let data_kind = if sequence == 0 { b"IDAT" } else { b"fdAT" };
        bytes.extend(png_chunk_bytes(data_kind, &[0; 4]));
    }
    bytes.extend(png_chunk_bytes(b"IEND", &[]));
    bytes
}

fn fake_animated_webp(durations_ms: &[u32]) -> Vec<u8> {
    let mut body = b"WEBP".to_vec();
    let mut vp8x = vec![0x02, 0, 0, 0];
    vp8x.extend([0; 6]);
    body.extend(b"VP8X");
    body.extend((vp8x.len() as u32).to_le_bytes());
    body.extend(&vp8x);
    for duration in durations_ms {
        let mut anmf = vec![0; 16];
        anmf[12..15].copy_from_slice(&duration.to_le_bytes()[..3]);
        body.extend(b"ANMF");
        body.extend((anmf.len() as u32).to_le_bytes());
        body.extend(&anmf);
    }
    let mut bytes = b"RIFF".to_vec();
    bytes.extend((body.len() as u32).to_le_bytes());
    bytes.extend(body);
    bytes
}

#[test]
fn animation_is_decided_by_content() {
    let directory = tempfile::tempdir().expect("temp dir");
    let gif = directory.path().join("moving.gif");
    write_two_frame_gif(&gif);
    assert!(is_animated_image(&gif));

    let still = directory.path().join("still.png");
    image::RgbaImage::from_pixel(2, 2, image::Rgba([0, 0, 0, 255]))
        .save(&still)
        .expect("save png");
    assert!(!is_animated_image(&still));

    let apng = directory.path().join("moving.png");
    std::fs::write(&apng, fake_apng(3, (1, 10))).expect("write apng");
    assert!(is_animated_image(&apng));
    let delays = chunk_frame_delays(&apng, AnimationContainer::Png)
        .expect("scan")
        .expect("delays");
    assert_eq!(delays, vec![Duration::from_millis(100); 3]);

    // 后缀不对也认内容。
    let webp = directory.path().join("moving.bin");
    std::fs::write(&webp, fake_animated_webp(&[40, 60])).expect("write webp");
    assert!(is_animated_image(&webp));
    let delays = chunk_frame_delays(&webp, AnimationContainer::WebP)
        .expect("scan")
        .expect("delays");
    assert_eq!(
        delays,
        vec![Duration::from_millis(40), Duration::from_millis(60)]
    );
}

#[test]
fn real_animated_frames_decode_for_every_container() {
    let directory = tempfile::tempdir().expect("temp dir");
    let gif = directory.path().join("moving.gif");
    write_two_frame_gif(&gif);
    let ((width, height), frames) = open_frames(&gif).expect("open gif");
    assert_eq!((width, height), (4, 4));
    assert_eq!(frames.count(), 2);
}

/// ffmpeg 写的真动图（testsrc 64×48、5 帧、每帧 0.2 秒）：解码器逐帧合成、块表时长都要对。
#[test]
fn real_webp_and_apng_animations_play() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/animation");
    for (name, container) in [
        ("five-frames.webp", AnimationContainer::WebP),
        ("five-frames.png", AnimationContainer::Png),
    ] {
        let path = fixtures.join(name);
        assert!(is_animated_image(&path), "{name}");
        let ((width, height), frames) = open_frames(&path).expect(name);
        assert_eq!((width, height), (64, 48), "{name}");
        let frames: Vec<_> = frames.collect::<Result<_, _>>().expect(name);
        assert_eq!(frames.len(), 5, "{name}");
        let delays = chunk_frame_delays(&path, container)
            .expect(name)
            .expect(name);
        assert_eq!(delays, vec![Duration::from_millis(200); 5], "{name}");
    }
}
