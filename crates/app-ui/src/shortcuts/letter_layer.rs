//! 字母层的两种用法：打字跳转（默认），或 vim 风格的单键命令。vim 只用不带 Ctrl 的键，
//! Ctrl 组合照旧是常规快捷键；键位是固定表，不在设置里逐个改。

use std::time::{Duration, Instant};

use super::{FileContentKey, MoveStep, SelectionMove, ShortcutAction, TypedLetter};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum LetterMode {
    #[default]
    TypeToJump,
    Vim,
}

impl LetterMode {
    pub(crate) const ALL: [Self; 2] = [Self::TypeToJump, Self::Vim];

    pub(crate) fn config_value(self) -> &'static str {
        match self {
            Self::TypeToJump => "type_to_jump",
            Self::Vim => "vim",
        }
    }

    pub(crate) fn from_config_value(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|mode| mode.config_value() == value)
    }

    pub(crate) fn label(self) -> String {
        match self {
            Self::TypeToJump => tr!("Type to Jump"),
            Self::Vim => tr!("Vim Commands"),
        }
    }
}

/// 两键命令按了第一个键之后、`/` 跳转打字时，停多久算作废 / 结束。
pub(crate) const VIM_INPUT_TIMEOUT: Duration = Duration::from_millis(1000);

/// vim 模式下字母层的状态：两键命令的第一个键、范围选择模式、`/` 跳转输入、上一次跳转的词。
#[derive(Debug, Clone, Default)]
pub(crate) struct VimState {
    pending: Option<(char, Instant)>,
    visual: bool,
    jump_deadline: Option<Instant>,
    last_jump: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum VimCommand {
    Key(FileContentKey),
    TabNumber(u8),
    ToggleVisual,
    StartJump,
    JumpNext,
    JumpPrevious,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum VimFeed {
    Run(VimCommand),
    /// 两键命令的第一个键，等第二个。
    Waiting,
    /// 不是命令（或接不上前一个键），这个键和前一个键都作废。
    Discarded,
}

impl VimState {
    pub(crate) fn feed(&mut self, letter: TypedLetter, now: Instant) -> VimFeed {
        let pending = self
            .pending
            .take()
            .filter(|(_, at)| now.duration_since(*at) < VIM_INPUT_TIMEOUT)
            .map(|(first, _)| first);
        // 布局给的是不带修饰的字符；大写锁定时也可能直接是大写，一并当 Shift。
        let shift = letter.shift || letter.character.is_ascii_uppercase();
        let key = (letter.character.to_ascii_lowercase(), shift);
        let command = match pending {
            Some('g') => match key {
                ('g', false) => self.movement(MoveStep::First),
                ('t', false) => action(ShortcutAction::NextTab),
                ('t', true) => action(ShortcutAction::PreviousTab),
                ('h', false) => action(ShortcutAction::NavigateHome),
                _ => return VimFeed::Discarded,
            },
            Some('c') => match key {
                ('c', false) => action(ShortcutAction::CopyPath),
                _ => return VimFeed::Discarded,
            },
            Some(_) => return VimFeed::Discarded,
            None => match key {
                (first @ ('g' | 'c'), false) => {
                    self.pending = Some((first, now));
                    return VimFeed::Waiting;
                }
                ('h', false) => self.movement(MoveStep::Left),
                ('j', false) => self.movement(MoveStep::Down),
                ('k', false) => self.movement(MoveStep::Up),
                ('l', false) => self.movement(MoveStep::Right),
                ('j', true) => {
                    VimCommand::Key(FileContentKey::Move(SelectionMove::extend(MoveStep::Down)))
                }
                ('k', true) => {
                    VimCommand::Key(FileContentKey::Move(SelectionMove::extend(MoveStep::Up)))
                }
                ('g', true) => self.movement(MoveStep::Last),
                ('h', true) => action(ShortcutAction::NavigateBack),
                ('l', true) => action(ShortcutAction::NavigateForward),
                ('o', false) => action(ShortcutAction::OpenSelected),
                ('y', false) => action(ShortcutAction::Copy),
                ('x', false) => action(ShortcutAction::Cut),
                ('p', false) => action(ShortcutAction::Paste),
                ('d', false) => action(ShortcutAction::Delete),
                ('d', true) => action(ShortcutAction::DeletePermanently),
                ('r', false) => action(ShortcutAction::RenameSelected),
                ('a', false) => action(ShortcutAction::NewFile),
                ('a', true) => action(ShortcutAction::NewFolder),
                ('u', false) => action(ShortcutAction::Undo),
                ('u', true) => action(ShortcutAction::Redo),
                ('v', false) => VimCommand::ToggleVisual,
                ('.', false) => action(ShortcutAction::ToggleHiddenFiles),
                ('/', false) => VimCommand::StartJump,
                ('n', false) => VimCommand::JumpNext,
                ('n', true) => VimCommand::JumpPrevious,
                ('s', false) => action(ShortcutAction::Search),
                ('t', false) => action(ShortcutAction::NewTab),
                ('q', false) => action(ShortcutAction::CloseTab),
                (digit @ '1'..='9', false) => VimCommand::TabNumber(digit as u8 - b'0'),
                _ => return VimFeed::Discarded,
            },
        };
        VimFeed::Run(command)
    }

    /// 范围选择模式里移动就是扩选（和按住 Shift 一样）。
    fn movement(&self, step: MoveStep) -> VimCommand {
        let movement = if self.visual {
            SelectionMove::extend(step)
        } else {
            SelectionMove::cursor(step)
        };
        VimCommand::Key(FileContentKey::Move(movement))
    }

    pub(crate) fn toggle_visual(&mut self) {
        self.visual = !self.visual;
    }

    pub(crate) fn start_jump(&mut self, now: Instant) {
        self.jump_deadline = Some(now + VIM_INPUT_TIMEOUT);
    }

    /// `/` 之后打的一个字已经拿去跳转：截止时刻顺延，记下这次跳转的词给 n / N 用。
    pub(crate) fn note_jump_text(&mut self, typed: &str, now: Instant) {
        self.jump_deadline = Some(now + VIM_INPUT_TIMEOUT);
        self.last_jump = Some(typed.to_owned());
    }

    pub(crate) fn last_jump(&self) -> Option<&str> {
        self.last_jump.as_deref()
    }

    pub(crate) fn is_jumping(&self, now: Instant) -> bool {
        self.jump_deadline.is_some_and(|deadline| deadline > now)
    }

    pub(crate) fn is_visual(&self) -> bool {
        self.visual
    }

    pub(crate) fn pending_key(&self) -> Option<char> {
        self.pending.map(|(first, _)| first)
    }

    /// Esc 一层层退：先结束 `/` 跳转，再丢掉待定的第一个键，再退出范围选择。返回退没退。
    pub(crate) fn escape(&mut self, now: Instant) -> bool {
        if self.is_jumping(now) {
            self.jump_deadline = None;
        } else if self.pending.is_some() {
            self.pending = None;
        } else if self.visual {
            self.visual = false;
        } else {
            return false;
        }
        true
    }

    /// Enter 结束 `/` 跳转（光标已经在匹配项上，不再打开它）。返回是不是在跳转。
    pub(crate) fn finish_jump(&mut self, now: Instant) -> bool {
        let jumping = self.is_jumping(now);
        self.jump_deadline = None;
        jumping
    }

    /// 到了截止时刻：待定的第一个键作废，`/` 跳转结束。
    pub(crate) fn expire(&mut self, now: Instant) {
        if self
            .pending
            .is_some_and(|(_, at)| now.duration_since(at) >= VIM_INPUT_TIMEOUT)
        {
            self.pending = None;
        }
        if !self.is_jumping(now) {
            self.jump_deadline = None;
        }
    }
}

fn action(action: ShortcutAction) -> VimCommand {
    VimCommand::Key(FileContentKey::Action(action))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn letter(character: char) -> TypedLetter {
        TypedLetter {
            character,
            shift: false,
        }
    }

    fn shifted(character: char) -> TypedLetter {
        TypedLetter {
            character,
            shift: true,
        }
    }

    fn run(command: VimCommand) -> VimFeed {
        VimFeed::Run(command)
    }

    fn moved(step: MoveStep) -> VimFeed {
        run(VimCommand::Key(FileContentKey::Move(
            SelectionMove::cursor(step),
        )))
    }

    fn acted(shortcut: ShortcutAction) -> VimFeed {
        run(action(shortcut))
    }

    #[test]
    fn single_keys_map_to_their_commands() {
        let now = Instant::now();
        let mut vim = VimState::default();
        let table = [
            (letter('h'), moved(MoveStep::Left)),
            (letter('j'), moved(MoveStep::Down)),
            (letter('k'), moved(MoveStep::Up)),
            (letter('l'), moved(MoveStep::Right)),
            (shifted('g'), moved(MoveStep::Last)),
            (shifted('h'), acted(ShortcutAction::NavigateBack)),
            (shifted('l'), acted(ShortcutAction::NavigateForward)),
            (letter('y'), acted(ShortcutAction::Copy)),
            (letter('x'), acted(ShortcutAction::Cut)),
            (letter('p'), acted(ShortcutAction::Paste)),
            (letter('d'), acted(ShortcutAction::Delete)),
            (shifted('d'), acted(ShortcutAction::DeletePermanently)),
            (letter('u'), acted(ShortcutAction::Undo)),
            (shifted('u'), acted(ShortcutAction::Redo)),
            (letter('a'), acted(ShortcutAction::NewFile)),
            (shifted('a'), acted(ShortcutAction::NewFolder)),
            (letter('.'), acted(ShortcutAction::ToggleHiddenFiles)),
            (letter('q'), acted(ShortcutAction::CloseTab)),
            (letter('3'), run(VimCommand::TabNumber(3))),
            (letter('/'), run(VimCommand::StartJump)),
            (shifted('n'), run(VimCommand::JumpPrevious)),
            (letter('z'), VimFeed::Discarded),
            (letter('ф'), VimFeed::Discarded),
        ];
        for (typed, expected) in table {
            assert_eq!(vim.feed(typed, now), expected, "{typed:?}");
        }
        // 大写锁定送来大写字母也当 Shift。
        assert_eq!(vim.feed(letter('G'), now), moved(MoveStep::Last));
    }

    #[test]
    fn two_key_commands_wait_and_then_run_or_get_discarded() {
        let now = Instant::now();
        let mut vim = VimState::default();
        assert_eq!(vim.feed(letter('g'), now), VimFeed::Waiting);
        assert_eq!(vim.pending_key(), Some('g'));
        assert_eq!(vim.feed(letter('g'), now), moved(MoveStep::First));

        assert_eq!(vim.feed(letter('g'), now), VimFeed::Waiting);
        assert_eq!(
            vim.feed(shifted('t'), now),
            acted(ShortcutAction::PreviousTab)
        );
        assert_eq!(vim.feed(letter('c'), now), VimFeed::Waiting);
        assert_eq!(vim.feed(letter('c'), now), acted(ShortcutAction::CopyPath));

        // 接不上：两个键都作废，j 不会当成往下走。
        assert_eq!(vim.feed(letter('g'), now), VimFeed::Waiting);
        assert_eq!(vim.feed(letter('j'), now), VimFeed::Discarded);
        assert_eq!(vim.pending_key(), None);

        // 超时：第一个键作废，第二个键照单键处理。
        assert_eq!(vim.feed(letter('g'), now), VimFeed::Waiting);
        let later = now + VIM_INPUT_TIMEOUT;
        vim.expire(later);
        assert_eq!(vim.pending_key(), None);
        assert_eq!(vim.feed(letter('g'), later), VimFeed::Waiting);
        assert_eq!(
            vim.feed(letter('j'), later + VIM_INPUT_TIMEOUT),
            moved(MoveStep::Down)
        );
    }

    #[test]
    fn visual_mode_turns_moves_into_extensions_and_escape_steps_back() {
        let now = Instant::now();
        let mut vim = VimState::default();
        vim.toggle_visual();
        assert_eq!(
            vim.feed(letter('j'), now),
            run(VimCommand::Key(FileContentKey::Move(
                SelectionMove::extend(MoveStep::Down)
            )))
        );

        vim.start_jump(now);
        assert_eq!(vim.feed(letter('g'), now), VimFeed::Waiting);
        assert!(vim.escape(now), "先结束跳转");
        assert!(vim.escape(now), "再丢掉待定的 g");
        assert!(vim.is_visual());
        assert!(vim.escape(now), "最后退出范围选择");
        assert!(!vim.is_visual());
        assert!(!vim.escape(now));
    }

    #[test]
    fn jumping_ends_on_enter_or_after_a_pause() {
        let now = Instant::now();
        let mut vim = VimState::default();
        vim.start_jump(now);
        vim.note_jump_text("re", now);
        assert!(vim.is_jumping(now));
        assert_eq!(vim.last_jump(), Some("re"));
        assert!(vim.finish_jump(now));
        assert!(!vim.is_jumping(now));

        vim.start_jump(now);
        vim.expire(now + VIM_INPUT_TIMEOUT);
        assert!(!vim.is_jumping(now));
        assert_eq!(vim.last_jump(), Some("re"));
    }
}
