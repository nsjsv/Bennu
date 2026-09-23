//! UI 节拍常量：跨模块共享的帧步进与进度刷新周期，避免各处硬编码漂移。
//! 常量本体已下沉 bennu-preview（预览命令层引用 PROGRESS_UI_INTERVAL）；
//! re-export 维持 crate::ui_pacing::* 既有调用路径（scrollbar、目录加载、
//! 传输进度等非预览消费者不变）。

pub(crate) use bennu_preview::ui_pacing::{FRAME_INTERVAL_60HZ, PROGRESS_UI_INTERVAL};
