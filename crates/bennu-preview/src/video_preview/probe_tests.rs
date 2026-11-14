use serde_json::json;

use super::*;

#[test]
fn reads_picture_tracks_and_container_from_one_probe() {
    let document = json!({
        "streams": [
            {"codec_type": "video", "codec_name": "mjpeg", "width": 600, "height": 600,
             "disposition": {"attached_pic": 1}},
            {"codec_type": "video", "codec_name": "hevc", "width": 3840, "height": 2160,
             "sample_aspect_ratio": "1:1", "avg_frame_rate": "24000/1001",
             "pix_fmt": "yuv420p10le", "color_space": "bt2020nc",
             "color_transfer": "smpte2084", "color_primaries": "bt2020",
             "field_order": "progressive", "bit_rate": "40000000",
             "disposition": {"attached_pic": 0, "default": 1}},
            {"codec_type": "audio", "codec_name": "truehd", "channels": 8,
             "sample_rate": "48000", "tags": {"language": "eng", "title": "Atmos"},
             "disposition": {"default": 1}},
            {"codec_type": "audio", "codec_name": "ac3", "channels": 6,
             "tags": {"language": "chi"}, "disposition": {"default": 0}},
            {"codec_type": "subtitle", "codec_name": "hdmv_pgs_subtitle",
             "tags": {"language": "chi"}, "disposition": {"default": 0}},
            {"codec_type": "subtitle", "codec_name": "subrip",
             "tags": {"language": "und"}, "disposition": {"default": 1}}
        ],
        "format": {"format_name": "matroska,webm", "format_long_name": "Matroska / WebM",
                   "duration": "5400.5", "bit_rate": "45000000"}
    });

    let probe = video_probe_from_document(&document);

    assert_eq!(probe.duration, Some(Duration::from_secs_f64(5400.5)));
    assert_eq!(probe.container.as_deref(), Some("Matroska / WebM"));
    assert_eq!(probe.bit_rate, Some(45_000_000));
    let video = probe.video.expect("picture stream");
    // 封面图也算在视频流序号里。
    assert_eq!(video.ordinal, 1);
    assert_eq!((video.width, video.height), (3840, 2160));
    assert_eq!(video.frame_rate, Some((24000, 1001)));
    assert!(video.color.is_hdr());
    assert_eq!(video.color.matrix, YuvMatrix::Bt2020);
    assert_eq!(video.color.primaries, VideoPrimaries::Bt2020);
    assert!(!video.interlaced);
    assert_eq!(probe.audio_tracks.len(), 2);
    assert_eq!(probe.audio_tracks[1].ordinal, 1);
    assert_eq!(probe.audio_tracks[1].language.as_deref(), Some("chi"));
    assert!(probe.audio_tracks[0].is_default);
    assert!(probe.subtitle_tracks[0].is_image);
    assert!(!probe.subtitle_tracks[1].is_image);
    // und 当作没写语言。
    assert_eq!(probe.subtitle_tracks[1].language, None);
}

#[test]
fn cover_art_only_means_no_picture() {
    let document = json!({
        "streams": [
            {"codec_type": "audio", "codec_name": "aac"},
            {"codec_type": "video", "codec_name": "png", "width": 500, "height": 500,
             "disposition": {"attached_pic": 1}}
        ],
        "format": {"duration": "200.0"}
    });
    assert_eq!(video_probe_from_document(&document).video, None);
}

#[test]
fn rotation_interlacing_and_untagged_color_are_read() {
    let stream = |extra: serde_json::Value| {
        let mut stream = json!({"codec_type": "video", "codec_name": "h264",
                                "width": 720, "height": 576});
        for (key, value) in extra.as_object().expect("object") {
            stream[key] = value.clone();
        }
        video_probe_from_document(&json!({"streams": [stream], "format": {}}))
            .video
            .expect("picture stream")
    };

    // 显示矩阵 -90 = 顺时针转 90 度；旧的 rotate 标签直接是顺时针角度。
    assert_eq!(
        stream(json!({"side_data_list": [{"rotation": -90}]})).rotation,
        90
    );
    assert_eq!(
        stream(json!({"side_data_list": [{"rotation": 90}]})).rotation,
        270
    );
    assert_eq!(stream(json!({"tags": {"rotate": "180"}})).rotation, 180);
    assert_eq!(stream(json!({})).rotation, 0);

    assert!(stream(json!({"field_order": "tt"})).interlaced);
    assert!(!stream(json!({"field_order": "unknown"})).interlaced);

    // 没标颜色：标清按 BT.601，SDR，BT.709 原色。
    let untagged = stream(json!({"sample_aspect_ratio": "16:15"}));
    assert_eq!(untagged.color.matrix, YuvMatrix::Bt601);
    assert!(!untagged.color.is_hdr());
    assert_eq!(untagged.sample_aspect, (16, 15));
    assert_eq!(untagged.frame_rate, None);
    let hlg = stream(json!({"color_transfer": "arib-std-b67", "height": 2160}));
    assert_eq!(hlg.color.transfer, VideoTransfer::Hlg);
}
