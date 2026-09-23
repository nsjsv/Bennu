//! UI 节拍常量：跨模块共享的帧步进与进度刷新周期，避免各处硬编码漂移。
//! 自 app-ui 下沉（预览命令层引用 PROGRESS_UI_INTERVAL；app-ui 经
//! ui_pacing.rs re-export 维持 crate::ui_pacing::* 路径，非预览消费者
//! 不变）。

use std::time::Duration;

/// 60Hz 单帧步进：动画帧间隔与高频 UI 提示节流统一引用此值。
pub const FRAME_INTERVAL_60HZ: Duration = Duration::from_millis(16);

/// 进度类消息的 UI 刷新周期：进度条无需 60Hz，100ms 已足够平滑。
pub const PROGRESS_UI_INTERVAL: Duration = Duration::from_millis(100);
