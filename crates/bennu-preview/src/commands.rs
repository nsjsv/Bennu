//! 预览命令层：返回 `Task<PreviewMessage>` 的自由函数（异步加载经
//! `Task::perform`/`Task::stream` 包装），自 app-ui 的 commands/preview.rs
//! 下沉。宿主调用侧经 `.map(Message::Preview)` 包装进各自顶层 Message。

pub mod preview;
// 文档预览命令层（Poppler/LibreOffice 外部进程管线），返回
// Task<PreviewMessage>；自 app-ui 的 commands/document_preview 下沉。
pub mod document_preview;
