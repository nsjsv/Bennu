//! 文件夹预览的消息、订阅（目录监视、测量节拍）和键盘移动后的滚动。模型在
//! `folder_preview`；打开某项（双击、回车）以 `FolderEntryOpened` 交宿主。

use std::path::PathBuf;
use std::time::Duration;

use iced::futures::{SinkExt, Stream};
use iced::widget::scrollable;
use iced::{Subscription, Task};

use crate::folder_preview::FolderPreviewMessage;
use crate::preview::{PreviewContent, PreviewState, TreeViewport};
use crate::preview_message::PreviewMessage;
use crate::preview_tree_panel::TREE_ROW_HEIGHT;

/// 与主列表的目录监视同一个合并间隔。
const FOLDER_WATCH_DEBOUNCE: Duration = Duration::from_millis(250);
/// 测量在跑时多久读一次数：数字在涨，但不每帧重排。
const MEASUREMENT_TICK: Duration = Duration::from_millis(150);

impl super::PreviewEngine {
    pub(crate) fn handle_folder_preview_message(
        &mut self,
        message: FolderPreviewMessage,
    ) -> Task<PreviewMessage> {
        let Some(PreviewState::Ready(PreviewContent::Folder(folder))) = &mut self.preview else {
            return Task::none();
        };
        match message {
            FolderPreviewMessage::Toggled(path) => folder.toggle(&path),
            FolderPreviewMessage::Selected(path) => {
                folder.select(path);
                Task::none()
            }
            FolderPreviewMessage::Key(key, scroll_region_id) => {
                let outcome = folder.press_key(key);
                let reveal = reveal_row(folder.viewport, outcome.selected_row, scroll_region_id);
                let open = match outcome.open {
                    Some((path, kind)) => {
                        Task::done(PreviewMessage::FolderEntryOpened { path, kind })
                    }
                    None => Task::none(),
                };
                Task::batch([outcome.task, reveal, open])
            }
            FolderPreviewMessage::Listed {
                generation,
                directory,
                outcome,
            } => folder.accept_listing(generation, directory, outcome),
            FolderPreviewMessage::Changed {
                generation,
                directory,
            } => folder.directory_changed(generation, directory),
            FolderPreviewMessage::MeasurementTick => {
                folder.poll_measurements();
                Task::none()
            }
        }
    }

    /// 监视根和展开着的文件夹；有测量在跑时读数节拍。不是文件夹预览时什么都不订。
    pub fn folder_preview_subscription(&self) -> Subscription<PreviewMessage> {
        let Some(PreviewState::Ready(PreviewContent::Folder(folder))) = &self.preview else {
            return Subscription::none();
        };
        let generation = folder.generation();
        let mut subscriptions = folder
            .watched_directories()
            .into_iter()
            .map(|directory| Subscription::run_with((generation, directory), watch_folder))
            .collect::<Vec<_>>();
        if folder.is_measuring() {
            subscriptions.push(
                iced::time::every(MEASUREMENT_TICK)
                    .map(|_| PreviewMessage::FolderPreview(FolderPreviewMessage::MeasurementTick)),
            );
        }
        Subscription::batch(subscriptions)
    }
}

fn watch_folder(key: &(u64, PathBuf)) -> impl Stream<Item = PreviewMessage> + use<> {
    let (generation, directory) = key.clone();
    iced::stream::channel(1, async move |mut output| {
        match file_core::watch_directory(&directory, FOLDER_WATCH_DEBOUNCE) {
            Ok(mut watcher) => {
                while watcher.recv().await.is_some() {
                    let changed = PreviewMessage::FolderPreview(FolderPreviewMessage::Changed {
                        generation,
                        directory: directory.clone(),
                    });
                    if output.send(changed).await.is_err() {
                        return;
                    }
                }
            }
            // 连轮询都起不来说明文件夹本身读不到，读目录时会报出原因。
            Err(error) => tracing::warn!(
                target: "bennu_preview::folder_preview",
                event = "folder_watch_unavailable",
                path = %directory.display(),
                %error,
                "folder preview will not follow changes"
            ),
        }
        iced::futures::future::pending::<()>().await;
    })
}

/// 键盘选中的行不在视口里时滚到刚好露出它。
fn reveal_row(
    viewport: TreeViewport,
    row: Option<usize>,
    scroll_region_id: iced::widget::Id,
) -> Task<PreviewMessage> {
    let Some(row) = row else {
        return Task::none();
    };
    let top = row as f32 * TREE_ROW_HEIGHT;
    let bottom = top + TREE_ROW_HEIGHT;
    let offset = if top < viewport.offset {
        top
    } else if bottom > viewport.offset + viewport.height {
        bottom - viewport.height
    } else {
        return Task::none();
    };
    iced::widget::operation::scroll_to(
        scroll_region_id,
        scrollable::AbsoluteOffset { x: 0.0, y: offset },
    )
}
