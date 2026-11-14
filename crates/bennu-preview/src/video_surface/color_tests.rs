use super::*;

fn close(actual: f32, expected: f32, tolerance: f32) -> bool {
    (actual - expected).abs() <= tolerance
}

fn sdr(matrix: YuvMatrix) -> ColorParams {
    ColorParams::new(
        VideoColor {
            matrix,
            transfer: VideoTransfer::Sdr,
            primaries: VideoPrimaries::Bt709,
        },
        SampleDepth::Eight,
    )
}

#[test]
fn limited_range_bt709_maps_reference_colors() {
    let params = sdr(YuvMatrix::Bt709);
    assert_eq!(params.display_rgb(235.0, 128.0, 128.0), [1.0, 1.0, 1.0]);
    assert_eq!(params.display_rgb(16.0, 128.0, 128.0), [0.0, 0.0, 0.0]);
    // BT.709 有限范围的纯红：Y 63、Cb 102、Cr 240。
    let red = params.display_rgb(63.0, 102.0, 240.0);
    assert!(close(red[0], 1.0, 0.01), "{red:?}");
    assert!(close(red[1], 0.0, 0.01), "{red:?}");
    assert!(close(red[2], 0.0, 0.01), "{red:?}");
}

#[test]
fn bt601_uses_its_own_coefficients() {
    // BT.601 有限范围的纯绿：Y 145、Cb 54、Cr 34。
    let green = sdr(YuvMatrix::Bt601).display_rgb(145.0, 54.0, 34.0);
    assert!(close(green[0], 0.0, 0.01), "{green:?}");
    assert!(close(green[1], 1.0, 0.01), "{green:?}");
    assert!(close(green[2], 0.0, 0.01), "{green:?}");
}

#[test]
fn ten_bit_samples_in_sixteen_bit_containers_match_eight_bit_codes() {
    let params = ColorParams::new(
        VideoColor {
            matrix: YuvMatrix::Bt709,
            transfer: VideoTransfer::Sdr,
            primaries: VideoPrimaries::Bt709,
        },
        SampleDepth::Sixteen,
    );
    // 10 位白 940、色度中点 512，p010 左移 6 位。
    let white = params.display_rgb(940.0 * 64.0, 512.0 * 64.0, 512.0 * 64.0);
    assert!(
        white.iter().all(|channel| close(*channel, 1.0, 0.001)),
        "{white:?}"
    );
}

#[test]
fn pq_curve_round_trips_known_points() {
    assert!(close(pq_to_nits(1.0), 10000.0, 1.0));
    assert!(close(pq_to_nits(0.0), 0.0, 0.001));
    // 100 尼特在 PQ 里约 0.5081，203 尼特约 0.5807。
    assert!(close(nits_to_pq(100.0), 0.5081, 0.001));
    assert!(close(nits_to_pq(203.0), 0.5807, 0.001));
    assert!(close(pq_to_nits(nits_to_pq(203.0)), 203.0, 0.1));
}

#[test]
fn highlight_compression_keeps_shadows_and_caps_the_peak() {
    // 膝点以下原样。
    assert!(close(compressed_nits(10.0), 10.0, 0.05));
    assert!(close(compressed_nits(50.0), 50.0, 0.2));
    // 母带峰值压到普通视频的白，更亮的也不超过。
    assert!(close(
        compressed_nits(HDR_SOURCE_PEAK_NITS),
        SDR_WHITE_NITS,
        0.5
    ));
    assert!(compressed_nits(4000.0) <= SDR_WHITE_NITS + 0.5);
    // 单调。
    assert!(compressed_nits(300.0) < compressed_nits(600.0));
}

#[test]
fn hdr_white_lands_on_display_white_without_clipping_midtones() {
    let pq = ColorParams::new(
        VideoColor {
            matrix: YuvMatrix::Bt2020,
            transfer: VideoTransfer::Pq,
            primaries: VideoPrimaries::Bt2020,
        },
        SampleDepth::Sixteen,
    );
    // 1000 尼特的白（PQ 约 0.7518）→ 显示白。
    let code = |encoded: f32| (16.0 + 219.0 * encoded) * 256.0;
    let peak = pq.display_rgb(code(nits_to_pq(1000.0)), 128.0 * 256.0, 128.0 * 256.0);
    assert!(
        peak.iter().all(|channel| close(*channel, 1.0, 0.01)),
        "{peak:?}"
    );
    // 20 尼特的灰保持暗：(20/203)^(1/2.4) ≈ 0.38。
    let shadow = pq.display_rgb(code(nits_to_pq(20.0)), 128.0 * 256.0, 128.0 * 256.0);
    assert!(
        close(shadow[1], (20.0_f32 / 203.0).powf(1.0 / 2.4), 0.01),
        "{shadow:?}"
    );
}

#[test]
fn hlg_scene_light_follows_the_inverse_oetf() {
    assert!(close(hlg_to_scene(0.5), 1.0 / 12.0, 0.0001));
    assert!(close(hlg_to_scene(1.0), 1.0, 0.001));
}
