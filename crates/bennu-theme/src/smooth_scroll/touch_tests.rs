use std::time::{Duration, Instant};

use iced::{window, Vector};

use super::{
    split_at_edges, Orientation, ScrollTarget, TouchScroll, DECELERATION_PER_SECOND, RELEASE_AFTER,
};

/// 一个竖向滚动区：视口 500，内容能滚到 2000；横向不能滚。
struct Scroller {
    offset: f32,
    max: f32,
}

impl Scroller {
    fn new(offset: f32) -> Self {
        Self {
            offset,
            max: 2000.0,
        }
    }
}

impl ScrollTarget for Scroller {
    fn scroll(&mut self, orientation: Orientation, delta: f32) -> (f32, f32) {
        if orientation == Orientation::Horizontal {
            return (0.0, 0.0);
        }
        let (scrolled, beyond) = split_at_edges(self.offset, self.max, delta);
        self.offset += scrolled;
        (scrolled, beyond)
    }

    fn extent(&self, orientation: Orientation) -> f32 {
        match orientation {
            Orientation::Horizontal => 300.0,
            Orientation::Vertical => 500.0,
        }
    }
}

fn down(y: f32) -> Vector {
    Vector::new(0.0, y)
}

/// 手指每 8 ms 移动 `step`，共 `count` 次，返回最后一次的时刻。
fn swipe(
    touch: &mut TouchScroll,
    scroller: &mut Scroller,
    start: Instant,
    step: f32,
    count: u32,
) -> Instant {
    let mut at = start;
    for index in 0..count {
        at = start + Duration::from_millis(8 * u64::from(index));
        touch.follow(down(step), at, scroller);
    }
    at
}

/// 按固定帧间隔推进到停下，返回每帧画出来的越界。
fn run(
    touch: &mut TouchScroll,
    scroller: &mut Scroller,
    from: Instant,
    frame: Duration,
) -> Vec<f32> {
    let mut displacements = Vec::new();
    let mut at = from;
    for _ in 0..2000 {
        if touch.next_redraw().is_none() {
            return displacements;
        }
        at += frame;
        touch.advance(at, scroller);
        displacements.push(touch.displacement().y);
    }
    panic!("the motion never came to rest");
}

#[test]
fn content_follows_the_fingers_one_to_one() {
    let mut touch = TouchScroll::default();
    let mut scroller = Scroller::new(100.0);
    swipe(&mut touch, &mut scroller, Instant::now(), 3.5, 10);
    assert_eq!(scroller.offset, 135.0);
    assert_eq!(touch.displacement(), Vector::ZERO);
}

/// 松手后按手指速度滑出去：总距离是 v/λ（每毫秒乘 0.998），和帧率无关。
#[test]
fn a_flick_coasts_the_same_distance_at_any_frame_rate() {
    let coast = |frame: Duration| {
        let start = Instant::now();
        let mut touch = TouchScroll::default();
        let mut scroller = Scroller::new(0.0);
        // 每 8 ms 4 像素 = 500 像素/秒。
        let last = swipe(&mut touch, &mut scroller, start, 4.0, 20);
        let before = scroller.offset;
        touch.release(last);
        run(&mut touch, &mut scroller, last, frame);
        scroller.offset - before
    };
    let expected = 500.0 / DECELERATION_PER_SECOND;
    for frame in [6_061, 16_667, 50_000].map(Duration::from_micros) {
        let coasted = coast(frame);
        assert!((coasted - expected).abs() < 6.0, "{coasted} vs {expected}");
    }
}

/// 手指停住再松手：最后 0.1 秒没动过，不滑。
#[test]
fn lifting_after_holding_still_does_not_coast() {
    let start = Instant::now();
    let mut touch = TouchScroll::default();
    let mut scroller = Scroller::new(0.0);
    let last = swipe(&mut touch, &mut scroller, start, 10.0, 10);
    touch.release(last + Duration::from_millis(150));
    assert_eq!(touch.next_redraw(), None);
}

/// 在顶上往上拉：内容不动，越界越拉越紧、不超过视口；往回推越过边缘后剩下的滚内容。
#[test]
fn pulling_past_the_top_stretches_less_and_less() {
    let start = Instant::now();
    let mut touch = TouchScroll::default();
    let mut scroller = Scroller::new(0.0);
    let mut previous = 0.0;
    let mut previous_gain = f32::INFINITY;
    for index in 1..=10 {
        let at = start + Duration::from_millis(8 * index);
        touch.follow(down(-100.0), at, &mut scroller);
        let shown = -touch.displacement().y;
        assert!(shown > previous && shown - previous <= previous_gain);
        previous_gain = shown - previous;
        previous = shown;
    }
    assert_eq!(scroller.offset, 0.0);
    assert!(previous < 500.0);

    // 拉了 1000，推回 1050：多出来的 50 滚内容。
    let at = start + Duration::from_millis(100);
    touch.follow(down(1050.0), at, &mut scroller);
    assert_eq!(touch.displacement(), Vector::ZERO);
    assert_eq!(scroller.offset, 50.0);
}

/// 拉着松手：弹回原位，一路往回、不越过，内容不动，一秒内停。
#[test]
fn releasing_a_stretch_springs_back() {
    let start = Instant::now();
    let mut touch = TouchScroll::default();
    let mut scroller = Scroller::new(0.0);
    let last = swipe(&mut touch, &mut scroller, start, -30.0, 5);
    let stretched = touch.displacement().y;
    assert!(stretched < -50.0, "{stretched}");

    touch.release(last);
    let shown = run(
        &mut touch,
        &mut scroller,
        last,
        Duration::from_micros(16_667),
    );
    assert!(shown.len() < 60, "took {} frames", shown.len());
    assert!(shown
        .windows(2)
        .all(|pair| pair[1] >= pair[0] && pair[1] <= 0.0));
    assert_eq!(touch.displacement(), Vector::ZERO);
    assert_eq!(scroller.offset, 0.0);
}

/// 惯性滑到底：冲出边缘一点再弹回来，停在底上。
#[test]
fn coasting_into_the_end_bounces_back() {
    let start = Instant::now();
    let mut touch = TouchScroll::default();
    let mut scroller = Scroller::new(1800.0);
    // 每 8 ms 16 像素 = 2000 像素/秒，能滑 1000，只剩 200 就到底。
    let last = swipe(&mut touch, &mut scroller, start, 16.0, 6);
    touch.release(last);
    let shown = run(
        &mut touch,
        &mut scroller,
        last,
        Duration::from_micros(16_667),
    );
    let peak = shown.iter().copied().fold(0.0, f32::max);
    assert!(peak > 10.0 && peak < 200.0, "{peak}");
    assert_eq!(shown.last(), Some(&0.0));
    assert_eq!(scroller.offset, 2000.0);
}

/// 内容放得下（这根轴不能滚）：不越界也不滑。
#[test]
fn an_axis_that_cannot_scroll_does_not_stretch() {
    let start = Instant::now();
    let mut touch = TouchScroll::default();
    let mut scroller = Scroller::new(0.0);
    scroller.max = 0.0;
    let last = swipe(&mut touch, &mut scroller, start, 20.0, 10);
    assert_eq!(touch.displacement(), Vector::ZERO);
    touch.release(last);
    touch.advance(last + Duration::from_millis(16), &mut scroller);
    assert_eq!(scroller.offset, 0.0);
    assert_eq!(touch.next_redraw(), None);
}

/// 没有结束信号的像素滚动拉过边缘后不再有输入：到点自己弹回，不会一直拉着。
#[test]
fn a_stretch_without_an_end_signal_springs_back_on_its_own() {
    let start = Instant::now();
    let mut touch = TouchScroll::default();
    let mut scroller = Scroller::new(0.0);
    let last = swipe(&mut touch, &mut scroller, start, -20.0, 3);
    assert_eq!(
        touch.next_redraw(),
        Some(window::RedrawRequest::At(last + RELEASE_AFTER))
    );
    touch.advance(last + RELEASE_AFTER, &mut scroller);
    assert!(!touch.is_held());
    run(
        &mut touch,
        &mut scroller,
        last + RELEASE_AFTER,
        Duration::from_millis(16),
    );
    assert_eq!(touch.displacement(), Vector::ZERO);
}

/// 弹回途中手指又放上来：越界原地被接住，不跳。
#[test]
fn grabbing_a_settling_stretch_keeps_it_in_place() {
    let start = Instant::now();
    let mut touch = TouchScroll::default();
    let mut scroller = Scroller::new(0.0);
    let last = swipe(&mut touch, &mut scroller, start, -40.0, 5);
    touch.release(last);
    let at = last + Duration::from_millis(50);
    touch.advance(at, &mut scroller);
    let settling = touch.displacement().y;
    assert!(settling < 0.0);

    touch.follow(down(-0.001), at, &mut scroller);
    assert_eq!(scroller.offset, 0.0);
    assert!((touch.displacement().y - settling).abs() <= 1.0);
}

/// 滑行的尾巴每帧挪不到一像素：只在攒够下一个像素时才约下一帧。
#[test]
fn a_slow_coast_waits_for_the_next_whole_pixel() {
    let start = Instant::now();
    let mut touch = TouchScroll::default();
    let mut scroller = Scroller::new(0.0);
    // 每 8 ms 0.2 像素 = 25 像素/秒。
    let last = swipe(&mut touch, &mut scroller, start, 0.2, 20);
    touch.release(last);
    let Some(window::RedrawRequest::At(at)) = touch.next_redraw() else {
        panic!("expected a timed redraw, got {:?}", touch.next_redraw());
    };
    assert!(at - last > Duration::from_millis(30), "{:?}", at - last);
}
