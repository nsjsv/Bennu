//! choices 会话状态：调用方下拉/复选选项的选中值簿记。从 spec 搬入
//! 会话后由视图消息驱动更新；确认回信时按请求顺序回传全部 (id, 选中
//! 值)。独立成子模块是控制 mod.rs 的 800 行上限。

use super::PickerSession;
use crate::picker_request::PickerChoice;

impl PickerSession {
    pub(crate) fn choices(&self) -> &[PickerChoice] {
        &self.choices
    }

    /// 选择更新：按 id 定位覆写 selected。id 不存在静默忽略——视图只
    /// 能从现有 choice 发出消息，正常不可达，此处仅防御视图/状态错位。
    pub(crate) fn choice_selected(&mut self, id: &str, value: String) {
        if let Some(choice) = self.choices.iter_mut().find(|choice| choice.id == id) {
            choice.selected = value;
        }
    }

    /// 回信载荷：全部 choice 的 (id, 选中值)，保持请求里的顺序（协议
    /// 只要求回传 id+值，顺序稳定便于调用方对账）。
    pub(crate) fn selected_choices_for_reply(&self) -> Vec<(String, String)> {
        self.choices
            .iter()
            .map(|choice| (choice.id.clone(), choice.selected.clone()))
            .collect()
    }
}
