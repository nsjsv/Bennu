//! 超级输入框执行命令：交给用户的 shell，进任务队列（能暂停、取消），框里等结果；Ctrl+Enter 改成
//! 送进底部终端面板。每条命令最后一次的结果记进状态库，重启后在命令模式的历史里还能看。
//!
//! 哪些任务是框发的命令由这里记（`SuperBoxCommands`），不靠框：命令跑完时框可能已经关了，
//! 结果照样要记下来。

use std::collections::HashMap;
use std::path::PathBuf;

use bennu_state::{CommandOutputStore, StoredCommandOutput, StoredCommandResult};
use iced::Task;

use super::super_box::SuperBoxMessage;
use super::FileBrowser;
use crate::model::Message;
use crate::super_box::{shell_invocation, PastRun, RunEnd, RunState, ShownOutput, PAST_RUNS_KEPT};
use file_operations::{FileOperationCompletion, FileOperationOutcome, QueuedFileOperation};

/// 存进状态库的输出最多这么多（末尾）。
const STORED_OUTPUT_BYTES: usize = 16 * 1024;

/// 框发出去、还没结束的命令（任务号 → 命令），和最近几条命令的结果（新的在前）。
#[derive(Debug, Default)]
pub(crate) struct SuperBoxCommands {
    running: HashMap<u64, String>,
    past: Vec<PastRun>,
}

impl SuperBoxCommands {
    /// 这条命令最后一次的结果。
    pub(crate) fn past(&self, command: &str) -> Option<&PastRun> {
        self.past.iter().find(|run| run.command == command)
    }

    fn remember(&mut self, run: PastRun) {
        self.past.retain(|past| past.command != run.command);
        self.past.insert(0, run);
        self.past.truncate(PAST_RUNS_KEPT);
    }
}

fn stored(run: &PastRun) -> StoredCommandOutput {
    let result = match &run.end {
        RunEnd::Finished(output) => StoredCommandResult::Finished {
            output: tail(&output.text, STORED_OUTPUT_BYTES).to_owned(),
        },
        RunEnd::Failed(reason) => StoredCommandResult::Failed {
            reason: reason.clone(),
        },
    };
    StoredCommandOutput {
        command: run.command.clone(),
        at_ms: run.at_ms,
        result,
    }
}

fn from_stored(record: StoredCommandOutput) -> PastRun {
    PastRun {
        command: record.command,
        at_ms: record.at_ms,
        end: match record.result {
            StoredCommandResult::Finished { output } => {
                RunEnd::Finished(ShownOutput::from_text(output))
            }
            StoredCommandResult::Failed { reason } => RunEnd::Failed(reason),
        },
    }
}

/// 最后 `bytes` 字节，从字符边界切。
fn tail(text: &str, bytes: usize) -> &str {
    let mut start = text.len().saturating_sub(bytes);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

impl FileBrowser {
    /// 和终端面板用同一个 shell：设置里选了就用它，没选就是登录 shell。
    fn super_box_shell(&self) -> PathBuf {
        let chosen = self.user_config.terminal_shell.trim();
        if chosen.is_empty() {
            PathBuf::from(crate::terminal_panel::session::shell_for_user())
        } else {
            PathBuf::from(chosen)
        }
    }

    /// 回车：入队，框里显示“正在运行”。同一个框一次只跑一条。
    pub(super) fn run_super_box_command(&mut self, command: String) -> Task<Message> {
        let running = self
            .super_box
            .as_ref()
            .and_then(|super_box| super_box.run())
            .is_some_and(|run| run.state == RunState::Running);
        let Some(folder) = self.keyboard_target_directory().filter(|_| !running) else {
            return Task::none();
        };
        let invocation = shell_invocation(
            &self.super_box_shell(),
            &command,
            &self.selected_paths_for_operation(),
            &folder,
        );
        let history = self
            .super_box
            .as_ref()
            .map(|super_box| super_box.history_text())
            .unwrap_or_default();
        let (task_id, queued) =
            self.enqueue_file_operation_with_id(QueuedFileOperation::RunCommands {
                invocations: vec![invocation],
            });
        let Some(task_id) = task_id else {
            return queued;
        };
        self.super_box_commands
            .running
            .insert(task_id, command.clone());
        if let Some(super_box) = self.super_box.as_mut() {
            super_box.start_run(task_id, command);
        }
        Task::batch([queued, self.remember_super_box_input(history)])
    }

    /// 停止按钮：取消这条命令的任务（整个进程组都结束）。
    pub(super) fn stop_super_box_command(&mut self) -> Task<Message> {
        let Some(task_id) = self
            .super_box
            .as_ref()
            .and_then(|super_box| super_box.run())
            .filter(|run| run.state == RunState::Running)
            .map(|run| run.task_id)
        else {
            return Task::none();
        };
        self.update(Message::FileOperationCancelRequested(task_id))
    }

    /// 任务结束时：框还开着、是它发的命令，就把结果交给框。
    /// 任务结束时：是框发的命令就记下结果（框关了也记），框还开着就交给框。
    pub(super) fn super_box_task_ended(
        &mut self,
        task_id: u64,
        completion: &FileOperationCompletion,
    ) -> Task<Message> {
        let Some(command) = self.super_box_commands.running.remove(&task_id) else {
            return Task::none();
        };
        let end = match completion {
            FileOperationCompletion::Succeeded(FileOperationOutcome::CommandsRan { output }) => {
                RunEnd::finished(output)
            }
            // 执行命令成功的结果总是 `CommandsRan`；没有输出可显示时当作空输出。
            FileOperationCompletion::Succeeded(_) => RunEnd::finished(&Default::default()),
            FileOperationCompletion::Failed { error, .. }
            | FileOperationCompletion::RecoveryBlocked { error, .. }
            | FileOperationCompletion::RecoveryInterrupted(error, _) => {
                RunEnd::Failed(error.clone())
            }
            // 改名被拒只出现在改名任务上，执行命令不会有；归到“停止”。
            FileOperationCompletion::Canceled(_)
            | FileOperationCompletion::RenameRejected { .. } => RunEnd::Failed(tr!("Stopped")),
        };
        if let Some(super_box) = self.super_box.as_mut() {
            super_box.finish_run(task_id, end.clone());
        }
        let run = PastRun {
            command,
            at_ms: chrono::Local::now().timestamp_millis(),
            end,
        };
        let record = stored(&run);
        self.super_box_commands.remember(run);
        let Some(store) = self.command_output_store() else {
            return Task::none();
        };
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || store.save(&record, PAST_RUNS_KEPT))
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|saved| saved.map_err(|error| error.to_string()))
            },
            |saved| Message::SuperBox(SuperBoxMessage::PastRunSaved(saved.err())),
        )
    }

    /// 启动时（状态库就绪之后）读入以前的命令结果。
    pub(in crate::app) fn load_past_super_box_runs(&mut self) -> Task<Message> {
        let Some(store) = self.command_output_store() else {
            return Task::none();
        };
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || store.read())
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|read| read.map_err(|error| error.to_string()))
                    .map(|records| records.into_iter().map(from_stored).collect())
            },
            |loaded| Message::SuperBox(SuperBoxMessage::PastRunsLoaded(loaded)),
        )
    }

    /// 读到的是旧的，本次已经记下的更新：本次的留在前面。
    pub(super) fn accept_past_super_box_runs(&mut self, loaded: Result<Vec<PastRun>, String>) {
        match loaded {
            Ok(loaded) => {
                for run in loaded.into_iter().rev() {
                    if self.super_box_commands.past(&run.command).is_none() {
                        self.super_box_commands.past.push(run);
                    }
                }
                self.super_box_commands.past.truncate(PAST_RUNS_KEPT);
            }
            // 读不出来不拦着用：本次的照样记、照样写。
            Err(error) => tracing::warn!(
                target: "app_ui::super_box",
                event = "past_command_runs_unreadable",
                error = %error,
                "past command runs could not be read"
            ),
        }
    }

    fn command_output_store(&self) -> Option<CommandOutputStore> {
        self.operation_queue
            .state_database()
            .map(CommandOutputStore::from)
    }

    /// Ctrl+Enter（命令模式）：关框，打开终端面板，把命令打进去。选中项不传：终端里自己写路径。
    pub(super) fn run_super_box_command_in_terminal(&mut self) -> Task<Message> {
        let Some(command) = self
            .super_box
            .as_ref()
            .map(|super_box| super_box.input().trim().to_owned())
            .filter(|command| !command.is_empty())
        else {
            return Task::none();
        };
        let history = self
            .super_box
            .as_ref()
            .map(|super_box| super_box.history_text())
            .unwrap_or_default();
        self.super_box = None;
        let opened = if self.terminal_panel.is_expanded() {
            Task::none()
        } else {
            self.toggle_terminal_panel()
        };
        let Some(tab) = self.terminal_panel.active_tab() else {
            return opened;
        };
        tab.session.write_input(format!("{command}\n").as_bytes());
        Task::batch([opened, self.remember_super_box_input(history)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_result_survives_the_state_database_with_its_end_only() {
        let long = format!("{}末尾", "前".repeat(10_000));
        let run = PastRun {
            command: "cat big".to_owned(),
            at_ms: 42,
            end: RunEnd::Finished(ShownOutput::from_text(long.clone())),
        };
        let record = stored(&run);
        let StoredCommandResult::Finished { output } = &record.result else {
            panic!("finished");
        };
        assert!(output.len() <= STORED_OUTPUT_BYTES);
        assert!(output.ends_with("末尾") && long.ends_with(output.as_str()));
        let back = from_stored(record);
        assert_eq!((back.command.as_str(), back.at_ms), ("cat big", 42));

        let failed = PastRun {
            command: "make".to_owned(),
            at_ms: 1,
            end: RunEnd::Failed("exit 2".to_owned()),
        };
        assert_eq!(from_stored(stored(&failed)).end, failed.end);
    }
}
