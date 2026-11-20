//! 动作文件夹在后台整份读出来，有变化时再整份重读，读完一次替换。界面线程不读盘。
//!
//! 文件夹还不存在时监视最近的已有上级，等它出现；没有定时轮询。

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use file_core::{watch_directory, ResolvedEntryChange};
use iced::futures::SinkExt;
use iced::Subscription;

use super::ActionMessage;
use crate::actions::{action_folder, ActionLibrary};
use crate::model::Message;

/// 编辑器保存时常是先写临时文件再改名，连着几次变化只重读一次。
const RELOAD_DEBOUNCE: Duration = Duration::from_millis(300);

pub(crate) fn action_library_subscription() -> Subscription<Message> {
    Subscription::run_with(action_folder(), action_library_stream)
}

fn action_library_stream(folder: &PathBuf) -> impl iced::futures::Stream<Item = Message> + 'static {
    let folder = folder.clone();
    iced::stream::channel(1, async move |mut output| loop {
        let reading = folder.clone();
        let library = tokio::task::spawn_blocking(move || ActionLibrary::load(&reading))
            .await
            .unwrap_or_else(|_| ActionLibrary::empty());
        if output
            .send(Message::Action(ActionMessage::LibraryLoaded(library)))
            .await
            .is_err()
        {
            return;
        }
        if !wait_for_change(&folder).await {
            return iced::futures::future::pending().await;
        }
    })
}

/// 等到动作文件夹（或者在它出现之前，通向它的上级）有变化。监视不了时返回 false，
/// 之后就不再重读，直到下次启动。
async fn wait_for_change(folder: &Path) -> bool {
    let watched = folder
        .ancestors()
        .find(|ancestor| ancestor.is_dir())
        .unwrap_or(folder)
        .to_path_buf();
    // 监视的是上级时，只看通向动作文件夹的那一段名字（不为 config.toml 的每次写入重读）。
    let next_name = (watched != folder)
        .then(|| {
            folder
                .strip_prefix(&watched)
                .ok()
                .and_then(|rest| rest.iter().next())
                .map(OsStr::to_os_string)
        })
        .flatten();
    let mut watcher = match watch_directory(&watched, RELOAD_DEBOUNCE) {
        Ok(watcher) => watcher,
        Err(error) => {
            tracing::warn!(
                target: "app_ui::actions",
                event = "action_folder_watch_unavailable",
                path = %watched.display(),
                %error,
                "changes to action files take effect after a restart"
            );
            return false;
        }
    };
    while let Some(observed) = watcher.recv().await {
        let relevant = match &next_name {
            None => true,
            Some(name) => observed
                .changes
                .iter()
                .any(|change| change_names(change, name)),
        };
        if relevant {
            return true;
        }
    }
    false
}

fn change_names(change: &ResolvedEntryChange, name: &OsStr) -> bool {
    let named = |path: &Path| path.file_name() == Some(name);
    match change {
        ResolvedEntryChange::Added(entry) | ResolvedEntryChange::ContentChanged(entry) => {
            named(entry.path())
        }
        ResolvedEntryChange::Removed { path } => named(path),
        ResolvedEntryChange::Renamed { from, to } => named(from) || named(to.path()),
        ResolvedEntryChange::RescanRequired => true,
    }
}
