//! 规则：写了 `[watch]` 的动作自己跑。监视它的文件夹（和打开文件夹时同一个监视器，不进子文件夹），
//! 新出现、改名过来、改过的文件等 `SETTLE` 不再变（下载、复制写完了）再按查询比对，对得上就对它跑
//! 这个动作。比对在本地做，和搜索服务的后备遍历是同一套（名字、按名字猜的类型、大小、时间）：服务
//! 没有“索引变了”的推送。
//!
//! 只管监视开始以后的变化。同一个文件一条规则只跑一次；动作做出来的文件也算处理过，不然“转成
//! webp”会把自己的输出再转一遍。一分钟里超过 `MOST_PER_MINUTE` 个就停下这条规则、通知一次，重启
//! 后恢复：一下子来这么多多半是解压、同步，不是要一个个处理的。

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use file_core::watch::{watch_directory, DirectoryEntryChanges, ResolvedEntryChange};
use file_core::FileKind;
use file_search::filter_semantics::EntryFacts;
use file_search::text_matching::{normalize_search_text, SearchTerms};
use file_search::SearchFileKind;
use iced::futures::SinkExt;
use iced::{Subscription, Task};

use super::run::RunPlan;
use super::ActionMessage;
use crate::actions::ActionId;
use crate::app::FileBrowser;
use crate::commands::publish_desktop_notification_command;
use crate::model::Message;

/// 文件这么久没再变才算写完。
const SETTLE: Duration = Duration::from_secs(2);
/// 一条规则一分钟里最多处理这么多个文件。
const MOST_PER_MINUTE: usize = 100;
const MINUTE: Duration = Duration::from_secs(60);
const WATCH_DEBOUNCE: Duration = Duration::from_millis(250);

#[derive(Debug, Default)]
pub(crate) struct Rules {
    handled: HashSet<(ActionId, PathBuf)>,
    recent: HashMap<ActionId, VecDeque<Instant>>,
    stopped: HashSet<ActionId>,
}

impl Rules {
    /// 规则跑出来的文件：它不再拿它们跑。
    pub(super) fn remember_results(&mut self, rule: &ActionId, results: &[PathBuf]) {
        self.handled
            .extend(results.iter().map(|path| (rule.clone(), path.clone())));
    }

    /// 这一分钟里还能再处理一个：能就记下这一个。
    fn take_turn(&mut self, rule: &ActionId, now: Instant) -> bool {
        let recent = self.recent.entry(rule.clone()).or_default();
        while recent
            .front()
            .is_some_and(|at| now.duration_since(*at) >= MINUTE)
        {
            recent.pop_front();
        }
        if recent.len() >= MOST_PER_MINUTE {
            return false;
        }
        recent.push_back(now);
        true
    }
}

/// 写完了的文件这一刻的样子，比对查询用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SettledFile {
    size: u64,
    modified: Option<SystemTime>,
    accessed: Option<SystemTime>,
    created: Option<SystemTime>,
}

impl FileBrowser {
    /// 每条规则监视它的文件夹；动作文件改了，订阅跟着换。
    pub(crate) fn rule_watch_subscriptions(&self) -> Vec<Subscription<Message>> {
        self.action_library
            .definitions()
            .iter()
            .filter_map(|definition| {
                let watch = definition.watch.as_ref()?;
                Some(Subscription::run_with(
                    (definition.id.clone(), watch.folder.clone()),
                    rule_watch_stream,
                ))
            })
            .collect()
    }

    /// 文件夹里有变化：新出现、改名过来、改过的普通文件，等一会儿再看它写完没有。
    pub(super) fn accept_rule_folder_changes(
        &mut self,
        rule: ActionId,
        changes: DirectoryEntryChanges,
    ) -> Task<Message> {
        if self.rules.stopped.contains(&rule) {
            return Task::none();
        }
        let files = changes
            .changes
            .into_iter()
            .filter_map(|change| match change {
                ResolvedEntryChange::Added(entry)
                | ResolvedEntryChange::ContentChanged(entry)
                | ResolvedEntryChange::Renamed { to: entry, .. } => {
                    (entry.kind() == FileKind::File && !entry.is_symlink())
                        .then(|| entry.path().to_path_buf())
                }
                ResolvedEntryChange::Removed { .. } | ResolvedEntryChange::RescanRequired => None,
            });
        Task::batch(files.map(|path| settle(rule.clone(), path)))
    }

    /// 写完了：没处理过、对得上查询、这一分钟还没满就跑。
    pub(super) fn accept_rule_file_settled(
        &mut self,
        rule: ActionId,
        path: PathBuf,
        file: SettledFile,
    ) -> Task<Message> {
        if self.rules.stopped.contains(&rule)
            || self.rules.handled.contains(&(rule.clone(), path.clone()))
        {
            return Task::none();
        }
        let Some(definition) = self
            .action_library
            .definitions()
            .iter()
            .find(|definition| definition.id == rule)
            .cloned()
        else {
            return Task::none();
        };
        let Some(watch) = &definition.watch else {
            return Task::none();
        };
        if path.parent() != Some(watch.folder.as_path())
            || !matches_query(&watch.query, &path, file)
        {
            return Task::none();
        }
        if !self.rules.take_turn(&rule, Instant::now()) {
            self.rules.stopped.insert(rule);
            return publish_desktop_notification_command(
                tr!("Rule “%1” stopped", definition.name),
                tr!(
                    "More than %1 files came in a minute; it runs again after Bennu restarts",
                    MOST_PER_MINUTE
                ),
            );
        }
        self.rules.handled.insert((rule.clone(), path.clone()));
        let plan = RunPlan {
            rule: Some(rule),
            ..RunPlan::from(&*definition)
        };
        self.start_action_run(plan, vec![path])
    }
}

/// 先看一眼，等 `SETTLE` 再看一眼：大小和修改时间都没变才交回去。
fn settle(rule: ActionId, path: PathBuf) -> Task<Message> {
    let looked_at = path.clone();
    Task::future(async move {
        let before = settled_file(&looked_at).await;
        tokio::time::sleep(SETTLE).await;
        let after = settled_file(&looked_at).await;
        before.filter(|_| before == after)
    })
    .and_then(move |file| {
        Task::done(Message::Action(ActionMessage::RuleFileSettled {
            rule: rule.clone(),
            path: path.clone(),
            file,
        }))
    })
}

async fn settled_file(path: &Path) -> Option<SettledFile> {
    let metadata = tokio::fs::symlink_metadata(path).await.ok()?;
    metadata.is_file().then(|| SettledFile {
        size: metadata.len(),
        modified: metadata.modified().ok(),
        accessed: metadata.accessed().ok(),
        created: metadata.created().ok(),
    })
}

/// 和搜索服务的后备遍历同一套比对：词都在名字里，筛选（类型按名字猜）都满足。
fn matches_query(query: &str, path: &Path, file: SettledFile) -> bool {
    let Ok(found) = crate::super_box::read_find_query(query) else {
        return false;
    };
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let search_name = normalize_search_text(&name);
    if !SearchTerms::parse(&found.terms).all_in_name(&search_name) {
        return false;
    }
    let types = file_core::mime::MimeDatabase::system();
    let mime_type = file_core::mime::type_for_name_only(&types, path);
    let milliseconds = |time: Option<SystemTime>| {
        time.and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
            .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
    };
    found.search_filters.resolve(&types).matches(&EntryFacts {
        search_name: &search_name,
        kind: SearchFileKind::File,
        mime_type: mime_type.as_deref(),
        size: file.size,
        modified_ms: milliseconds(file.modified),
        accessed_ms: milliseconds(file.accessed),
        created_ms: milliseconds(file.created),
    })
}

fn rule_watch_stream(
    (rule, folder): &(ActionId, PathBuf),
) -> impl iced::futures::Stream<Item = Message> + 'static {
    let (rule, folder) = (rule.clone(), folder.clone());
    iced::stream::channel(8, async move |mut output| {
        match watch_directory(&folder, WATCH_DEBOUNCE) {
            Ok(mut watcher) => {
                while let Some(changes) = watcher.recv().await {
                    let message = Message::Action(ActionMessage::RuleFolderChanged {
                        rule: rule.clone(),
                        changes,
                    });
                    if output.send(message).await.is_err() {
                        break;
                    }
                }
            }
            // 文件夹没了、读不了：规则不跑，留个记录。
            Err(error) => tracing::warn!(
                target: "app_ui::actions",
                event = "rule_folder_unwatchable",
                folder = %folder.display(),
                %error,
                "a rule's folder cannot be watched"
            ),
        }
        iced::futures::future::pending().await
    })
}

#[cfg(test)]
#[path = "rules_tests.rs"]
mod tests;
