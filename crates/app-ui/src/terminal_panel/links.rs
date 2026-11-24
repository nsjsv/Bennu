//! 终端屏幕上某一格是不是链接：程序用 OSC 8 给的链接、网址、或者一个可能是路径的词。
//! Ctrl 悬停画下划线、Ctrl+点击打开都按它认。

use std::path::{Path, PathBuf};

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Direction, Point};
use alacritty_terminal::term::search::{RegexIter, RegexSearch};
use alacritty_terminal::term::Term;

use iced::Task;

use super::emulator::grid_point;
use super::session::EventProxy;
use super::TerminalPanelMessage;
use crate::app::FileBrowser;
use crate::model::Message;

/// 认网址的写法（alacritty 默认的那条）：协议开头，到空白、引号、尖括号之类为止。
const URL_PATTERN: &str = "(ipfs:|ipns:|magnet:|mailto:|gemini://|gopher://|https://|http://|news:|file:|git://|ssh:|ftp://)[^\u{0000}-\u{001F}\u{007F}-\u{009F}<>\"\\s{-}\\^⟨⟩`]+";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TerminalLink {
    /// 交给系统默认程序打开。
    Url(String),
    /// 屏幕上写的样子；相对路径、`~` 由宿主按 shell 所在目录解释，存在才定位。
    Path(String),
}

/// 链接在屏幕上占的格子（同一行，含两头）：画下划线用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScreenSpan {
    pub(crate) line: usize,
    pub(crate) first_column: usize,
    pub(crate) last_column: usize,
}

/// 屏幕第 `row` 行第 `column` 列上的链接。先认程序给的链接，再认网址，最后把那个词（跟双击
/// 选词同一套边界）当路径。
pub(crate) fn link_at(
    term: &Term<EventProxy>,
    row: usize,
    column: usize,
) -> Option<(TerminalLink, ScreenSpan)> {
    let point = grid_point(term, row, column);
    let to_screen = |first: Point, last: Point| ScreenSpan {
        line: row,
        first_column: first.column.0,
        last_column: last.column.0,
    };
    if let Some(link) = term.grid()[point].hyperlink() {
        let same = |column: usize| {
            term.grid()[Point::new(point.line, Column(column))]
                .hyperlink()
                .is_some_and(|other| other == link)
        };
        let first = (0..=column)
            .rev()
            .take_while(|&column| same(column))
            .last()?;
        let last = (column..term.columns())
            .take_while(|&column| same(column))
            .last()?;
        return Some((
            TerminalLink::Url(link.uri().to_owned()),
            ScreenSpan {
                line: row,
                first_column: first,
                last_column: last,
            },
        ));
    }
    if let Ok(mut regex) = RegexSearch::new(URL_PATTERN) {
        let line_start = Point::new(point.line, Column(0));
        let line_end = Point::new(point.line, term.last_column());
        let found = RegexIter::new(line_start, line_end, Direction::Right, term, &mut regex)
            .find(|found| found.contains(&point));
        if let Some(found) = found {
            let text = term.bounds_to_string(*found.start(), *found.end());
            return Some((
                TerminalLink::Url(text),
                to_screen(*found.start(), *found.end()),
            ));
        }
    }
    let first = term.semantic_search_left(point);
    let last = term.semantic_search_right(point);
    let word = term.bounds_to_string(first, last);
    let word = word.trim();
    (!word.is_empty()).then(|| (TerminalLink::Path(word.to_owned()), to_screen(first, last)))
}

impl FileBrowser {
    /// Ctrl+点击：网址交给系统默认程序；路径在后台看一眼，在就到文件区里定位。
    pub(super) fn open_terminal_link(&mut self, link: TerminalLink) -> Task<Message> {
        match link {
            TerminalLink::Url(url) => Task::perform(
                async move {
                    desktop_linux::open_url(&url)
                        .await
                        .map_err(|error| error.to_string())
                },
                |outcome| Message::TerminalPanel(TerminalPanelMessage::LinkOpenFinished(outcome)),
            ),
            TerminalLink::Path(text) => {
                let Some(tab) = self.terminal_panel.active_tab() else {
                    return Task::none();
                };
                let path = resolve_path(&text, &tab.directory, &self.home_dir);
                Task::perform(
                    async move {
                        let is_directory = tokio::fs::metadata(&path)
                            .await
                            .ok()
                            .map(|metadata| metadata.is_dir());
                        (path, is_directory)
                    },
                    |(path, is_directory)| {
                        Message::TerminalPanel(TerminalPanelMessage::LinkPathChecked {
                            path,
                            is_directory,
                        })
                    },
                )
            }
        }
    }
}

/// 屏幕上写的路径按 shell 的规矩解释：`~` 是主目录，相对路径从 shell 所在的目录算。
fn resolve_path(text: &str, shell_directory: &Path, home: &Path) -> PathBuf {
    if text == "~" {
        home.to_path_buf()
    } else if let Some(rest) = text.strip_prefix("~/") {
        home.join(rest)
    } else {
        shell_directory.join(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_panel::emulator::tests::{emulator, feed};

    fn link(bytes: &[u8], column: usize) -> Option<(TerminalLink, ScreenSpan)> {
        let emulator = emulator(60, 3);
        feed(&emulator, bytes);
        let term = emulator.lock();
        link_at(&term, 0, column)
    }

    #[test]
    fn a_web_address_is_a_url_up_to_the_quote() {
        let found = link(b"see \"https://example.com/a?b=1\" here", 10);
        assert_eq!(
            found,
            Some((
                TerminalLink::Url("https://example.com/a?b=1".to_owned()),
                ScreenSpan {
                    line: 0,
                    first_column: 5,
                    last_column: 29
                }
            ))
        );
    }

    #[test]
    fn a_word_with_a_line_number_after_it_is_the_path() {
        let found = link(b"error at src/main.rs:12:5", 12);
        assert_eq!(
            found.map(|(link, _)| link),
            Some(TerminalLink::Path("src/main.rs".to_owned()))
        );
    }

    #[test]
    fn a_link_the_program_gave_wins_over_its_text() {
        // OSC 8：显示的是 docs，链接是网址。
        let found = link(
            b"\x1b]8;;https://bennu.example/docs\x1b\\docs\x1b]8;;\x1b\\ end",
            1,
        );
        assert_eq!(
            found,
            Some((
                TerminalLink::Url("https://bennu.example/docs".to_owned()),
                ScreenSpan {
                    line: 0,
                    first_column: 0,
                    last_column: 3
                }
            ))
        );
    }

    #[test]
    fn blank_space_is_no_link() {
        assert_eq!(link(b"a     b", 3), None);
    }

    #[test]
    fn paths_are_read_the_way_the_shell_reads_them() {
        let shell = Path::new("/work/project");
        let home = Path::new("/home/me");
        assert_eq!(
            resolve_path("src/a.rs", shell, home),
            PathBuf::from("/work/project/src/a.rs")
        );
        assert_eq!(
            resolve_path("/etc/hosts", shell, home),
            PathBuf::from("/etc/hosts")
        );
        assert_eq!(
            resolve_path("~/notes", shell, home),
            PathBuf::from("/home/me/notes")
        );
        assert_eq!(resolve_path("~", shell, home), PathBuf::from("/home/me"));
    }
}
