//! 超级输入框执行命令：交给用户的 shell，进任务队列（能暂停、取消），框里等结果；Ctrl+Enter 改成
//! 送进底部终端面板。

use std::path::PathBuf;

use iced::Task;

use super::FileBrowser;
use crate::model::Message;
use crate::super_box::{shell_invocation, RunState};
use file_operations::{FileOperationCompletion, FileOperationOutcome, QueuedFileOperation};

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
    pub(super) fn super_box_task_ended(
        &mut self,
        task_id: u64,
        completion: &FileOperationCompletion,
    ) {
        let Some(super_box) = self.super_box.as_mut() else {
            return;
        };
        let state = match completion {
            FileOperationCompletion::Succeeded(FileOperationOutcome::CommandsRan { output }) => {
                RunState::finished(output)
            }
            FileOperationCompletion::Failed { error, .. }
            | FileOperationCompletion::RecoveryBlocked { error, .. }
            | FileOperationCompletion::RecoveryInterrupted(error, _) => {
                RunState::Failed(error.clone())
            }
            FileOperationCompletion::Canceled(_) => RunState::Failed(tr!("Stopped")),
            // 别的任务（不是命令）结束了：`finish_run` 也只认自己的任务号，这里不用分辨。
            FileOperationCompletion::Succeeded(_)
            | FileOperationCompletion::RenameRejected { .. } => return,
        };
        super_box.finish_run(task_id, state);
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
