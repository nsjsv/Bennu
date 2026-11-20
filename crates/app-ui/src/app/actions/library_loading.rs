//! 动作在后台整份读出来，有变化时再整份重读，读完一次替换。界面线程不读盘。
//!
//! 要看的位置有：动作文件夹、每个数据目录下的服务菜单文件夹、`kservicemenurc`。
//! 还不存在的，监视它最近的已有上级，等它出现；没有定时轮询。

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Duration;

use file_core::{watch_directory, DirectoryWatcher, ResolvedEntryChange};
use iced::futures::{FutureExt, SinkExt};
use iced::Subscription;

use super::ActionMessage;
use crate::actions::{ActionLibrary, ActionSources};
use crate::config::UiLanguage;
use crate::model::Message;

/// 编辑器保存时常是先写临时文件再改名，连着几次变化只重读一次。
const RELOAD_DEBOUNCE: Duration = Duration::from_millis(300);

/// 界面语言变了，订阅的键跟着变：服务菜单的名字和问题说明都按新语言重读。
pub(crate) fn action_library_subscription(language: UiLanguage) -> Subscription<Message> {
    Subscription::run_with(ActionSources::current(language), action_library_stream)
}

fn action_library_stream(
    sources: &ActionSources,
) -> impl iced::futures::Stream<Item = Message> + 'static {
    let sources = sources.clone();
    iced::stream::channel(1, async move |mut output| loop {
        let reading = sources.clone();
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
        if !wait_for_change(&sources.watched_paths()).await {
            return iced::futures::future::pending().await;
        }
    })
}

/// 一个被监视的目录要看哪些变化：所有的（它本身就是要看的位置），或者只看通向还不存在的
/// 位置的那几段名字（不为 `~/.config` 里别的文件的每次写入重读）。
#[derive(Debug, PartialEq, Eq)]
enum Interest {
    Everything,
    Names(BTreeSet<OsString>),
}

/// 每个位置换成要监视的目录，同一个目录只监视一次。
fn watch_plan(paths: &[PathBuf]) -> BTreeMap<PathBuf, Interest> {
    let mut plan = BTreeMap::new();
    for path in paths {
        let watched = path
            .ancestors()
            .find(|ancestor| ancestor.is_dir())
            .unwrap_or(path)
            .to_path_buf();
        let next_name = (watched != *path)
            .then(|| {
                path.strip_prefix(&watched)
                    .ok()
                    .and_then(|rest| rest.iter().next())
                    .map(OsStr::to_os_string)
            })
            .flatten();
        let interest = plan
            .entry(watched)
            .or_insert_with(|| Interest::Names(BTreeSet::new()));
        match (next_name, &mut *interest) {
            (Some(name), Interest::Names(names)) => {
                names.insert(name);
            }
            (Some(_), Interest::Everything) => {}
            (None, _) => *interest = Interest::Everything,
        }
    }
    plan
}

/// 等到任何一个位置有相关变化。一个都监视不了时返回 false，之后就不再重读，直到下次启动。
async fn wait_for_change(paths: &[PathBuf]) -> bool {
    let mut watchers = Vec::new();
    for (watched, interest) in watch_plan(paths) {
        match watch_directory(&watched, RELOAD_DEBOUNCE) {
            Ok(watcher) => watchers.push((watcher, interest)),
            Err(error) => tracing::warn!(
                target: "app_ui::actions",
                event = "action_folder_watch_unavailable",
                path = %watched.display(),
                %error,
                "changes here take effect after a restart"
            ),
        }
    }
    if watchers.is_empty() {
        return false;
    }
    let waits = watchers
        .iter_mut()
        .map(|(watcher, interest)| next_relevant_change(watcher, interest).boxed())
        .collect::<Vec<_>>();
    let (changed, _, _) = iced::futures::future::select_all(waits).await;
    changed
}

/// 这个监视器看到相关变化时返回 true；它停了时返回 false。
async fn next_relevant_change(watcher: &mut DirectoryWatcher, interest: &Interest) -> bool {
    while let Some(observed) = watcher.recv().await {
        let relevant = match interest {
            Interest::Everything => true,
            Interest::Names(names) => observed
                .changes
                .iter()
                .any(|change| names.iter().any(|name| change_names(change, name))),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations_share_a_watched_folder_and_an_existing_one_is_watched_whole() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config");
        let share = root.path().join("share");
        let menus = share.join("kio/servicemenus");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::create_dir_all(&menus).unwrap();

        let plan = watch_plan(&[
            config.join("bennu/actions"),
            config.join("kservicemenurc"),
            menus.clone(),
            share.join("kservices5"),
        ]);

        let names = |names: &[&str]| Interest::Names(names.iter().map(OsString::from).collect());
        assert_eq!(plan.len(), 3);
        assert_eq!(plan[&config], names(&["bennu", "kservicemenurc"]));
        assert_eq!(plan[&share], names(&["kservices5"]));
        assert_eq!(plan[&menus], Interest::Everything);
    }
}
