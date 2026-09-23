//! 预览命令层的宿主适配：命令本体已下沉 bennu-preview（返回
//! `Task<PreviewMessage>`），这里统一 `.map(Message::Preview)` 包装成
//! 宿主消息，维持既有函数名与签名、调用点零改动（状态机组迁移后由
//! PreviewEngine 直接消费 crate 命令，本层随之收拢）。

use file_core::{FileKind, ScanOptions};
use iced::Task;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

use crate::config::PreviewExtensionRules;
use crate::model::Message;

pub(crate) fn preview_command(
    path: PathBuf,
    kind: FileKind,
    rules: PreviewExtensionRules,
    options: ScanOptions,
    max_file_bytes: u64,
) -> Task<Message> {
    bennu_preview::commands::preview::preview_command(path, kind, rules, options, max_file_bytes)
        .map(Message::Preview)
}

pub(crate) fn right_preview_panel_info_command(path: PathBuf) -> Task<Message> {
    bennu_preview::commands::preview::right_preview_panel_info_command(path).map(Message::Preview)
}

pub(crate) fn image_preview_dimensions_command(path: PathBuf, generation: u64) -> Task<Message> {
    bennu_preview::commands::preview::image_preview_dimensions_command(path, generation)
        .map(Message::Preview)
}

pub(crate) fn original_image_preview_command(
    path: PathBuf,
    generation: u64,
    max_file_bytes: u64,
    placeholder_handle: Option<iced::widget::image::Handle>,
    cancellation: CancellationToken,
) -> Task<Message> {
    bennu_preview::commands::preview::original_image_preview_command(
        path,
        generation,
        max_file_bytes,
        placeholder_handle,
        cancellation,
    )
    .map(Message::Preview)
}

pub(crate) fn animated_image_preview_command(
    path: PathBuf,
    generation: u64,
    max_file_bytes: u64,
) -> Task<Message> {
    bennu_preview::commands::preview::animated_image_preview_command(
        path,
        generation,
        max_file_bytes,
    )
    .map(Message::Preview)
}

pub(crate) fn start_audio_preview_command(path: PathBuf) -> Task<Message> {
    bennu_preview::commands::preview::start_audio_preview_command(path).map(Message::Preview)
}
