//! UI 节拍常量：跨模块共享的帧步进与进度刷新周期，避免各处硬编码漂移。

use std::time::Duration;

/// 60Hz 单帧步进：动画帧间隔与高频 UI 提示节流统一引用此值。
pub(crate) const FRAME_INTERVAL_60HZ: Duration = Duration::from_millis(16);

/// 进度类消息的 UI 刷新周期：进度条无需 60Hz，100ms 已足够平滑。
pub(crate) const PROGRESS_UI_INTERVAL: Duration = Duration::from_millis(100);
