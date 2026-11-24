//! 停靠区「显示哪个条目」的唯一所有者：用户开了「显示鼠标悬停的项目」后，指针在某个条目上
//! 停稳，信息面板和预览面板一起显示它。两个面板读同一份状态（[`FileBrowser::dock_item`]），
//! 不会一个看悬停项、一个看选中项。追踪在每次 `update` 出口、两个面板同步之前跑一次。

use std::path::{Path, PathBuf};
use std::time::Duration;

use iced::Task;

use super::FileBrowser;
use crate::model::Message;
use crate::right_dock::DockPanel;

/// 指针在一个条目上停这么久才换成显示它：划过一串文件时不挨个读、挨个跑 ffprobe。
const HOVER_SETTLE_DELAY: Duration = Duration::from_millis(300);

/// 指针离开所有条目这么久才从悬停的条目回到选中项：从一个条目挪到下一个要经过行间空隙，
/// 一离开就切回去，面板会在悬停项和选中项之间来回闪。
const HOVER_RELEASE_DELAY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub(crate) enum DockHoverMessage {
    Settled { generation: u64 },
    Released { generation: u64 },
    ShowHoveredItemToggled,
}

/// 在等的那个定时。同一时刻只有一个：新的悬停事件换掉旧的，旧的到点时号对不上就作废。
#[derive(Debug, Clone, PartialEq)]
enum HoverTimer {
    /// 指针停在这个条目上，等够了就显示它。
    Settling { path: PathBuf, generation: u64 },
    /// 指针已离开所有条目，等够了就回到选中项。
    Releasing { generation: u64 },
}

#[derive(Debug, Default)]
pub(crate) struct DockHoverState {
    timer: Option<HoverTimer>,
    generation: u64,
    /// 等够了的悬停条目：面板正显示它，直到指针离开所有条目满 [`HOVER_RELEASE_DELAY`]。
    settled: Option<PathBuf>,
}

impl DockHoverState {
    /// 不跟悬停了：已显示的悬停条目也放掉，回到选中项。
    fn forget(&mut self) {
        self.timer = None;
        self.settled = None;
    }

    fn next_generation(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }
}

impl FileBrowser {
    pub(super) fn handle_dock_hover_message(&mut self, message: DockHoverMessage) -> Task<Message> {
        match message {
            DockHoverMessage::Settled { generation } => {
                if let Some(HoverTimer::Settling {
                    path,
                    generation: armed,
                }) = &self.dock_hover.timer
                {
                    if *armed == generation {
                        self.dock_hover.settled = Some(path.clone());
                        self.dock_hover.timer = None;
                    }
                }
            }
            DockHoverMessage::Released { generation } => {
                if self.dock_hover.timer == Some(HoverTimer::Releasing { generation }) {
                    self.dock_hover.forget();
                }
            }
            DockHoverMessage::ShowHoveredItemToggled => {
                self.context_menu = None;
                self.user_config.dock_shows_hovered_item =
                    !self.user_config.dock_shows_hovered_item;
                return self.persist_user_preferences_command();
            }
        }
        Task::none()
    }

    /// 两个面板显示的那个条目：停稳的悬停项，没有就是焦点选中项。
    pub(super) fn dock_item(&self) -> Option<&Path> {
        self.dock_hovered_item().or(self.tab().selected.as_deref())
    }

    /// 停稳的悬停项。没开悬停、面板没显示、独立预览窗口开着时 `sync_dock_hover` 已把它放掉。
    pub(super) fn dock_hovered_item(&self) -> Option<&Path> {
        self.dock_hover.settled.as_deref()
    }

    /// 悬停开着时：指针在某个条目上就等它停稳再显示；离开条目后继续显示最后停稳的那个，
    /// 离开所有条目满 [`HOVER_RELEASE_DELAY`] 才回到选中项。
    pub(super) fn sync_dock_hover(&mut self) -> Task<Message> {
        if !self.dock_follows_hover() {
            self.dock_hover.forget();
            return Task::none();
        }
        let state = &mut self.dock_hover;
        let Some(path) = &self.hovered_entry else {
            if state.settled.is_none() {
                // 没停稳就离开了：不显示它，也没有要回到的。
                state.timer = None;
                return Task::none();
            }
            if matches!(state.timer, Some(HoverTimer::Releasing { .. })) {
                return Task::none();
            }
            let generation = state.next_generation();
            state.timer = Some(HoverTimer::Releasing { generation });
            return timer_task(
                HOVER_RELEASE_DELAY,
                DockHoverMessage::Released { generation },
            );
        };
        if state.settled.as_ref() == Some(path) {
            // 回到正显示的条目上：离开的、去别处的定时都作废。
            state.timer = None;
            return Task::none();
        }
        let waiting = matches!(
            &state.timer,
            Some(HoverTimer::Settling { path: waiting, .. }) if waiting == path
        );
        if waiting {
            return Task::none();
        }
        let generation = state.next_generation();
        state.timer = Some(HoverTimer::Settling {
            path: path.clone(),
            generation,
        });
        timer_task(HOVER_SETTLE_DELAY, DockHoverMessage::Settled { generation })
    }

    /// 跟不跟悬停是整个停靠区一处决定的，两个面板不各自判断。
    fn dock_follows_hover(&self) -> bool {
        let layout = self.right_dock.layout();
        self.user_config.dock_shows_hovered_item
            && (layout.shows(DockPanel::Info) || layout.shows(DockPanel::Preview))
            // 窗口开着时预览归窗口，悬停换目标会把窗口里的内容一起换掉。
            && !self.preview_session_held_by_window()
    }
}

/// 定时器在任务跑起来时才建：建它要在 tokio 运行时里。
fn timer_task(delay: Duration, elapsed: DockHoverMessage) -> Task<Message> {
    Task::perform(async move { tokio::time::sleep(delay).await }, move |()| {
        Message::DockHover(elapsed)
    })
}

#[cfg(test)]
#[path = "dock_hover_tests.rs"]
mod tests;
