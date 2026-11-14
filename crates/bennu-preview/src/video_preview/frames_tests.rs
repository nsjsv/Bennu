use super::*;
use crate::video_preview::test_fixtures::video_fixture;
use crate::video_preview::{VideoColor, VideoPrimaries, VideoTransfer, YuvMatrix};

const SDR: VideoColor = VideoColor {
    matrix: YuvMatrix::Bt709,
    transfer: VideoTransfer::Sdr,
    primaries: VideoPrimaries::Bt709,
};

fn facts(width: u32, height: u32) -> VideoStreamFacts {
    VideoStreamFacts {
        ordinal: 0,
        codec: "h264".to_owned(),
        width,
        height,
        sample_aspect: (1, 1),
        frame_rate: Some((30, 1)),
        rotation: 0,
        pixel_format: None,
        color: SDR,
        interlaced: false,
        bit_rate: None,
    }
}

const FULL_HD_SCREEN: Size<u32> = Size::new(1920, 1080);

#[test]
fn picture_is_capped_at_the_screen_and_never_enlarged() {
    let uhd = FramePlan::new(&facts(3840, 2160), FULL_HD_SCREEN);
    assert_eq!((uhd.width, uhd.height), (1920, 1080));
    let small = FramePlan::new(&facts(640, 360), FULL_HD_SCREEN);
    assert_eq!((small.width, small.height), (640, 360));
    // 竖拍：先转正再按屏幕高度封顶。
    let portrait = FramePlan::new(
        &VideoStreamFacts {
            rotation: 90,
            ..facts(1920, 1080)
        },
        FULL_HD_SCREEN,
    );
    assert_eq!((portrait.width, portrait.height), (608, 1080));
    // DVD 的非方形像素拉成方形；奇数取偶。
    let dvd = FramePlan::new(
        &VideoStreamFacts {
            sample_aspect: (32, 27),
            ..facts(720, 480)
        },
        FULL_HD_SCREEN,
    );
    assert_eq!((dvd.width, dvd.height), (854, 480));
}

#[test]
fn frame_rate_is_capped_at_sixty_and_drives_frame_times() {
    let fast = FramePlan::new(
        &VideoStreamFacts {
            frame_rate: Some((120, 1)),
            ..facts(1280, 720)
        },
        FULL_HD_SCREEN,
    );
    assert_eq!(fast.frame_rate, (60, 1));
    assert_eq!(fast.frame_offset(60), Duration::from_secs(1));
    let film = FramePlan::new(
        &VideoStreamFacts {
            frame_rate: Some((24000, 1001)),
            ..facts(1280, 720)
        },
        FULL_HD_SCREEN,
    );
    assert_eq!(film.frame_rate, (24000, 1001));
    let unknown = FramePlan::new(
        &VideoStreamFacts {
            frame_rate: None,
            ..facts(1280, 720)
        },
        FULL_HD_SCREEN,
    );
    assert_eq!(unknown.frame_rate, (30, 1));
}

#[test]
fn filters_deinterlace_only_marked_frames_and_keep_hdr_in_ten_bits() {
    let interlaced = FramePlan::new(
        &VideoStreamFacts {
            interlaced: true,
            ..facts(720, 576)
        },
        FULL_HD_SCREEN,
    );
    assert_eq!(
        interlaced.stream_filter(None),
        "bwdif=deint=interlaced,fps=30/1,scale=720:576:out_range=tv:flags=area,format=nv12"
    );
    assert_eq!(
        interlaced.still_filter(),
        "bwdif=deint=interlaced,scale=720:576:out_range=tv:flags=area,format=nv12"
    );
    let hdr = FramePlan::new(
        &VideoStreamFacts {
            color: VideoColor {
                matrix: YuvMatrix::Bt2020,
                transfer: VideoTransfer::Pq,
                primaries: VideoPrimaries::Bt2020,
            },
            ..facts(3840, 2160)
        },
        FULL_HD_SCREEN,
    );
    assert!(hdr.stream_filter(None).ends_with("format=p010le"));
    assert_eq!(hdr.frame_bytes(), 1920 * 1080 * 3 / 2 * 2);
}

#[tokio::test]
async fn ffmpeg_streams_whole_frames_and_seeks_to_stills() {
    let directory = tempfile::tempdir().expect("temp dir");
    let Some(path) = video_fixture(directory.path(), "clip.mkv", "320x240", 2, &[]) else {
        return;
    };
    let plan = FramePlan::new(&facts(320, 240), FULL_HD_SCREEN);

    let mut child = stream_command(&path, &plan, Duration::from_secs(1), None, None)
        .spawn()
        .expect("spawn ffmpeg");
    let mut stdout = child.stdout.take().expect("stdout");
    let mut frames = 0;
    while let Some(picture) = read_picture(&mut stdout, &plan).await.expect("read") {
        assert_eq!((picture.width(), picture.height()), (320, 240));
        frames += 1;
    }
    // 从第 1 秒起到 2 秒结束，30 帧每秒。
    assert!((29..=31).contains(&frames), "{frames}");

    for precision in [SeekPrecision::Keyframe, SeekPrecision::Exact] {
        let mut child = still_command(&path, &plan, Duration::from_millis(1500), precision)
            .spawn()
            .expect("spawn ffmpeg");
        let mut stdout = child.stdout.take().expect("stdout");
        assert!(read_picture(&mut stdout, &plan)
            .await
            .expect("read")
            .is_some());
        assert!(read_picture(&mut stdout, &plan)
            .await
            .expect("read")
            .is_none());
    }
}

#[tokio::test]
async fn exact_still_is_the_same_frame_continuous_decoding_shows_there() {
    let directory = tempfile::tempdir().expect("temp dir");
    let Some(path) = video_fixture(directory.path(), "clip.mkv", "320x240", 2, &[]) else {
        return;
    };
    let plan = FramePlan::new(&facts(320, 240), FULL_HD_SCREEN);
    let target = Duration::from_millis(1500);

    let mut child = stream_command(&path, &plan, Duration::ZERO, None, None)
        .spawn()
        .expect("spawn ffmpeg");
    let mut stdout = child.stdout.take().expect("stdout");
    let mut continuous = None;
    for index in 0..=45 {
        let picture = read_picture(&mut stdout, &plan).await.expect("read");
        if plan.frame_offset(index) == target {
            continuous = picture;
            break;
        }
    }
    let continuous = continuous.expect("frame at 1.5 s");

    let mut child = still_command(&path, &plan, target, SeekPrecision::Exact)
        .spawn()
        .expect("spawn ffmpeg");
    let mut stdout = child.stdout.take().expect("stdout");
    let still = read_picture(&mut stdout, &plan)
        .await
        .expect("read")
        .expect("still");
    assert!(still.same_pixels(&continuous));
}
