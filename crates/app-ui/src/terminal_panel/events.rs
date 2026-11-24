//! 会话的事件进界面：所有会话共用一条通道、一个常驻订阅；同一会话的屏幕变化按批合并、
//! 两次之间至少隔 [`SCREEN_UPDATE_INTERVAL`]，大量输出时不按读到的每一块重建界面。

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use alacritty_terminal::event::{Event, WindowSize};
use alacritty_terminal::term::ClipboardType;
use iced::Task;
use tokio::sync::mpsc::UnboundedReceiver;

use super::session::{SessionEvent, SessionEventSender, SessionId};
use super::{palette, view, TerminalPanelMessage};
use crate::app::FileBrowser;
use crate::model::Message;

/// 屏幕变化通知之间至少隔这么久：界面最多每秒重建约 120 次。
const SCREEN_UPDATE_INTERVAL: Duration = Duration::from_millis(8);

/// 所有会话的事件走同一条通道，界面只有一个常驻订阅收它
/// （[`FileBrowser::terminal_events_subscription`]）。
pub(crate) struct SessionEvents {
    sender: SessionEventSender,
    receiver: EventsReceiver,
}

impl SessionEvents {
    pub(super) fn new() -> Self {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        Self {
            sender,
            receiver: EventsReceiver(Arc::new(Mutex::new(Some(receiver)))),
        }
    }

    pub(crate) fn sender(&self) -> &SessionEventSender {
        &self.sender
    }

    /// 测试里没有订阅：直接拿这条流把事件交给浏览器。
    #[cfg(test)]
    pub(crate) fn stream(&self) -> impl iced::futures::Stream<Item = Message> + 'static {
        self.receiver.stream()
    }
}

/// 订阅第一次启动时取走接收端。整个应用只有这一个订阅，哈希不随内容变，iced 不会重建它。
#[derive(Clone)]
struct EventsReceiver(Arc<Mutex<Option<UnboundedReceiver<(SessionId, SessionEvent)>>>>);

impl Hash for EventsReceiver {
    fn hash<H: Hasher>(&self, _state: &mut H) {}
}

impl EventsReceiver {
    fn stream(&self) -> impl iced::futures::Stream<Item = Message> + 'static {
        let receiver = self.0.lock().ok().and_then(|mut slot| slot.take());
        iced::futures::stream::unfold((receiver, false), |(receiver, screen_changed)| async move {
            let mut receiver = receiver?;
            if screen_changed {
                tokio::time::sleep(SCREEN_UPDATE_INTERVAL).await;
            }
            let mut batch = vec![receiver.recv().await?];
            while let Ok(next) = receiver.try_recv() {
                batch.push(next);
            }
            keep_last_wakeup_per_session(&mut batch);
            let screen_changed = batch
                .iter()
                .any(|(_, event)| matches!(event, SessionEvent::Wakeup));
            let message = Message::TerminalPanel(TerminalPanelMessage::SessionEvents(batch));
            Some((message, (Some(receiver), screen_changed)))
        })
    }
}

/// 一批事里同一会话的屏幕变化只留最后一个；别的事原样按先后。
fn keep_last_wakeup_per_session(batch: &mut Vec<(SessionId, SessionEvent)>) {
    let mut woken = HashSet::new();
    let keep: Vec<bool> = batch
        .iter()
        .rev()
        .map(|(session, event)| !matches!(event, SessionEvent::Wakeup) || woken.insert(*session))
        .collect();
    let mut keep = keep.into_iter().rev();
    batch.retain(|_| keep.next().unwrap_or(true));
}

impl FileBrowser {
    /// 所有终端会话的事件：一个常驻订阅（见 [`SessionEvents`]）。
    pub(crate) fn terminal_events_subscription(&self) -> iced::Subscription<Message> {
        iced::Subscription::run_with(
            self.terminal_panel.events.receiver.clone(),
            EventsReceiver::stream,
        )
    }

    pub(super) fn accept_session_event(
        &mut self,
        session: SessionId,
        event: SessionEvent,
    ) -> Task<Message> {
        match event {
            SessionEvent::Wakeup => self.terminal_screen_changed(session),
            SessionEvent::DirectoryChanged(directory) => {
                return self.accept_shell_directory(session, directory);
            }
            SessionEvent::Request(request) => {
                return self.answer_terminal_request(session, request)
            }
            SessionEvent::Ended(end) => return self.terminal_session_ended(session, end),
        }
        Task::none()
    }

    /// 后台标签亮未读圆点；超级输入框的命令打开了全屏界面就转进标签条让用户操作。
    fn terminal_screen_changed(&mut self, session: SessionId) {
        let active_session_id = self.terminal_panel.active_session_id;
        if let Some(tab) = self
            .terminal_panel
            .tabs
            .iter_mut()
            .find(|tab| tab.session.id == session)
        {
            if active_session_id != Some(session) {
                tab.has_unread_output = true;
            }
        } else if self
            .hidden_terminal_session(session)
            .is_some_and(|hidden| hidden.emulator.is_full_screen())
        {
            self.show_super_box_command(session);
        }
    }

    /// 进程收了：超级输入框的命令把结果交回去；终端标签正常退出就关掉，断开的留着
    /// "已断开"提示和已有输出，由用户自己关。
    fn terminal_session_ended(
        &mut self,
        session: SessionId,
        end: Result<portable_pty::ExitStatus, String>,
    ) -> Task<Message> {
        if let Some(mut ended) = self.take_hidden_terminal_session(session) {
            ended.process_ended();
            return self.super_box_command_ended(ended, end);
        }
        let Some(tab) = self
            .terminal_panel
            .tabs
            .iter_mut()
            .find(|tab| tab.session.id == session)
        else {
            return Task::none();
        };
        tab.session.process_ended();
        if tab.session.disconnected().is_some() {
            return Task::none();
        }
        self.close_terminal_tab(session)
    }

    /// 程序问颜色、问屏幕像素大小，或者要写剪贴板（OSC 52；只许写，不许读）。
    fn answer_terminal_request(&mut self, session: SessionId, request: Event) -> Task<Message> {
        let theme = self.theme(self.main_window_id());
        let Some(terminal) = self
            .terminal_panel
            .tabs
            .iter()
            .chain(&self.terminal_panel.hidden_tabs)
            .map(|tab| &tab.session)
            .find(|terminal| terminal.id == session)
        else {
            return Task::none();
        };
        match request {
            Event::ColorRequest(index, format) => {
                let color =
                    palette::color_for_index(index, terminal.emulator.lock().colors(), &theme);
                terminal.write_input(format(color).as_bytes());
            }
            Event::TextAreaSizeRequest(format) => {
                let dimensions = terminal.emulator.dimensions();
                let size = WindowSize {
                    num_lines: dimensions.screen_lines as u16,
                    num_cols: dimensions.columns as u16,
                    cell_width: view::cell_width().round() as u16,
                    cell_height: view::CELL_HEIGHT.round() as u16,
                };
                terminal.write_input(format(size).as_bytes());
            }
            Event::ClipboardStore(ClipboardType::Clipboard, text) => {
                return iced::clipboard::write(text);
            }
            Event::ClipboardStore(ClipboardType::Selection, text) => {
                return iced::clipboard::write_primary(text);
            }
            _ => {}
        }
        Task::none()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use super::*;
    use crate::terminal_panel::tabs::{TabProgram, TerminalTab};
    use crate::terminal_panel::{emulator_dimensions, session};

    #[test]
    fn a_batch_keeps_only_the_last_screen_change_of_each_session() {
        let mut batch = vec![
            (1, SessionEvent::Wakeup),
            (2, SessionEvent::Wakeup),
            (1, SessionEvent::DirectoryChanged(PathBuf::from("/tmp"))),
            (1, SessionEvent::Wakeup),
            (1, SessionEvent::Ended(Err("gone".to_owned()))),
        ];
        keep_last_wakeup_per_session(&mut batch);
        let kept: Vec<(SessionId, &str)> = batch
            .iter()
            .map(|(session, event)| {
                let kind = match event {
                    SessionEvent::Wakeup => "wakeup",
                    SessionEvent::DirectoryChanged(_) => "directory",
                    SessionEvent::Request(_) => "request",
                    SessionEvent::Ended(_) => "ended",
                };
                (*session, kind)
            })
            .collect();
        assert_eq!(
            kept,
            vec![(2, "wakeup"), (1, "directory"), (1, "wakeup"), (1, "ended")]
        );
    }

    #[tokio::test]
    async fn an_exited_tab_closes_but_a_disconnected_one_stays_with_its_output() {
        use iced::futures::StreamExt;

        let folder = tempfile::tempdir().unwrap();
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        for id in [1, 2] {
            let session = session::spawn_terminal_session(
                id,
                &file_operations::CommandInvocation {
                    program: "/bin/sh".into(),
                    arguments: vec!["-c".into(), "exit 0".into()],
                    working_directory: folder.path().to_path_buf(),
                },
                emulator_dimensions(80, 24),
                browser.terminal_panel.events.sender(),
            )
            .unwrap();
            browser.terminal_panel.tabs.push(TerminalTab {
                session,
                directory: folder.path().to_path_buf(),
                has_unread_output: false,
                program: TabProgram::Command,
            });
        }
        browser.terminal_panel.tabs[1]
            .session
            .disconnect_for_test("input/output error");

        let mut events = Box::pin(browser.terminal_panel.events.stream());
        let mut ended = 0;
        while ended < 2 {
            let message = tokio::time::timeout(Duration::from_secs(20), events.next())
                .await
                .expect("both shells end")
                .expect("the event stream stays open");
            let Message::TerminalPanel(message) = message else {
                continue;
            };
            if let TerminalPanelMessage::SessionEvents(batch) = &message {
                ended += batch
                    .iter()
                    .filter(|(_, event)| matches!(event, SessionEvent::Ended(_)))
                    .count();
            }
            drop(browser.handle_terminal_panel_message(message));
        }

        let remaining: Vec<SessionId> = browser
            .terminal_panel
            .tabs
            .iter()
            .map(|tab| tab.session.id)
            .collect();
        assert_eq!(remaining, vec![2]);
    }
}
