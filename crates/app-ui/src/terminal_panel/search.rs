//! 回看搜索：按字面找（不是正则），全小写时不分大小写、有大写就分（alacritty 的做法），在整个
//! 回看里找。改字、按上一个/下一个时才算——新输出不自动重算，大量输出时不会一直全文搜。
//! 匹配按网格的绝对行记，回看滚动后照样对得上。

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Direction, Point};
use alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};
use alacritty_terminal::term::Term;

use iced::widget::{container, row, text};
use iced::{Alignment, Element, Length, Task};

use super::session::EventProxy;
use super::TerminalPanelMessage;
use crate::app::FileBrowser;
use crate::model::Message;
use bennu_theme::controls::{icon_button, on_escape, text_field};
use bennu_theme::icons::IconSymbol;
use bennu_theme::typography::CAPTION_SIZE;

/// 最多记这么多个匹配：回看一万行里搜一个字母时不至于攒出几十万个。
const MAX_MATCHES: usize = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchStep {
    /// 往上，更早的输出。
    Older,
    /// 往下，更新的输出。
    Newer,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TerminalSearch {
    query: String,
    matches: Vec<Match>,
    current: Option<usize>,
}

impl TerminalSearch {
    pub(crate) fn query(&self) -> &str {
        &self.query
    }

    pub(crate) fn matches(&self) -> &[Match] {
        &self.matches
    }

    pub(crate) fn current(&self) -> Option<&Match> {
        self.matches.get(self.current?)
    }

    /// 「第几个 / 共几个」，从上往下数。
    pub(crate) fn position(&self) -> Option<(usize, usize)> {
        Some((self.current? + 1, self.matches.len()))
    }

    /// 改了要找的字：重新找，当前那个是最下面（最新）的，滚到它露出来。
    pub(crate) fn set_query(&mut self, query: String, term: &mut Term<EventProxy>) {
        self.query = query;
        self.matches = find_all(&self.query, term);
        self.current = self.matches.len().checked_sub(1);
        self.reveal_current(term);
    }

    /// 往上或往下一个（到头绕回另一头）。先按现在的屏幕重找一遍：这期间可能又有了输出。
    pub(crate) fn step(&mut self, step: SearchStep, term: &mut Term<EventProxy>) {
        let previous = self.current().map(|found| *found.start());
        self.matches = find_all(&self.query, term);
        let last = self.matches.len().checked_sub(1);
        self.current = match (previous, step, last) {
            (_, _, None) => None,
            (None, _, last) => last,
            (Some(previous), SearchStep::Older, last) => self
                .matches
                .iter()
                .rposition(|found| *found.start() < previous)
                .or(last),
            (Some(previous), SearchStep::Newer, _) => Some(
                self.matches
                    .iter()
                    .position(|found| *found.start() > previous)
                    .unwrap_or(0),
            ),
        };
        self.reveal_current(term);
    }

    fn reveal_current(&self, term: &mut Term<EventProxy>) {
        if let Some(found) = self.current() {
            term.scroll_to_point(*found.start());
        }
    }
}

pub(crate) fn search_input_id() -> iced::widget::Id {
    iced::widget::Id::new("terminal-search")
}

const SEARCH_FIELD_WIDTH: f32 = 200.0;
const SEARCH_ICON_SIZE: f32 = 14.0;

impl FileBrowser {
    /// Ctrl+Shift+F：给激活的标签打开搜索条（开着就把焦点放回输入框）。
    pub(crate) fn open_terminal_search(&mut self) -> Task<Message> {
        let Some(session) = self.terminal_panel.active_session_id else {
            return Task::none();
        };
        if self.terminal_panel.search.as_ref().map(|(owner, _)| *owner) != Some(session) {
            self.terminal_panel.search = Some((session, TerminalSearch::default()));
        }
        iced::widget::operation::focus(search_input_id())
    }

    pub(super) fn change_terminal_search(&mut self, query: String) {
        let panel = &mut self.terminal_panel;
        let Some((owner, search)) = panel.search.as_mut() else {
            return;
        };
        if let Some(tab) = panel.tabs.iter().find(|tab| tab.session.id == *owner) {
            search.set_query(query, &mut tab.session.emulator.lock());
        }
    }

    pub(super) fn step_terminal_search(&mut self, step: SearchStep) {
        let panel = &mut self.terminal_panel;
        let Some((owner, search)) = panel.search.as_mut() else {
            return;
        };
        if let Some(tab) = panel.tabs.iter().find(|tab| tab.session.id == *owner) {
            search.step(step, &mut tab.session.emulator.lock());
        }
    }
}

/// 网格右上角的搜索条：输入框（回车往上、Shift+回车往下、Esc 关）、第几个/共几个、上下、关闭。
pub(super) fn search_bar(search: &TerminalSearch) -> Element<'_, Message> {
    let field = text_field(&tr!("Find in output"), search.query())
        .id(search_input_id())
        .on_input(|query| Message::TerminalPanel(TerminalPanelMessage::SearchChanged(query)))
        .on_submit(Message::TerminalPanel(
            TerminalPanelMessage::SearchSubmitted,
        ))
        .width(Length::Fixed(SEARCH_FIELD_WIDTH));
    let count = match search.position() {
        Some((current, total)) => tr!("%1 of %2", current, total),
        None if search.query().is_empty() => String::new(),
        None => tr!("No matches"),
    };
    let button = |icon: IconSymbol, message: TerminalPanelMessage| {
        icon_button(
            icon.view(SEARCH_ICON_SIZE)
                .width(Length::Fixed(SEARCH_ICON_SIZE))
                .height(Length::Fixed(SEARCH_ICON_SIZE))
                .style(crate::appearance::icon_svg_style()),
        )
        .on_press(Message::TerminalPanel(message))
    };
    container(
        row![
            on_escape(
                field,
                Message::TerminalPanel(TerminalPanelMessage::SearchClosed)
            ),
            text(count).size(CAPTION_SIZE),
            button(
                IconSymbol::ArrowUp,
                TerminalPanelMessage::SearchStepped(SearchStep::Older)
            ),
            button(
                IconSymbol::ArrowDown,
                TerminalPanelMessage::SearchStepped(SearchStep::Newer)
            ),
            button(IconSymbol::Close, TerminalPanelMessage::SearchClosed),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .padding(6)
    .style(crate::appearance::context_menu_style)
    .into()
}

fn find_all(query: &str, term: &Term<EventProxy>) -> Vec<Match> {
    if query.is_empty() {
        return Vec::new();
    }
    let Ok(mut regex) = RegexSearch::new(&literal_pattern(query)) else {
        return Vec::new();
    };
    let start = Point::new(term.topmost_line(), Column(0));
    let end = Point::new(term.bottommost_line(), term.last_column());
    RegexIter::new(start, end, Direction::Right, term, &mut regex)
        .take(MAX_MATCHES)
        .collect()
}

/// 把要找的字转成只认字面的正则（正则的特殊字符都加反斜杠）。
fn literal_pattern(query: &str) -> String {
    let mut pattern = String::with_capacity(query.len() * 2);
    for character in query.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(character) {
            pattern.push('\\');
        }
        pattern.push(character);
    }
    pattern
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_panel::emulator::tests::{emulator, feed};

    #[test]
    fn it_finds_literally_and_starts_from_the_newest() {
        let emulator = emulator(20, 3);
        // 比屏幕多两行，前面的进了回看。
        feed(&emulator, b"a.c one\r\nabc\r\na.c two\r\nx\r\na.c three");
        let mut term = emulator.lock();
        let mut search = TerminalSearch::default();

        search.set_query("a.c".to_owned(), &mut term);

        assert_eq!(search.matches().len(), 3, "`.` is a dot, not any character");
        assert_eq!(search.position(), Some((3, 3)));
    }

    #[test]
    fn stepping_goes_up_and_down_and_wraps_around() {
        let emulator = emulator(20, 5);
        feed(&emulator, b"hit 1\r\nhit 2\r\nhit 3");
        let mut term = emulator.lock();
        let mut search = TerminalSearch::default();
        search.set_query("hit".to_owned(), &mut term);

        search.step(SearchStep::Older, &mut term);
        assert_eq!(search.position(), Some((2, 3)));
        search.step(SearchStep::Older, &mut term);
        search.step(SearchStep::Older, &mut term);
        assert_eq!(search.position(), Some((3, 3)), "wraps to the newest");
        search.step(SearchStep::Newer, &mut term);
        assert_eq!(search.position(), Some((1, 3)), "wraps to the oldest");
    }

    #[test]
    fn lower_case_ignores_case_and_upper_case_does_not() {
        let emulator = emulator(20, 3);
        feed(&emulator, b"Error error");
        let mut term = emulator.lock();
        let mut search = TerminalSearch::default();
        search.set_query("error".to_owned(), &mut term);
        assert_eq!(search.matches().len(), 2);
        search.set_query("Error".to_owned(), &mut term);
        assert_eq!(search.matches().len(), 1);
    }

    #[test]
    fn while_searching_the_keys_go_to_the_search_and_clicking_the_grid_gives_them_back() {
        let folder = tempfile::tempdir().unwrap();
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
        let session = crate::terminal_panel::session::spawn_terminal_session(
            3,
            &file_operations::CommandInvocation {
                program: "/bin/sh".into(),
                arguments: vec!["-c".into(), "printf 'hit here'; read x".into()],
                working_directory: folder.path().to_path_buf(),
            },
            crate::terminal_panel::emulator_dimensions(40, 5),
            browser.terminal_panel.events.sender(),
        )
        .unwrap();
        browser
            .terminal_panel
            .tabs
            .push(crate::terminal_panel::tabs::TerminalTab {
                session,
                directory: folder.path().to_path_buf(),
                has_unread_output: false,
                program: crate::terminal_panel::tabs::TabProgram::Command,
            });
        browser.terminal_panel.active_session_id = Some(3);
        browser.terminal_panel.expand_without_shell();
        browser.terminal_panel.focused = true;
        for _ in 0..800 {
            if browser.terminal_panel.tabs[0]
                .session
                .emulator
                .last_lines(1)
                == "hit here"
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(browser.terminal_has_keyboard());

        drop(browser.open_terminal_search());
        drop(
            browser.handle_terminal_panel_message(TerminalPanelMessage::SearchChanged(
                "hit".to_owned(),
            )),
        );

        assert!(!browser.terminal_has_keyboard());
        let search = &browser.terminal_panel.search.as_ref().unwrap().1;
        assert_eq!(search.position(), Some((1, 1)));

        drop(browser.handle_terminal_panel_message(TerminalPanelMessage::GridPressed));
        assert!(browser.terminal_panel.search.is_none());
        assert!(browser.terminal_has_keyboard());
    }
}
