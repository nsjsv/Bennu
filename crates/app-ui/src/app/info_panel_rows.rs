//! 信息面板里的文字：标题和一列「标签 + 可选中的值」。
//!
//! 值是可选中的控件，选区在它的内容里，所以内容是面板状态，不能每次画面重建时现拼。文字由
//! `info_panel_text` 算好，这里拿它和手上的行比，没变的行原样留着，选区才不会被无关的更新冲掉。

use bennu_theme::controls::SelectableTextContent;
use iced::widget::text_editor::Action;

use crate::view::InfoPanelText;

#[derive(Debug)]
pub(crate) struct InfoValueRow {
    /// 行在面板里的编号，建出来就不变也不重用。画面按它保留控件状态（焦点、光标），动作消息
    /// 也按它找行：行的位置会因为多出或少掉一行而挪，按位置找会把动作交给别的行。
    pub(crate) key: u64,
    pub(crate) label: String,
    pub(crate) value: SelectableTextContent,
}

#[derive(Debug, Default)]
pub(crate) struct InfoPanelRows {
    title: Option<String>,
    values: Vec<InfoValueRow>,
    next_key: u64,
}

impl InfoPanelRows {
    pub(crate) fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub(crate) fn values(&self) -> &[InfoValueRow] {
        &self.values
    }

    /// 手上的行对到新算出的文字上：标签和值都没变的行原样留着（选区、焦点跟着留），变了的和
    /// 新来的换成新编号的新行，控件状态从头开始。
    pub(super) fn reconcile(&mut self, text: InfoPanelText) {
        self.title = text.title;
        let mut previous = std::mem::take(&mut self.values);
        let mut last_key = self.next_key;
        self.values = text
            .entries
            .into_iter()
            .map(|entry| {
                let unchanged = previous
                    .iter()
                    .position(|row| row.label == entry.label && row.value.text() == entry.value);
                match unchanged {
                    Some(index) => previous.remove(index),
                    None => {
                        last_key += 1;
                        InfoValueRow {
                            key: last_key,
                            value: SelectableTextContent::new(&entry.value),
                            label: entry.label,
                        }
                    }
                }
            })
            .collect();
        self.next_key = last_key;
    }

    /// 控件发来的动作交给那一行。动作发出后面板内容可能已经换了，行不在了就丢掉。
    pub(super) fn perform(&mut self, key: u64, action: Action) {
        if let Some(row) = self.values.iter_mut().find(|row| row.key == key) {
            row.value.perform(action);
        }
    }
}
