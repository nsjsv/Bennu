//! 超级输入框的管道：按 `|` 前面写的挑出项来，编成一次动作运行（进任务队列、整体撤销），或者
//! Ctrl+S 存成动作。读法在 `crate::super_box::pipe`。

use std::ffi::CString;
use std::path::PathBuf;
use std::sync::Arc;

use file_search::{
    SearchMatchMode, SearchOrder, SearchQuery, SearchScope, SearchTextScope, MAX_QUERY_RESULTS,
};
use iced::Task;
use tokio_util::sync::CancellationToken;

use super::actions::RunPlan;
use super::super_box::SuperBoxMessage;
use super::super_box_find::found_or_why;
use super::FileBrowser;
use crate::actions::{ActionId, ActionNotice, ActionStep};
use crate::model::{IndexedSearchOutcome, Message};
use crate::super_box::{Pipe, PipeAction, PipeSource, Stage, MOST_PICKED};

impl FileBrowser {
    /// 管道里能写名字的动作：自己的动作和服务菜单，不管适不适用于现在选中的项（写了就是要用）。
    pub(super) fn super_box_pipe_actions(&self) -> Vec<PipeAction> {
        let own = self
            .action_library
            .definitions()
            .iter()
            .map(|definition| PipeAction {
                name: definition.name.clone(),
                id: definition.id.clone(),
                ends_the_pipe: matches!(definition.steps.last(), Some(ActionStep::Trash(_))),
            });
        let service_menus = self
            .action_library
            .service_menus()
            .iter()
            .map(|action| PipeAction {
                name: action.name.clone(),
                id: action.id(),
                ends_the_pipe: true,
            });
        own.chain(service_menus).collect()
    }

    /// 按挑法挑出项来，交回框（`Picked`）。最多要 `MOST_PICKED + 1` 个：多出一个就知道太多了。
    pub(super) fn pick_super_box_items(&self, source: PipeSource) -> Task<Message> {
        let picked = |source: PipeSource, items: Result<Vec<PathBuf>, String>| {
            Message::SuperBox(SuperBoxMessage::Picked {
                source: Box::new(source),
                items,
            })
        };
        match &source {
            PipeSource::Selection => {
                let items = self.selected_paths_for_operation();
                Task::done(picked(source, Ok(items)))
            }
            PipeSource::Glob(pattern) => {
                let items = self.matching_names_here(pattern);
                Task::done(picked(source, Ok(items)))
            }
            PipeSource::Find(query) => {
                let query = SearchQuery {
                    query_id: 0,
                    terms: query.terms.clone(),
                    text_scope: SearchTextScope::NameAndContent,
                    match_mode: SearchMatchMode::Plain,
                    scope: SearchScope::Global,
                    recursive: true,
                    filters: query.search_filters.clone(),
                    order: SearchOrder::Relevance,
                    limit: MAX_QUERY_RESULTS,
                    cursor: None,
                };
                Task::perform(find_all(query), move |items| picked(source.clone(), items))
            }
        }
    }

    /// 当前文件夹里名字匹配通配的项，看已经列出来的条目，不读盘。通配用系统的 `fnmatch`，同
    /// shell：`*` 不匹配开头的 `.`。
    fn matching_names_here(&self, pattern: &str) -> Vec<PathBuf> {
        let tab = self.tab();
        let (Some(folder), Ok(pattern)) = (tab.current_directory(), CString::new(pattern)) else {
            return Vec::new();
        };
        tab.entries
            .iter()
            .filter(|entry| entry.path().parent() == Some(folder))
            .filter(|entry| {
                use std::os::unix::ffi::OsStrExt;
                CString::new(entry.name().as_bytes()).is_ok_and(|name| unsafe {
                    libc::fnmatch(pattern.as_ptr(), name.as_ptr(), libc::FNM_PERIOD) == 0
                })
            })
            .map(|entry| entry.path().to_path_buf())
            .collect()
    }

    /// 管道编成一次运行：步骤原样、动作换成它的各步；最后一段是服务菜单时各步做完交给它。
    fn pipe_plan(&self, pipe: &Pipe, name: String) -> Result<RunPlan, String> {
        let mut plan = RunPlan {
            name,
            steps: Vec::new(),
            notice: ActionNotice::Standard,
            then: None,
        };
        for (stage, label) in pipe.stages.iter().zip(&pipe.labels) {
            match stage {
                Stage::Step(step) => plan.steps.push(step.clone()),
                Stage::Action(id @ ActionId::File(_)) => {
                    let definition = self
                        .action_library
                        .definitions()
                        .iter()
                        .find(|definition| definition.id == *id)
                        .ok_or_else(|| tr!("“%1” is no longer there", label))?;
                    plan.steps.extend(definition.steps.iter().cloned());
                }
                Stage::Action(ActionId::ServiceMenu(key)) => {
                    let menu = self
                        .action_library
                        .service_menus()
                        .iter()
                        .find(|menu| menu.key == *key)
                        .ok_or_else(|| tr!("“%1” is no longer there", label))?;
                    plan.then = Some(Arc::clone(menu));
                }
            }
        }
        Ok(plan)
    }

    /// 回车：对挑出来的项跑这一串，框关掉。
    pub(super) fn run_super_box_pipe(&mut self) -> Task<Message> {
        let Some(super_box) = self.super_box.as_ref() else {
            return Task::none();
        };
        let (Some(pipe), Some(items)) = (super_box.pipe(), super_box.picked()) else {
            return Task::none();
        };
        let (generation, history) = (super_box.generation(), super_box.history_text());
        let items = items.to_vec();
        let plan = match self.pipe_plan(pipe, super_box.input().trim().to_owned()) {
            Ok(plan) => plan,
            Err(reason) => return self.super_box_notice(generation, reason),
        };
        self.super_box = None;
        let remembered = self.remember_super_box_input(history);
        let started = match (plan.steps.is_empty(), &plan.then) {
            // 只有一个服务菜单：直接启动。
            (true, Some(menu)) => {
                let menu = Arc::clone(menu);
                self.launch_service_menu(&menu, items)
            }
            _ => self.start_action_run(plan, items),
        };
        Task::batch([remembered, started])
    }

    /// Ctrl+S：在设置里用这几步起草一个新动作。服务菜单写不进动作文件，以它结尾的存不了。
    pub(super) fn save_super_box_pipe(&mut self) -> Task<Message> {
        let Some(super_box) = self.super_box.as_ref() else {
            return Task::none();
        };
        let Some(pipe) = super_box.pipe() else {
            return Task::none();
        };
        let generation = super_box.generation();
        let name = pipe.labels.join(" → ");
        let plan = match self.pipe_plan(pipe, name.clone()) {
            Ok(plan) if plan.then.is_some() => {
                return self.super_box_notice(
                    generation,
                    tr!("A pipe that ends with a service menu cannot be saved as an action"),
                )
            }
            Ok(plan) => plan,
            Err(reason) => return self.super_box_notice(generation, reason),
        };
        match self.draft_action_from_steps(name, &plan.steps) {
            Ok(opened) => {
                self.super_box = None;
                opened
            }
            Err(reason) => self.super_box_notice(generation, reason),
        }
    }

    fn super_box_notice(&mut self, generation: u64, notice: String) -> Task<Message> {
        if let Some(super_box) = self.super_box.as_mut() {
            super_box.show_notice(generation, notice);
        }
        Task::none()
    }
}

/// 索引里找到的全部路径，按页要（服务一次最多给 `MAX_QUERY_RESULTS` 条），够 `MOST_PICKED + 1`
/// 个就停：多出一个就知道太多了。
async fn find_all(mut query: SearchQuery) -> Result<Vec<PathBuf>, String> {
    let mut found = Vec::new();
    loop {
        let outcome = crate::commands::search_once(query.clone(), CancellationToken::new()).await;
        let batch = match outcome {
            IndexedSearchOutcome::Batch(batch) => batch,
            other => {
                return match found_or_why(other) {
                    Some(Err(reason)) => Err(reason),
                    _ => Ok(found),
                }
            }
        };
        found.extend(batch.hits.into_iter().map(|hit| hit.path));
        match batch.next_cursor {
            Some(cursor) if !batch.finished && found.len() <= MOST_PICKED => {
                query.cursor = Some(cursor);
            }
            _ => return Ok(found),
        }
    }
}

#[cfg(test)]
#[path = "super_box_pipe_tests.rs"]
mod tests;
