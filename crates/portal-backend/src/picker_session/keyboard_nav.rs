//! 列表键盘导航：光标移动（↑/↓/Home/End/PageUp/PageDown）、type-ahead
//! 子串跳转与滚动跟随。选中语义不另立规则——光标落行统一走
//! `click_entry(row, false, false)`，与鼠标单击同源（三模式种类门控、
//! SaveFile 文件名同步都在那里维护）；本模块只拥有键盘特有的状态：
//! 光标行、type-ahead 缓冲（1s 截止，惰性过期，不挂定时器）。

use std::time::{Duration, Instant};

use bennu_theme::icon_grid_geometry::IconGridDirection;
use file_core::entry::FileKind;

use super::scrollbar::SessionScrollRegion;
use super::{PickerKind, PickerSession, PickerViewMode, SessionEffect, SessionMessage};

/// type-ahead 截止时长：约 1s 无后续输入即重置缓冲（资源管理器惯例）。
const TYPE_AHEAD_RESET_AFTER: Duration = Duration::from_millis(1000);
/// 视口缓存缺失（首帧探针未回）时的翻页步长回落值（行）。
pub(super) const PAGE_FALLBACK_ROWS: isize = 10;

/// 键盘导航状态。
#[derive(Debug, Default)]
pub(crate) struct KeyboardNavState {
    /// 键盘光标行；None = 本目录尚未开始键盘导航。
    cursor: Option<usize>,
    /// type-ahead 缓冲（匹配时才小写化，这里保留原始输入）。
    type_ahead: String,
    /// 缓冲截止时刻；惰性过期——只在下一次输入/查询时检查。
    type_ahead_deadline: Option<Instant>,
}

impl KeyboardNavState {
    /// 行集重建（扫描回填/过滤切换/展开收起）后的光标重验证：越界
    /// 回退到最后一行，空列表清空光标。
    pub(crate) fn revalidate_cursor(&mut self, row_count: usize) {
        self.cursor = self
            .cursor
            .filter(|_| row_count > 0)
            .map(|cursor| cursor.min(row_count - 1));
    }

    /// 导航换目录：光标与缓冲整体作废。
    pub(crate) fn reset(&mut self) {
        self.cursor = None;
        self.clear_type_ahead();
    }

    /// 缓冲是否仍活跃（非空且未过截止）；过期缓冲对 Esc 路由不可见。
    fn is_type_ahead_active(&self) -> bool {
        !self.type_ahead.is_empty()
            && self
                .type_ahead_deadline
                .is_some_and(|deadline| deadline > Instant::now())
    }

    /// 追加一个字符；截止已过则先重置再追加（惰性过期，无需定时器）。
    fn push_type_ahead(&mut self, ch: char, now: Instant) {
        if self
            .type_ahead_deadline
            .is_none_or(|deadline| deadline <= now)
        {
            self.type_ahead.clear();
        }
        self.type_ahead.push(ch);
        self.type_ahead_deadline = Some(now + TYPE_AHEAD_RESET_AFTER);
    }

    pub(crate) fn clear_type_ahead(&mut self) {
        self.type_ahead.clear();
        self.type_ahead_deadline = None;
    }

    /// 当前缓冲的小写匹配串。
    fn type_ahead_needle(&self) -> String {
        self.type_ahead.to_lowercase()
    }

    /// 追加一个字符并返回当前缓冲的小写匹配串（列表与多栏 type-ahead
    /// 共用的入口：缓冲归 keyboard_nav，匹配范围由调用方按焦点行集
    /// 决定）。
    pub(crate) fn advance_type_ahead(&mut self, ch: char) -> String {
        self.push_type_ahead(ch, Instant::now());
        self.type_ahead_needle()
    }
}

impl PickerSession {
    /// ↑/↓ 移动光标（列表键同时清空 type-ahead 缓冲）。多栏下作用于
    /// 焦点栏（语义见 columns 子模块）。
    pub(crate) fn move_list_cursor(&mut self, delta: isize) -> SessionEffect {
        if self.view_mode() == PickerViewMode::Columns {
            return self.columns_move_cursor(delta);
        }
        self.keyboard_nav.clear_type_ahead();
        self.move_list_cursor_by_rows(delta)
    }

    /// Home/End 跳列表首/尾。
    pub(crate) fn jump_list_cursor(&mut self, to_end: bool) -> SessionEffect {
        if self.view_mode() == PickerViewMode::Columns {
            return self.columns_jump_cursor(to_end);
        }
        self.keyboard_nav.clear_type_ahead();
        let row_count = self.rows.len();
        if row_count == 0 {
            return SessionEffect::None;
        }
        self.place_list_cursor(if to_end { row_count - 1 } else { 0 })
    }

    /// PageUp/PageDown 按视口行数翻页（步长随视图模式：列表按行，
    /// 大图按行×列数，见 view_mode 子模块）。
    pub(crate) fn move_list_page(&mut self, pages: isize) -> SessionEffect {
        if self.view_mode() == PickerViewMode::Columns {
            return self.columns_page_cursor(pages);
        }
        self.keyboard_nav.clear_type_ahead();
        self.move_list_cursor_by_rows(pages.saturating_mul(self.list_page_rows()))
    }

    /// type-ahead 字符输入：累积收窄并跳首个匹配行；无匹配保持原位且
    /// 不清缓冲（用户可能处在更长输入的中间态）。多栏下匹配焦点栏。
    pub(crate) fn push_type_ahead_char(&mut self, ch: char) -> SessionEffect {
        let needle = self.keyboard_nav.advance_type_ahead(ch);
        if self.view_mode() == PickerViewMode::Columns {
            return self.columns_type_ahead_match(needle);
        }
        let matched = self.rows.iter().position(|row| {
            row.entry
                .name
                .to_string_lossy()
                .to_lowercase()
                .contains(&needle)
        });
        match matched {
            Some(index) => self.place_list_cursor(index),
            None => SessionEffect::None,
        }
    }

    /// 清空 type-ahead 缓冲（Esc 优先级：非编辑态时先于关窗）。
    pub(crate) fn reset_type_ahead(&mut self) {
        self.keyboard_nav.clear_type_ahead();
    }

    /// main 层 Esc 路由查询：缓冲非空且未过期。
    pub(crate) fn type_ahead_is_active(&self) -> bool {
        self.keyboard_nav.is_type_ahead_active()
    }

    /// 键盘光标行（测试观察口；运行时路由不需要读取光标本身）。
    pub(crate) fn list_cursor(&self) -> Option<usize> {
        self.keyboard_nav.cursor
    }

    /// 光标记录收口：键盘落点与鼠标点击统一走这里（滚动跟随由调用方
    /// 决定）。不碰选中集——非可选行的位置可见性靠 row_highlighted。
    pub(crate) fn place_cursor(&mut self, index: usize) {
        self.keyboard_nav.cursor = Some(index);
    }

    /// ←/→ 折叠开关的目标行：光标所在行是目录行时返回索引（任意模式；
    /// 文件行不劫持）。
    pub(crate) fn cursor_directory_row(&self) -> Option<usize> {
        let index = self.keyboard_nav.cursor?;
        match self.rows.get(index) {
            Some(row) if row.entry.kind == FileKind::Directory => Some(index),
            _ => None,
        }
    }

    /// Enter 的激活目标消息：光标在目录行且当前模式不是“选目录”时，
    /// 返回激活该行的消息（列表 = 双击进入；多栏 = 打开子栏）；其余
    /// None → 调用方走确认路径（多选保持选中集）。
    pub(crate) fn keyboard_enter_activation(&self) -> Option<SessionMessage> {
        if matches!(
            self.kind,
            PickerKind::OpenFile {
                directory: true,
                ..
            }
        ) {
            return None;
        }
        if self.view_mode() == PickerViewMode::Columns {
            let lane = self.columns.focused();
            let index = self.columns_cursor(lane)?;
            let entry = self.column_entry(lane, index)?;
            (entry.kind == FileKind::Directory)
                .then_some(SessionMessage::ColumnEntryDoubleClicked { lane, index })
        } else {
            self.cursor_directory_row()
                .map(|index| SessionMessage::EntryDoubleClicked { index })
        }
    }

    /// ←/→ 无光标位移语义时的二级动作（view_mode 模式分派的键盘侧）：
    /// 列表 = 目录行折叠开关；多栏 = 焦点在父子栏之间移动。
    pub(crate) fn arrow_secondary_action(
        &self,
        direction: IconGridDirection,
    ) -> Option<SessionMessage> {
        if self.view_mode() == PickerViewMode::Columns {
            return match direction {
                IconGridDirection::Left => Some(SessionMessage::ColumnsBackwardRequested),
                IconGridDirection::Right => Some(SessionMessage::ColumnsForwardRequested),
                _ => None,
            };
        }
        self.cursor_directory_row()
            .map(|index| SessionMessage::EntryExpandToggled { index })
    }

    /// 按位移行数移动光标；空列表 no-op。首次键盘移动落在当前选中首行
    /// （或第 0 行）且本次不位移——与“从选中处出发”的直觉一致。
    fn move_list_cursor_by_rows(&mut self, delta: isize) -> SessionEffect {
        let row_count = self.rows.len();
        if row_count == 0 {
            return SessionEffect::None;
        }
        let Some(current) = self.keyboard_nav.cursor else {
            let landing = self.selection.first().copied().unwrap_or(0);
            return self.place_list_cursor(landing);
        };
        let target = current.saturating_add_signed(delta).min(row_count - 1);
        self.place_list_cursor(target)
    }

    /// 光标落行：复用 click_entry 的单击选中规则（三模式门控 + SaveFile
    /// 文件名同步与鼠标同源；光标记录也在其中），再滚动跟随。
    fn place_list_cursor(&mut self, target: usize) -> SessionEffect {
        self.click_entry(target, false, false);
        self.scroll_list_to_cursor()
    }

    /// 视图切换后的主选中项揭示：只落光标并滚动跟随，不改选中集
    /// （多选集在切换后必须完整保留，与键盘移动的单选语义解耦）。
    pub(super) fn reveal_cursor_on(&mut self, index: usize) -> SessionEffect {
        self.place_cursor(index);
        self.scroll_list_to_cursor()
    }

    /// 翻页步长（条目数）：视口高度按当前视图步长换算，至少 1 条；
    /// 无视口缓存（首帧探针未回）时回落 10 行（PageUp/Down 仍可用）。
    fn list_page_rows(&self) -> isize {
        self.scrollbar_viewport_for(&SessionScrollRegion::List)
            .map(|viewport| self.page_step_entries(viewport.viewport_height))
            .unwrap_or(PAGE_FALLBACK_ROWS)
    }

    /// 光标条目滚入视野：条目矩形不在视口时把偏移贴到对应边缘（上
    /// 方贴顶/下方贴底），经 SessionEffect 交给 main 层执行 scroll_to
    /// （即时到位，不触发惯性）。无视口缓存（首帧探针未回）时跳过
    /// ——缓存就位后下一次移动即可跟随。行矩形按视图模式几何计算
    /// （cursor_row_span）。
    fn scroll_list_to_cursor(&mut self) -> SessionEffect {
        let Some(cursor) = self.keyboard_nav.cursor else {
            return SessionEffect::None;
        };
        let Some(viewport) = self.scrollbar_viewport_for(&SessionScrollRegion::List) else {
            return SessionEffect::None;
        };
        let (row_top, row_height) = self.cursor_row_span(cursor);
        let row_bottom = row_top + row_height;
        let target = if row_top < viewport.offset_y {
            row_top
        } else if row_bottom > viewport.offset_y + viewport.viewport_height {
            row_bottom - viewport.viewport_height
        } else {
            return SessionEffect::None;
        };
        let max_offset = (viewport.content_height - viewport.viewport_height).max(0.0);
        let clamped = target.clamp(0.0, max_offset);
        self.record_keyboard_list_scroll(clamped);
        // 偏移突变让新行进入可见区间：与滚动路径同源补排缩略图。
        self.schedule_visible_thumbnails();
        SessionEffect::ScrollListTo { offset_y: clamped }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_ahead_expires_lazily_on_next_input() {
        let mut state = KeyboardNavState::default();
        let now = Instant::now();
        state.push_type_ahead('a', now);
        assert_eq!(state.type_ahead_needle(), "a");

        // 截止已过：下一次输入先重置再追加，而不是续接旧缓冲。
        let later = now + TYPE_AHEAD_RESET_AFTER + Duration::from_millis(1);
        state.push_type_ahead('b', later);
        assert_eq!(state.type_ahead_needle(), "b");
    }

    #[test]
    fn type_ahead_is_active_respects_deadline() {
        let mut state = KeyboardNavState::default();
        assert!(!state.is_type_ahead_active());

        state.push_type_ahead('a', Instant::now());
        assert!(state.is_type_ahead_active());

        // 过期后缓冲虽在，活跃判定为假：Esc 不再吞掉关窗。
        state.type_ahead_deadline = Some(Instant::now() - Duration::from_millis(1));
        assert!(!state.is_type_ahead_active());

        state.clear_type_ahead();
        assert!(!state.is_type_ahead_active());
        assert_eq!(state.type_ahead_needle(), "");
    }

    #[test]
    fn revalidate_cursor_clamps_and_clears() {
        let mut state = KeyboardNavState {
            cursor: Some(5),
            ..KeyboardNavState::default()
        };
        state.revalidate_cursor(3);
        assert_eq!(state.cursor, Some(2));
        state.revalidate_cursor(0);
        assert_eq!(state.cursor, None);
        state.revalidate_cursor(4);
        assert_eq!(state.cursor, None);
    }
}
