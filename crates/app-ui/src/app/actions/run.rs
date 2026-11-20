//! 一个动作的一次运行：一步一步做成队列任务，上一步的结果交给下一步。
//!
//! 一步失败或被取消就停下，后面的不再跑；已经做完的保留，照常进撤销历史、照常通知，
//! 没有清理分支。整个动作结束时做过的各步记成一条撤销记录。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use file_operations::{FileOperationEnqueueOutcome, FileOperationOutcome};
use iced::Task;

use super::steps::{prepare_step, FollowUp, PreparedStep, StepContext, StepPlan};
use super::ActionMessage;
use crate::actions::{ActionDefinition, ActionNotice, ActionStep, RunValues, ServiceMenuAction};
use crate::app::FileBrowser;
use crate::model::Message;
use crate::operation_history::TaskEnd;

#[derive(Debug, Default)]
pub(crate) struct ActionRuns {
    runs: HashMap<u64, ActionRun>,
    next_run: u64,
    /// 队列面板上动作任务的标题。运行结束后任务行还留在面板上，标题也留着。
    labels: HashMap<u64, String>,
}

/// 一次运行要做的：动作文件、超级输入框的管道都编成它。运行用不着动作文件的身份和路径。
#[derive(Debug, Clone)]
pub(crate) struct RunPlan {
    pub(crate) name: String,
    pub(crate) steps: Vec<ActionStep>,
    pub(crate) notice: ActionNotice,
    /// 各步都做完后把结果交给它启动（管道的最后一段是服务菜单时）。
    pub(crate) then: Option<Arc<ServiceMenuAction>>,
}

impl From<&ActionDefinition> for RunPlan {
    fn from(definition: &ActionDefinition) -> Self {
        Self {
            name: definition.name.clone(),
            steps: definition.steps.clone(),
            notice: definition.notice.clone(),
            then: None,
        }
    }
}

#[derive(Debug)]
struct ActionRun {
    plan: RunPlan,
    context: StepContext,
    step: usize,
    /// 这一步拿到的项。
    input: Vec<PathBuf>,
    /// 这一步做出来的项，这一步做完后交给下一步。
    produced: Vec<PathBuf>,
    /// 这一步放进队列、还没结束的任务，和它结束后接着要做的。
    waiting: Vec<(u64, Option<FollowUp>)>,
    /// 个别项没做成，"名字：原因"。
    failures: Vec<String>,
    /// 做成的各步，按执行顺序，结束时记成一条撤销记录。
    outcomes: Vec<FileOperationOutcome>,
    stop: Option<RunStop>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RunStop {
    Failed(String),
    Canceled,
}

/// 运行结束时交给通知的东西。
pub(super) struct FinishedRun {
    pub(super) plan: RunPlan,
    pub(super) results: Vec<PathBuf>,
    pub(super) failures: Vec<String>,
    pub(super) stop: Option<RunStop>,
}

impl ActionRuns {
    pub(crate) fn owns(&self, task_id: u64) -> bool {
        self.run_waiting_for(task_id).is_some()
    }

    #[cfg(test)]
    pub(crate) fn runs_started(&self) -> u64 {
        self.next_run
    }

    pub(crate) fn label_for(&self, task_id: u64) -> Option<&str> {
        self.labels.get(&task_id).map(String::as_str)
    }

    fn run_waiting_for(&self, task_id: u64) -> Option<u64> {
        self.runs
            .iter()
            .find(|(_, run)| run.waiting.iter().any(|(waiting, _)| *waiting == task_id))
            .map(|(run_id, _)| *run_id)
    }
}

impl ActionRun {
    fn current_step(&self) -> Option<&ActionStep> {
        self.plan.steps.get(self.step)
    }

    fn label(&self) -> String {
        let step = self
            .current_step()
            .map(ActionStep::label)
            .unwrap_or_default();
        tr!(
            "%1 · %2 (%3/%4)",
            self.plan.name,
            step,
            self.step + 1,
            self.plan.steps.len()
        )
    }
}

impl FileBrowser {
    pub(crate) fn start_action_run(&mut self, plan: RunPlan, items: Vec<PathBuf>) -> Task<Message> {
        let working_directory = self
            .tab()
            .current_directory()
            .map(PathBuf::from)
            .or_else(|| {
                items
                    .first()
                    .and_then(|item| item.parent())
                    .map(PathBuf::from)
            })
            .unwrap_or_else(file_core::app_dirs::home_dir);
        let context = StepContext {
            selected: items.clone(),
            values: RunValues::starting_now(),
            working_directory,
        };
        let queue = &self.operation_queue;
        self.action_runs
            .labels
            .retain(|task_id, _| queue.operation(*task_id).is_some());
        let run_id = self.action_runs.next_run;
        self.action_runs.next_run += 1;
        self.action_runs.runs.insert(
            run_id,
            ActionRun {
                plan,
                context,
                step: 0,
                input: items,
                produced: Vec::new(),
                waiting: Vec::new(),
                failures: Vec::new(),
                outcomes: Vec::new(),
                stop: None,
            },
        );
        self.begin_action_step(run_id)
    }

    /// 开始这一步；没有下一步了就结束。
    fn begin_action_step(&mut self, run_id: u64) -> Task<Message> {
        let Some(run) = self.action_runs.runs.get_mut(&run_id) else {
            return Task::none();
        };
        let Some(step) = run.current_step().cloned() else {
            return self.finish_action_run(run_id);
        };
        if step == ActionStep::CopyPaths {
            // 只写剪贴板，不用等：这一批原样交给下一步。
            let paths = run.input.clone();
            let copy = self.copy_paths_text(&paths);
            return Task::batch([copy, self.advance_action_run(run_id, paths)]);
        }
        let input = run.input.clone();
        let context = run.context.clone();
        Task::perform(prepare_step(step, input, context), move |prepared| {
            Message::Action(ActionMessage::StepPrepared {
                run: run_id,
                prepared,
            })
        })
    }

    pub(super) fn accept_action_step(
        &mut self,
        run_id: u64,
        prepared: PreparedStep,
    ) -> Task<Message> {
        let plan = match prepared {
            PreparedStep::Ready(plan) => plan,
            PreparedStep::Failed(reason) => {
                if let Some(run) = self.action_runs.runs.get_mut(&run_id) {
                    run.stop = Some(RunStop::Failed(reason));
                }
                return self.settle_action_step(run_id);
            }
        };
        let StepPlan {
            work,
            passed_through,
            skipped,
        } = plan;
        let Some(run) = self.action_runs.runs.get_mut(&run_id) else {
            return Task::none();
        };
        run.failures.extend(skipped);
        run.produced.extend(passed_through);
        let label = run.label();
        for step_work in work {
            if run.stop.is_some() {
                break;
            }
            match self.operation_queue.enqueue(step_work.operation) {
                FileOperationEnqueueOutcome::Queued { task_id } => {
                    run.waiting.push((task_id, step_work.then));
                    self.action_runs.labels.insert(task_id, label.clone());
                }
                FileOperationEnqueueOutcome::Rejected { error } => {
                    run.stop = Some(RunStop::Failed(error));
                }
            }
        }
        Task::batch([
            self.settle_action_step(run_id),
            self.continue_file_operation_persistence(),
            self.launch_ready_driver(),
        ])
    }

    /// 动作里的一个任务结束了（任务结束收口交过来的）。
    pub(in crate::app) fn accept_action_task_end(
        &mut self,
        task_id: u64,
        end: TaskEnd<'_>,
    ) -> Task<Message> {
        let Some(run_id) = self.action_runs.run_waiting_for(task_id) else {
            return Task::none();
        };
        let Some(run) = self.action_runs.runs.get_mut(&run_id) else {
            return Task::none();
        };
        let position = run
            .waiting
            .iter()
            .position(|(waiting, _)| *waiting == task_id)
            .expect("the run waits for this task");
        let (_, follow_up) = run.waiting.remove(position);
        let mut follow_up_task = Task::none();
        match end {
            TaskEnd::Completed(outcome) => {
                run.produced.extend(produced_paths(outcome));
                if let FileOperationOutcome::Convert { failures, .. } = outcome {
                    run.failures.extend(failures.iter().cloned());
                }
                run.outcomes.push(outcome.clone());
                if let (
                    Some(follow_up),
                    FileOperationOutcome::CreateDirectories { leaf_paths, .. },
                ) = (follow_up, outcome)
                {
                    if let Some(folder) = leaf_paths.first() {
                        follow_up_task =
                            self.enqueue_follow_up(run_id, follow_up.operation(folder));
                    }
                }
            }
            TaskEnd::Failed { partial, error } => {
                run.outcomes.push(partial.clone());
                run.stop.get_or_insert(RunStop::Failed(error.to_owned()));
            }
            TaskEnd::Canceled(partial) => {
                run.outcomes.push(partial.clone());
                run.stop.get_or_insert(RunStop::Canceled);
            }
            // 恢复完它还会再结束一次，那时按普通操作记；动作不再往下走。
            TaskEnd::Interrupted(_) => {
                run.stop
                    .get_or_insert(RunStop::Failed(tr!("The task was interrupted")));
            }
        }
        Task::batch([follow_up_task, self.settle_action_step(run_id)])
    }

    fn enqueue_follow_up(
        &mut self,
        run_id: u64,
        operation: file_operations::QueuedFileOperation,
    ) -> Task<Message> {
        let Some(run) = self.action_runs.runs.get_mut(&run_id) else {
            return Task::none();
        };
        match self.operation_queue.enqueue(operation) {
            FileOperationEnqueueOutcome::Queued { task_id } => {
                run.waiting.push((task_id, None));
                self.action_runs.labels.insert(task_id, run.label());
            }
            FileOperationEnqueueOutcome::Rejected { error } => {
                run.stop.get_or_insert(RunStop::Failed(error));
            }
        }
        Task::batch([
            self.continue_file_operation_persistence(),
            self.launch_ready_driver(),
        ])
    }

    /// 这一步的任务都结束了：停下了就结束，否则把做出来的交给下一步。
    fn settle_action_step(&mut self, run_id: u64) -> Task<Message> {
        let Some(run) = self.action_runs.runs.get_mut(&run_id) else {
            return Task::none();
        };
        if !run.waiting.is_empty() {
            return Task::none();
        }
        if run.stop.is_some() {
            return self.finish_action_run(run_id);
        }
        let produced = std::mem::take(&mut run.produced);
        self.advance_action_run(run_id, produced)
    }

    fn advance_action_run(&mut self, run_id: u64, mut next_input: Vec<PathBuf>) -> Task<Message> {
        let Some(run) = self.action_runs.runs.get_mut(&run_id) else {
            return Task::none();
        };
        let mut seen = std::collections::HashSet::new();
        next_input.retain(|path| seen.insert(path.clone()));
        run.input = next_input;
        run.step += 1;
        self.begin_action_step(run_id)
    }

    fn finish_action_run(&mut self, run_id: u64) -> Task<Message> {
        let Some(run) = self.action_runs.runs.remove(&run_id) else {
            return Task::none();
        };
        self.operation_history.record_action(&run.outcomes);
        let finished = FinishedRun {
            plan: run.plan,
            // 停下时交出来的是停下那一步之前做成的。
            results: if run.stop.is_some() {
                run.produced
            } else {
                run.input
            },
            failures: run.failures,
            stop: run.stop,
        };
        let mut tasks = vec![self.action_notification_command(&finished)];
        if finished.stop.is_none() {
            self.select_results_here(&finished.results);
            if let Some(menu) = &finished.plan.then {
                tasks.push(self.launch_service_menu(menu, finished.results.clone()));
            }
        }
        Task::batch(tasks)
    }

    /// 结果在当前文件夹里就选中它们（和创建副本一样）。
    fn select_results_here(&mut self, results: &[PathBuf]) {
        let Some(directory) = self.tab().current_directory().map(PathBuf::from) else {
            return;
        };
        let here = results
            .iter()
            .filter(|path| path.parent() == Some(directory.as_path()))
            .cloned()
            .collect::<Vec<_>>();
        if !here.is_empty() {
            self.select_operation_result_paths(here);
        }
    }
}

/// 一个任务做出来、交给下一步的项。
fn produced_paths(outcome: &FileOperationOutcome) -> Vec<PathBuf> {
    match outcome {
        FileOperationOutcome::Convert { outputs, .. } => outputs.clone(),
        FileOperationOutcome::CreatedArchive { archive } => vec![archive.clone()],
        FileOperationOutcome::Extracted { placed } => vec![placed.clone()],
        FileOperationOutcome::Copy { transfers, .. }
        | FileOperationOutcome::Move { transfers, .. } => transfers
            .iter()
            .map(|transfer| transfer.target.clone())
            .collect(),
        FileOperationOutcome::GatheredIntoNewFolder { moved, .. }
        | FileOperationOutcome::CreateDirectories { moved, .. } => moved
            .iter()
            .map(|transfer| transfer.target.clone())
            .collect(),
        FileOperationOutcome::NoHistory
        | FileOperationOutcome::Rename { .. }
        | FileOperationOutcome::BatchRename { .. }
        | FileOperationOutcome::CreateDirectory { .. }
        | FileOperationOutcome::CreateEmptyFile { .. }
        | FileOperationOutcome::Trash { .. }
        | FileOperationOutcome::Restore { .. }
        | FileOperationOutcome::NewFolderUngathered { .. } => Vec::new(),
    }
}

#[cfg(test)]
#[path = "run_tests.rs"]
mod tests;
