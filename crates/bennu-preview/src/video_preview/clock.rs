//! 视频播放的时钟：有声音就以声卡实际放到哪为准（画面跟着声音走），没有声音按墙钟。
//! 帧流订阅和引擎（进度条、时间）读同一个时钟；暂停时帧流停在等待上，ffmpeg 被管道背压挂起，
//! 继续时不用重开。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::Notify;

use crate::audio_preview::AudioPreviewRuntime;

/// 声卡回调一次取一块采样，报上来的位置是一格一格跳的；两次跳变之间按墙钟补，最多补这么多
/// （声音卡住时画面也跟着停，而不是一直往前走）。
const AUDIO_INTERPOLATION_LIMIT: Duration = Duration::from_millis(60);

pub struct PlaybackClock {
    state: Mutex<ClockState>,
    resumed: Notify,
}

struct ClockState {
    /// 上次定位（打开、跳转、暂停）时的位置。
    origin: Duration,
    /// 在走就是从什么时候开始走的；暂停时是 None。
    running_since: Option<Instant>,
    source: ClockSource,
}

enum ClockSource {
    Wall,
    /// 有音轨但声音还没起来：停在 `origin`，画面等声音。
    AwaitingAudio,
    Audio {
        runtime: AudioPreviewRuntime,
        /// 声卡上次报的位置和看到它变化的时刻，用来在两次跳变之间插值。
        reported: Duration,
        reported_at: Instant,
    },
}

impl std::fmt::Debug for PlaybackClock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PlaybackClock")
            .finish_non_exhaustive()
    }
}

impl PlaybackClock {
    /// 暂停着、在 0 处。有音轨的视频等声音起来再走。
    pub fn new(awaits_audio: bool) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(ClockState {
                origin: Duration::ZERO,
                running_since: None,
                source: if awaits_audio {
                    ClockSource::AwaitingAudio
                } else {
                    ClockSource::Wall
                },
            }),
            resumed: Notify::new(),
        })
    }

    pub fn position(&self) -> Duration {
        let mut state = self.lock();
        let running = state.running_since.is_some();
        let origin = state.origin;
        match &mut state.source {
            ClockSource::AwaitingAudio => origin,
            ClockSource::Wall => {
                origin
                    + state
                        .running_since
                        .map_or(Duration::ZERO, |since| since.elapsed())
            }
            ClockSource::Audio {
                runtime,
                reported,
                reported_at,
            } => {
                let now = runtime.position();
                if now != *reported {
                    *reported = now;
                    *reported_at = Instant::now();
                    return now;
                }
                if running {
                    now + reported_at.elapsed().min(AUDIO_INTERPOLATION_LIMIT)
                } else {
                    now
                }
            }
        }
    }

    pub fn is_running(&self) -> bool {
        self.lock().running_since.is_some()
    }

    pub fn pause(&self) {
        let position = self.position();
        let mut state = self.lock();
        state.origin = position;
        state.running_since = None;
    }

    pub fn resume(&self) {
        {
            let mut state = self.lock();
            if state.running_since.is_some() {
                return;
            }
            state.running_since = Some(Instant::now());
            if let ClockSource::Audio { reported_at, .. } = &mut state.source {
                *reported_at = Instant::now();
            }
        }
        self.resumed.notify_waiters();
    }

    /// 跳到 `position`，走着的接着走。有声音时声卡那边由调用方让播放器换流。
    pub fn seek(&self, position: Duration) {
        let mut state = self.lock();
        state.origin = position;
        if state.running_since.is_some() {
            state.running_since = Some(Instant::now());
        }
        if let ClockSource::Audio {
            reported,
            reported_at,
            ..
        } = &mut state.source
        {
            *reported = position;
            *reported_at = Instant::now();
        }
    }

    /// 声音起来了：之后以声卡为准。
    pub fn follow_audio(&self, runtime: AudioPreviewRuntime) {
        let mut state = self.lock();
        let reported = runtime.position();
        state.source = ClockSource::Audio {
            runtime,
            reported,
            reported_at: Instant::now(),
        };
    }

    /// 声音起不来（或放到一半坏了）：从当前位置起按墙钟走，画面不跟着卡住。
    pub fn follow_wall(&self) {
        let position = self.position();
        let mut state = self.lock();
        state.origin = position;
        if state.running_since.is_some() {
            state.running_since = Some(Instant::now());
        }
        state.source = ClockSource::Wall;
    }

    /// 在走就立刻返回，暂停时等到继续。
    pub async fn wait_until_running(&self) {
        loop {
            let notified = self.resumed.notified();
            tokio::pin!(notified);
            // 先登记再查状态：查完到等待之间来的 resume 不会漏掉。
            notified.as_mut().enable();
            if self.is_running() {
                return;
            }
            notified.await;
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ClockState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wall_clock_runs_pauses_and_seeks() {
        let clock = PlaybackClock::new(false);
        assert_eq!(clock.position(), Duration::ZERO);
        assert!(!clock.is_running());

        clock.seek(Duration::from_secs(5));
        assert_eq!(clock.position(), Duration::from_secs(5));
        clock.resume();
        std::thread::sleep(Duration::from_millis(30));
        let running = clock.position();
        assert!(running >= Duration::from_millis(5030), "{running:?}");

        clock.pause();
        let paused = clock.position();
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(clock.position(), paused);
    }

    #[test]
    fn video_with_sound_waits_for_it_and_falls_back_to_the_wall_clock() {
        let clock = PlaybackClock::new(true);
        clock.seek(Duration::from_secs(2));
        clock.resume();
        std::thread::sleep(Duration::from_millis(20));
        // 声音还没起来：停着等。
        assert_eq!(clock.position(), Duration::from_secs(2));

        clock.follow_wall();
        std::thread::sleep(Duration::from_millis(20));
        assert!(clock.position() > Duration::from_secs(2));
    }

    #[tokio::test]
    async fn waiting_for_resume_wakes_when_resumed() {
        let clock = PlaybackClock::new(false);
        let waiter = {
            let clock = Arc::clone(&clock);
            tokio::spawn(async move { clock.wait_until_running().await })
        };
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!waiter.is_finished());
        clock.resume();
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("woke up")
            .expect("task");
    }
}
