use super::*;

/// 要下一帧：等一个刷新间隔；定了时间的不早于一个刷新间隔；不要就不画。
#[test]
fn a_skipped_frame_waits_a_refresh_interval_before_the_next() {
    let now = Instant::now();
    let interval = Duration::from_millis(6);

    assert_eq!(
        after_skipped_frame(RedrawRequest::NextFrame, now, interval),
        RedrawRequest::At(now + interval)
    );
    assert_eq!(
        after_skipped_frame(
            RedrawRequest::At(now + Duration::from_millis(1)),
            now,
            interval
        ),
        RedrawRequest::At(now + interval)
    );

    let later = now + Duration::from_millis(500);
    assert_eq!(
        after_skipped_frame(RedrawRequest::At(later), now, interval),
        RedrawRequest::At(later)
    );
    assert_eq!(
        after_skipped_frame(RedrawRequest::Wait, now, interval),
        RedrawRequest::Wait
    );
}

#[test]
fn the_refresh_interval_follows_the_monitor() {
    assert_eq!(frame_interval(Some(165_000)), Duration::from_secs(1) / 165);
    assert_eq!(frame_interval(None), Duration::from_secs(1) / 60);
    assert_eq!(frame_interval(Some(0)), Duration::from_secs(1) / 60);
}
