//! 超级输入框执行命令：交给用户的 shell，在一个不显示的终端标签里跑——程序看到的是终端，所以全屏
//! 界面、分页、问密码都照常。框里边跑边显示最后几行，跑完显示退出情况和输出的末尾。程序打开全屏
//! 界面、或者用户点“转到终端”时，把它原样移进底部终端面板接着用。Ctrl+Enter 改成打进终端面板的
//! shell。每条命令最后一次的结果记进状态库，重启后在命令模式的历史里还能看。
//!
//! 哪些会话是框发的命令由这里记（`SuperBoxCommands`），不靠框：命令跑完时框可能已经关了，
//! 结果照样要记下来。

use std::collections::HashMap;
use std::time::Duration;

use bennu_state::{CommandOutputStore, StoredCommandOutput, StoredCommandResult, StoredExit};
use iced::Task;

use super::super_box::SuperBoxMessage;
use super::FileBrowser;
use crate::model::Message;
use crate::super_box::{
    shell_invocation, Exit, PastRun, RunEnd, RunState, ShownOutput, SuperBox, PAST_RUNS_KEPT,
    SHOWN_OUTPUT_LINES,
};
use crate::terminal_panel::session::{SessionId, TerminalSession};

/// 存进状态库的输出最多这么多（末尾）。
const STORED_OUTPUT_BYTES: usize = 16 * 1024;
/// 命令的终端有几行：输出在框里看，多少行都行，取最常见的终端高度。
const COMMAND_ROWS: usize = 24;
/// 停止时先 SIGTERM 让它自己收拾；过这么久还没结束就 SIGKILL 整组。
const STOP_GRACE: Duration = Duration::from_secs(2);

/// 框发出去、还没结束的命令（会话号 → 命令），和最近几条命令的结果（新的在前）。转到终端的命令
/// 不在这里：之后它就是一个终端标签，输出在终端里。
#[derive(Debug, Default)]
pub(crate) struct SuperBoxCommands {
    running: HashMap<SessionId, String>,
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
        RunEnd::Exited { exit, output } => StoredCommandResult::Exited {
            exit: match exit {
                Exit::Code(code) => StoredExit::Code(*code),
                Exit::Signal(name) => StoredExit::Signal(name.clone()),
            },
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
            StoredCommandResult::Exited { exit, output } => RunEnd::Exited {
                exit: match exit {
                    StoredExit::Code(code) => Exit::Code(code),
                    StoredExit::Signal(name) => Exit::Signal(name),
                },
                output: ShownOutput::from_text(output),
            },
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

fn exit_of(status: &portable_pty::ExitStatus) -> Exit {
    match status.signal() {
        Some(name) => Exit::Signal(name.to_owned()),
        None => Exit::Code(status.exit_code()),
    }
}

impl FileBrowser {
    /// 回车：在不显示的终端标签里跑，框里显示“正在执行”。同一个框一次只跑一条。
    pub(super) fn run_super_box_command(&mut self, command: String) -> Task<Message> {
        let Some(super_box) = self.super_box.as_ref() else {
            return Task::none();
        };
        let running = super_box
            .run()
            .is_some_and(|run| run.state == RunState::Running);
        let Some(folder) = self.keyboard_target_directory().filter(|_| !running) else {
            return Task::none();
        };
        let (generation, history) = (super_box.generation(), super_box.history_text());
        let invocation = shell_invocation(
            std::path::Path::new(&self.terminal_panel_shell()),
            &command,
            &self.selected_paths_for_operation(),
            &folder,
        );
        let size = crate::terminal_panel::emulator_dimensions(
            crate::view::super_box_output_columns(),
            COMMAND_ROWS,
        );
        let started = self.spawn_hidden_terminal_tab(&invocation, size);
        let Some(super_box) = self.super_box.as_mut() else {
            return Task::none();
        };
        match started {
            Ok(session) => {
                self.super_box_commands
                    .running
                    .insert(session, command.clone());
                super_box.start_run(session, command);
                self.remember_super_box_input(history)
            }
            Err(reason) => {
                super_box.show_notice(generation, reason);
                Task::none()
            }
        }
    }

    /// 框里正在跑的那条命令的会话。
    fn running_super_box_command(&self) -> Option<SessionId> {
        self.super_box
            .as_ref()?
            .run()
            .filter(|run| run.state == RunState::Running)
            .map(|run| run.session)
    }

    /// 停止按钮：整组进程先 SIGTERM，过一会儿还在就 SIGKILL（`StopGraceEnded`）。
    pub(super) fn stop_super_box_command(&mut self) -> Task<Message> {
        let Some(session) = self.running_super_box_command() else {
            return Task::none();
        };
        let Some(running) = self.hidden_terminal_session(session) else {
            return Task::none();
        };
        running.signal(libc::SIGTERM);
        Task::perform(tokio::time::sleep(STOP_GRACE), move |()| {
            Message::SuperBox(SuperBoxMessage::StopGraceEnded(session))
        })
    }

    /// 停止之后等够了：已经结束、转到终端了就不用管。
    pub(super) fn kill_super_box_command(&self, session: SessionId) {
        if let Some(running) = self.hidden_terminal_session(session) {
            running.signal(libc::SIGKILL);
        }
    }

    /// “转到终端”按钮。
    pub(super) fn show_running_super_box_command(&mut self) {
        if let Some(session) = self.running_super_box_command() {
            self.show_super_box_command(session);
        }
    }

    /// 命令要用户来操作（打开了全屏界面，或者点了“转到终端”）：移进终端面板，键盘给它；框在显示
    /// 这条命令就关掉。之后它就是一个终端标签，结果不再记。
    pub(crate) fn show_super_box_command(&mut self, session: SessionId) {
        if !self.show_hidden_terminal_tab(session) {
            return;
        }
        self.super_box_commands.running.remove(&session);
        if self
            .super_box
            .as_ref()
            .and_then(SuperBox::run)
            .is_some_and(|run| run.session == session)
        {
            self.super_box = None;
        }
    }

    /// 进程收了（读线程等到了它的退出）：取下输出的末尾，连同退出情况交回去。
    pub(crate) fn super_box_command_ended(
        &mut self,
        ended: TerminalSession,
        end: Result<portable_pty::ExitStatus, String>,
    ) -> Task<Message> {
        let output = ShownOutput::from_text(ended.emulator.last_lines(SHOWN_OUTPUT_LINES));
        let end = match end {
            Ok(status) => RunEnd::Exited {
                exit: exit_of(&status),
                output,
            },
            Err(reason) => RunEnd::Failed(reason),
        };
        self.accept_super_box_command_end(ended.id, end)
    }

    /// 命令退出了：记下结果（框关了也记），框还开着就交给框。
    fn accept_super_box_command_end(&mut self, session: SessionId, end: RunEnd) -> Task<Message> {
        let Some(command) = self.super_box_commands.running.remove(&session) else {
            return Task::none();
        };
        if let Some(super_box) = self.super_box.as_mut() {
            super_box.finish_run(session, end.clone());
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
            end: RunEnd::Exited {
                exit: Exit::Code(0),
                output: ShownOutput::from_text(long.clone()),
            },
        };
        let record = stored(&run);
        let StoredCommandResult::Exited { output, .. } = &record.result else {
            panic!("exited");
        };
        assert!(output.len() <= STORED_OUTPUT_BYTES);
        assert!(output.ends_with("末尾") && long.ends_with(output.as_str()));
        let back = from_stored(record);
        assert_eq!((back.command.as_str(), back.at_ms), ("cat big", 42));

        for end in [
            RunEnd::Exited {
                exit: Exit::Signal("Terminated".to_owned()),
                output: ShownOutput::from_text("half".to_owned()),
            },
            RunEnd::Failed("no status".to_owned()),
        ] {
            let run = PastRun {
                command: "make".to_owned(),
                at_ms: 1,
                end,
            };
            assert_eq!(from_stored(stored(&run)).end, run.end);
        }
    }
}
