//! 会话缩略图请求的 main 层翻译：drain 出队 → iced Task::perform →
//! `Message::Session(ThumbnailReady)` 回信。滚动类消息绕过
//! SessionEffect 路径，因此缩略图请求的发起统一挂在
//! `apply_session_message` 收口处（滚动与 update 两条路径都覆盖），
//! 而不是新增 SessionEffect 变体。

use iced::{window, Task};

use crate::picker_session::thumbnails::ThumbnailLoadFailed;
use crate::picker_session::PickerSession;
use crate::{Message, SessionMessage};

/// 会话积攒的缩略图请求出队并发起加载（并发额度由会话层限流）：
/// 结果回信 ThumbnailReady；错误细节在此边界用 tracing 记一次，回信只
/// 携带轻量失败标记——ThumbnailError 不 Clone 而 iced 要求消息 Clone。
pub(crate) fn spawn_thumbnail_tasks(
    window_id: window::Id,
    session: &mut PickerSession,
) -> Vec<Task<Message>> {
    let cache_dir = session.thumbnail_cache_dir().to_path_buf();
    if cache_dir.as_os_str().is_empty() {
        // 无 HOME 等环境异常：dirs 解析不出缓存目录，直接不起任务，
        // 让行内图标兜底，而不是每张图都失败一遍再进 backoff。
        session.clear_pending_thumbnail_requests();
        return Vec::new();
    }
    session
        .drain_pending_thumbnail_requests()
        .into_iter()
        .map(|request| {
            let reply_request = request.clone();
            let load_cache_dir = cache_dir.clone();
            Task::perform(
                async move {
                    thumbnails::load_or_generate_thumbnail(load_cache_dir, request)
                        .await
                        .map_err(|error| {
                            tracing::debug!(%error, "文件选择器缩略图生成失败");
                            ThumbnailLoadFailed
                        })
                },
                move |outcome| {
                    Message::Session(
                        window_id,
                        SessionMessage::ThumbnailReady {
                            request: reply_request,
                            outcome,
                        },
                    )
                },
            )
        })
        .collect()
}
