//! vim 模式下字母层的按键：查固定表，执行对应的命令（大多就是常规快捷键的同一个动作）。

use std::time::Instant;

use file_core::type_ahead::MatchDirection;
use iced::Task;

use super::selection::letter_input_deadline;
use super::FileBrowser;
use crate::model::Message;
use crate::shortcuts::{LetterMode, TypedLetter, VimCommand, VimFeed};

impl FileBrowser {
    /// 已确认归文件区的一个字母（vim 模式）。`/` 跳转期间打的字拿去跳转。
    pub(super) fn run_vim_letter(&mut self, letter: TypedLetter) -> Task<Message> {
        let now = Instant::now();
        if self.vim.is_jumping(now) {
            let jump = self.jump_to_typed_name(letter.character);
            self.vim.note_jump_text(self.type_ahead.typed(), now);
            return jump;
        }
        match self.vim.feed(letter, now) {
            VimFeed::Run(command) => self.run_vim_command(command, now),
            VimFeed::Waiting => letter_input_deadline(),
            VimFeed::Discarded => Task::none(),
        }
    }

    fn run_vim_command(&mut self, command: VimCommand, now: Instant) -> Task<Message> {
        match command {
            VimCommand::Key(key) => self.invoke_file_content_key(key),
            VimCommand::TabNumber(number) => self.select_tab_by_number(number),
            VimCommand::ToggleVisual => {
                self.vim.toggle_visual();
                Task::none()
            }
            VimCommand::StartJump => {
                self.type_ahead.clear();
                self.vim.start_jump(now);
                letter_input_deadline()
            }
            VimCommand::JumpNext => self.repeat_vim_jump(MatchDirection::Forward),
            VimCommand::JumpPrevious => self.repeat_vim_jump(MatchDirection::Backward),
        }
    }

    fn repeat_vim_jump(&mut self, direction: MatchDirection) -> Task<Message> {
        let Some(needle) = self.vim.last_jump().map(str::to_owned) else {
            return Task::none();
        };
        self.jump_to_adjacent_match(&needle, direction)
    }

    pub(super) fn select_letter_mode(&mut self, mode: LetterMode) -> Task<Message> {
        if self.user_config.letter_mode == mode {
            return Task::none();
        }
        self.user_config.letter_mode = mode;
        self.vim = Default::default();
        self.type_ahead.clear();
        self.persist_user_preferences_command()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use file_core::{DirectoryEntry, EntryMetadata, FileKind};
    use iced::keyboard::{key, Key, Modifiers};

    use super::*;
    use crate::config;
    use crate::model::BrowserLocation;
    use crate::shortcuts::FileContentKey;

    fn vim_browser(names: &[&str]) -> FileBrowser {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        browser.tab_mut().location = BrowserLocation::Directory(PathBuf::from("/workspace"));
        browser.tab_mut().entries = names
            .iter()
            .map(|name| {
                DirectoryEntry::new(
                    PathBuf::from("/workspace").join(name),
                    FileKind::File,
                    EntryMetadata::default(),
                    false,
                    false,
                    false,
                )
            })
            .collect::<Vec<_>>()
            .into();
        drop(browser.select_letter_mode(LetterMode::Vim));
        browser
    }

    fn keys(browser: &mut FileBrowser, typed: &str) {
        for character in typed.chars() {
            let letter = TypedLetter {
                character: character.to_ascii_lowercase(),
                shift: character.is_ascii_uppercase(),
            };
            drop(browser.invoke_routed_file_content_key(FileContentKey::Letter(letter)));
        }
    }

    fn selected(browser: &FileBrowser) -> Option<String> {
        browser
            .tab()
            .selected
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
    }

    fn selected_count(browser: &FileBrowser) -> usize {
        browser.tab().selected_paths.len()
    }

    const NAMES: [&str; 5] = ["alpha", "bravo", "report", "readme", "zulu"];

    #[test]
    fn hjkl_and_g_move_the_cursor() {
        let mut browser = vim_browser(&NAMES);
        keys(&mut browser, "jj");
        assert_eq!(selected(&browser).as_deref(), Some("bravo"));
        keys(&mut browser, "G");
        assert_eq!(selected(&browser).as_deref(), Some("zulu"));
        keys(&mut browser, "k");
        assert_eq!(selected(&browser).as_deref(), Some("readme"));
        keys(&mut browser, "gg");
        assert_eq!(selected(&browser).as_deref(), Some("alpha"));
        // 打字跳转在 vim 模式里不生效：z 不是命令。
        keys(&mut browser, "z");
        assert_eq!(selected(&browser).as_deref(), Some("alpha"));
    }

    #[test]
    fn v_turns_moves_into_a_range_until_escape() {
        let mut browser = vim_browser(&NAMES);
        keys(&mut browser, "j");
        keys(&mut browser, "vjj");
        assert_eq!(selected_count(&browser), 3);
        assert!(browser.vim.is_visual());

        drop(browser.handle_escape_shortcut());
        assert!(!browser.vim.is_visual());
        assert_eq!(selected_count(&browser), 3, "退出范围选择不丢选择");
        keys(&mut browser, "j");
        assert_eq!(selected_count(&browser), 1);
    }

    #[test]
    fn slash_jumps_by_name_enter_finishes_and_n_repeats() {
        let mut browser = vim_browser(&NAMES);
        keys(&mut browser, "/re");
        assert_eq!(selected(&browser).as_deref(), Some("report"));

        drop(browser.handle_keyboard_key_pressed(
            Key::Named(key::Named::Enter),
            Modifiers::default(),
            iced::event::Status::Ignored,
        ));
        assert!(!browser.vim.is_jumping(Instant::now()));
        assert_eq!(
            selected(&browser).as_deref(),
            Some("report"),
            "Enter 不打开"
        );

        keys(&mut browser, "n");
        assert_eq!(selected(&browser).as_deref(), Some("readme"));
        keys(&mut browser, "n");
        assert_eq!(selected(&browser).as_deref(), Some("report"));
        keys(&mut browser, "N");
        assert_eq!(selected(&browser).as_deref(), Some("readme"));
    }

    #[test]
    fn command_letters_run_the_same_actions_as_their_shortcuts() {
        let mut browser = vim_browser(&NAMES);
        keys(&mut browser, "j");
        keys(&mut browser, "y");
        assert!(matches!(
            browser.pending_operation,
            Some(crate::model::PendingOperation::Copy(ref paths)) if paths == &vec![PathBuf::from("/workspace/alpha")]
        ));
        keys(&mut browser, "d");
        assert_eq!(browser.operation_queue.tasks().len(), 1);

        keys(&mut browser, "t");
        assert_eq!(browser.pane().tabs.len(), 2);
    }

    #[test]
    fn switching_back_to_type_to_jump_makes_letters_jump_again() {
        let mut browser = vim_browser(&NAMES);
        keys(&mut browser, "v");
        drop(browser.select_letter_mode(LetterMode::TypeToJump));
        assert!(!browser.vim.is_visual());
        keys(&mut browser, "zu");
        assert_eq!(selected(&browser).as_deref(), Some("zulu"));
    }
}
