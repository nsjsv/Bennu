use std::time::Instant;

use iced::futures::StreamExt;

use super::*;
use crate::video_preview::test_fixtures::video_fixture;

#[tokio::test]
async fn paused_stream_shows_one_frame_then_waits_and_resumes_without_restarting() {
    let directory = tempfile::tempdir().expect("temp dir");
    let Some(path) = video_fixture(directory.path(), "clip.mkv", "160x120", 1, &[]) else {
        return;
    };
    let probe = probe_video_preview(path.clone()).await.expect("probe");
    let video = probe.video.expect("picture");
    let plan = Arc::new(FramePlan::new(&video, iced::Size::new(1920, 1080)));
    let clock = PlaybackClock::new(false);
    let segment = VideoSegment {
        path,
        plan,
        clock: Arc::clone(&clock),
        generation: 7,
        start: Duration::ZERO,
    };
    let (mut sender, mut frames) = iced::futures::channel::mpsc::channel(4);
    let stream = tokio::spawn(async move { stream_frames(&segment, &mut sender).await });

    // 暂停着也先送第一帧（跳转后停在那一帧要看得见）。
    let first = tokio::time::timeout(Duration::from_secs(5), frames.next())
        .await
        .expect("first frame")
        .expect("message");
    assert!(matches!(
        first,
        PreviewMessage::VideoPreviewFrameLoaded(VideoPreviewFrame { generation: 7, .. })
    ));
    // 暂停中不再出帧。
    assert!(
        tokio::time::timeout(Duration::from_millis(300), frames.next())
            .await
            .is_err()
    );

    let resumed_at = Instant::now();
    clock.resume();
    let mut count = 1;
    while let Some(message) = frames.next().await {
        assert!(matches!(
            message,
            PreviewMessage::VideoPreviewFrameLoaded(_)
        ));
        count += 1;
    }
    stream.await.expect("task").expect("stream");
    // 1 秒 30 帧，按时钟一帧帧放出来，不是一下子倒完。
    assert!((28..=31).contains(&count), "{count}");
    assert!(resumed_at.elapsed() >= Duration::from_millis(800));
}
