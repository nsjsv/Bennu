//! 不显示的标签：超级输入框执行的命令。命令在自己的伪终端里跑——程序看到的是终端，输出带颜色、
//! 会分页、能问密码——只是不画出来；要用户来操作时原样移进标签条，接着跑，不重跑。输出订阅按会话号
//! 走，移过去时输出不断、不重读。

use file_operations::CommandInvocation;

use super::emulator::TerminalDimensions;
use super::session::{spawn_terminal_session, SessionId, TerminalSession};
use super::tabs::{TabProgram, TerminalTab};
use crate::app::FileBrowser;

impl FileBrowser {
    /// 起一个不显示的标签跑 `program`；尺寸由显示它输出的地方定，移进标签条时再换成面板的。
    pub(crate) fn spawn_hidden_terminal_tab(
        &mut self,
        program: &CommandInvocation,
        dimensions: TerminalDimensions,
    ) -> Result<SessionId, String> {
        let session_id = self.terminal_panel.take_session_id();
        let session = spawn_terminal_session(
            session_id,
            program,
            dimensions,
            self.terminal_panel.events.sender(),
        )?;
        self.terminal_panel.hidden_tabs.push(TerminalTab {
            session,
            directory: program.working_directory.clone(),
            has_unread_output: false,
            program: TabProgram::Command,
        });
        Ok(session_id)
    }

    pub(crate) fn hidden_terminal_session(
        &self,
        session_id: SessionId,
    ) -> Option<&TerminalSession> {
        self.terminal_panel
            .hidden_tabs
            .iter()
            .find(|tab| tab.session.id == session_id)
            .map(|tab| &tab.session)
    }

    /// 移进标签条并激活；面板收着就展开；键盘给它。已经结束（不在了）时返回 `false`。
    pub(crate) fn show_hidden_terminal_tab(&mut self, session_id: SessionId) -> bool {
        let Some(index) = self
            .terminal_panel
            .hidden_tabs
            .iter()
            .position(|tab| tab.session.id == session_id)
        else {
            return false;
        };
        let tab = self.terminal_panel.hidden_tabs.remove(index);
        self.terminal_panel.tabs.push(tab);
        self.terminal_panel.active_session_id = Some(session_id);
        self.terminal_panel.focused = true;
        if !self.terminal_panel.expanded {
            let target = self.terminal_panel.clamp_height_for_window(
                self.terminal_panel.expanded_height,
                self.main_window_height,
            );
            self.terminal_panel.expanded_height = target;
            self.terminal_panel.expanded = true;
            self.terminal_panel.start_height_animation(target);
        }
        self.sync_terminal_panel_size();
        true
    }

    /// 输出读到头了：拿走它的会话，交给超级输入框等它退出。
    pub(super) fn take_hidden_terminal_session(
        &mut self,
        session_id: SessionId,
    ) -> Option<TerminalSession> {
        let index = self
            .terminal_panel
            .hidden_tabs
            .iter()
            .position(|tab| tab.session.id == session_id)?;
        Some(self.terminal_panel.hidden_tabs.remove(index).session)
    }
}
