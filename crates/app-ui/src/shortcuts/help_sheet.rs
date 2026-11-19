//! F1 快捷键一览的内容：用户当前实际的键（改过的就是改后的），加上固定键、vim 命令和
//! 预览窗口里的键。视图只负责排版。

use super::action::{ShortcutAction, ShortcutGroup};
use super::keymap::ShortcutConfig;
use super::letter_layer::LetterMode;

pub(crate) struct HelpSection {
    pub(crate) title: String,
    pub(crate) rows: Vec<HelpRow>,
}

pub(crate) struct HelpRow {
    pub(crate) keys: String,
    pub(crate) label: String,
}

fn row(keys: impl Into<String>, label: String) -> HelpRow {
    HelpRow {
        keys: keys.into(),
        label,
    }
}

fn labels(actions: &[ShortcutAction]) -> String {
    actions
        .iter()
        .map(|action| action.label())
        .collect::<Vec<_>>()
        .join(" / ")
}

pub(crate) fn shortcut_help_sections(
    config: &ShortcutConfig,
    letter_mode: LetterMode,
) -> Vec<HelpSection> {
    let mut sections = vec![moving_section(letter_mode)];
    let mut groups: Vec<(ShortcutGroup, Vec<HelpRow>)> = Vec::new();
    for action in ShortcutAction::all().iter().copied() {
        let keys = config
            .keys(action)
            .iter()
            .map(|key| key.display_label())
            .collect::<Vec<_>>();
        let keys = if keys.is_empty() {
            "—".to_owned()
        } else {
            keys.join(" / ")
        };
        let entry = row(keys, action.label());
        match groups.last_mut() {
            Some((group, rows)) if *group == action.group() => rows.push(entry),
            _ => groups.push((action.group(), vec![entry])),
        }
    }
    for (group, mut rows) in groups {
        if group == ShortcutGroup::Terminal {
            rows.push(row(
                "Ctrl+Shift+C / Ctrl+Shift+V",
                labels(&[ShortcutAction::Copy, ShortcutAction::Paste]),
            ));
        }
        sections.push(HelpSection {
            title: group.label(),
            rows,
        });
    }
    if letter_mode == LetterMode::Vim {
        sections.push(vim_section());
    }
    sections.extend(preview_sections());
    sections
}

fn moving_section(letter_mode: LetterMode) -> HelpSection {
    let mut rows = vec![
        row("↑ ↓ ← →", tr!("Move the cursor")),
        row("Home / End", tr!("First / last item")),
        row("PgUp / PgDn", tr!("Previous / next page")),
        row(tr!("Shift + movement key"), tr!("Extend the selection")),
        row("Esc", tr!("Close the top layer, then clear the selection")),
        row("Alt+1…9", tr!("Go to tab 1…9 (9 is the last one)")),
    ];
    if letter_mode == LetterMode::TypeToJump {
        rows.push(row(
            tr!("Type letters"),
            tr!("Jump to a name starting with them"),
        ));
    }
    HelpSection {
        title: tr!("Moving and Selecting"),
        rows,
    }
}

fn vim_section() -> HelpSection {
    use ShortcutAction as A;
    HelpSection {
        title: LetterMode::Vim.label(),
        rows: vec![
            row("h j k l", tr!("Move the cursor")),
            row("J / K", tr!("Extend down / up")),
            row("gg / G", tr!("First / last item")),
            row("H / L", labels(&[A::NavigateBack, A::NavigateForward])),
            row("o", A::OpenSelected.label()),
            row("y / x / p", labels(&[A::Copy, A::Cut, A::Paste])),
            row("d / D", labels(&[A::Delete, A::DeletePermanently])),
            row("r", A::RenameSelected.label()),
            row("a / A", labels(&[A::NewFile, A::NewFolder])),
            row("u / U", labels(&[A::Undo, A::Redo])),
            row("v", tr!("Range selection (v again or Esc leaves it)")),
            row(".", A::ToggleHiddenFiles.label()),
            row("/", tr!("Jump to a name (Enter finishes)")),
            row("n / N", tr!("Next / previous match")),
            row("s", A::Search.label()),
            row("t / q", labels(&[A::NewTab, A::CloseTab])),
            row("gt / gT  1…9", tr!("Switch tabs")),
            row("cc", A::CopyPath.label()),
            row("gh", A::NavigateHome.label()),
        ],
    }
}

fn preview_sections() -> [HelpSection; 3] {
    [
        HelpSection {
            title: tr!("Picture Preview"),
            rows: vec![
                row("← / →", tr!("Previous / next picture")),
                row("+ / - / 0 / 1", tr!("Zoom in / out / fit / actual size")),
                row("i", tr!("Show or hide details")),
            ],
        },
        HelpSection {
            title: tr!("Audio and Video Preview"),
            rows: vec![
                row("Enter", tr!("Play or pause")),
                row("← / →", tr!("Back / forward (Shift goes farther)")),
                row("↑ / ↓", tr!("Volume up / down")),
                row("f", tr!("Full screen")),
                row("i", tr!("Show or hide details")),
            ],
        },
        HelpSection {
            title: tr!("Document Preview"),
            rows: vec![
                row("PgUp / PgDn", tr!("Previous / next page")),
                row("Home / End", tr!("Start / end")),
                row("+ / - / 0", tr!("Zoom in / out / fit width")),
                row("Ctrl+F", tr!("Find")),
                row("F3 / Shift+F3", tr!("Next / previous match")),
                row("Ctrl+C", ShortcutAction::Copy.label()),
            ],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shortcuts::KeyPress;

    fn find_row<'a>(sections: &'a [HelpSection], label: &str) -> Option<&'a HelpRow> {
        sections
            .iter()
            .flat_map(|section| &section.rows)
            .find(|row| row.label == label)
    }

    #[test]
    fn the_sheet_shows_the_keys_the_user_set() {
        let mut config = ShortcutConfig::default();
        let default = shortcut_help_sections(&config, LetterMode::TypeToJump);
        assert_eq!(
            find_row(&default, &ShortcutAction::Redo.label()).map(|row| row.keys.as_str()),
            Some("Ctrl+Shift+Z / Ctrl+Y")
        );

        config.set_keys(ShortcutAction::Redo, vec![KeyPress::ctrl('R')]);
        config.set_keys(ShortcutAction::RenameSelected, Vec::new());
        let changed = shortcut_help_sections(&config, LetterMode::TypeToJump);
        assert_eq!(
            find_row(&changed, &ShortcutAction::Redo.label()).map(|row| row.keys.as_str()),
            Some("Ctrl+R")
        );
        assert_eq!(
            find_row(&changed, &ShortcutAction::RenameSelected.label())
                .map(|row| row.keys.as_str()),
            Some("—")
        );
    }

    #[test]
    fn vim_commands_appear_only_in_vim_mode() {
        let config = ShortcutConfig::default();
        let title = LetterMode::Vim.label();
        let has_vim = |mode| {
            shortcut_help_sections(&config, mode)
                .iter()
                .any(|section| section.title == title)
        };
        assert!(!has_vim(LetterMode::TypeToJump));
        assert!(has_vim(LetterMode::Vim));
    }
}
