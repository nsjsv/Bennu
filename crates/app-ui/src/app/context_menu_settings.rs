use iced::{Point, Task};

use super::{FileBrowser, POINTER_DRAG_ACTIVATION_DISTANCE};
use crate::model::{
    ContextMenuSettingsDragState, ContextMenuSettingsPage, ContextMenuSettingsPageStep,
    FileAreaMenuItem, Message, CONTEXT_MENU_SETTINGS_ROW_PITCH,
};

impl FileBrowser {
    pub(super) fn shift_context_menu_settings_page(
        &mut self,
        step: ContextMenuSettingsPageStep,
    ) -> Task<Message> {
        self.context_menu_settings_page = self.context_menu_settings_page.stepped(step);
        self.context_menu_reset_confirmation = None;
        self.context_menu_settings_drag = None;
        self.context_menu_preview_expansion = None;
        Task::none()
    }

    /// 设置页文件条目预览的悬停展开;拖拽进行中忽略,避免换位时成员面板跳动。
    pub(super) fn update_context_menu_preview_expansion(
        &mut self,
        expansion: Option<FileAreaMenuItem>,
    ) -> Task<Message> {
        if self.context_menu_settings_drag.is_none() {
            self.context_menu_preview_expansion = expansion;
        }
        Task::none()
    }

    pub(super) fn toggle_context_menu_settings_item(
        &mut self,
        page: ContextMenuSettingsPage,
        index: usize,
    ) -> Task<Message> {
        self.user_config
            .context_menus
            .toggle_settings_row(page, index);
        self.persist_user_preferences_command()
    }

    /// 拖拽在首次移动时记录原点(设置窗口的指针位置不进共享 cursor_position),
    /// 越过激活距离后按行距实时换位,松手时若顺序有变才持久化。
    pub(super) fn start_context_menu_settings_drag(
        &mut self,
        page: ContextMenuSettingsPage,
        index: usize,
    ) -> Task<Message> {
        self.context_menu_reset_confirmation = None;
        self.context_menu_settings_drag = Some(ContextMenuSettingsDragState::new(page, index));
        Task::none()
    }

    pub(super) fn update_context_menu_settings_drag(&mut self, position: Point) -> Task<Message> {
        let Some(mut drag) = self.context_menu_settings_drag else {
            return Task::none();
        };
        let Some(origin) = drag.origin else {
            drag.origin = Some(position);
            drag.latest = Some(position);
            self.context_menu_settings_drag = Some(drag);
            return Task::none();
        };
        drag.latest = Some(position);

        let delta = position - origin;
        if !drag.order_changed
            && delta.x * delta.x + delta.y * delta.y
                < POINTER_DRAG_ACTIVATION_DISTANCE * POINTER_DRAG_ACTIVATION_DISTANCE
        {
            self.context_menu_settings_drag = Some(drag);
            return Task::none();
        }

        let count = self
            .user_config
            .context_menus
            .settings_rows(drag.page)
            .len();
        let last_index = count.saturating_sub(1);
        let projected = (drag.source_index as f32 + delta.y / CONTEXT_MENU_SETTINGS_ROW_PITCH)
            .round()
            .clamp(0.0, last_index as f32) as usize;
        if projected != drag.current_index {
            self.user_config.context_menus.reorder_settings_row(
                drag.page,
                drag.current_index,
                projected,
            );
            drag.current_index = projected;
            drag.order_changed = true;
        }
        self.context_menu_settings_drag = Some(drag);
        Task::none()
    }

    pub(super) fn finish_context_menu_settings_drag(&mut self) -> Task<Message> {
        let Some(drag) = self.context_menu_settings_drag.take() else {
            return Task::none();
        };
        if drag.order_changed {
            return self.persist_user_preferences_command();
        }
        Task::none()
    }

    /// 当前正在被拖动的行号(仅限正在配置的页)。
    pub(crate) fn context_menu_settings_drag_index(&self) -> Option<usize> {
        self.context_menu_settings_drag
            .as_ref()
            .filter(|drag| drag.page == self.context_menu_settings_page)
            .map(|drag| drag.current_index)
    }

    /// 视图渲染被拖行时的纵向位移:光标偏移补偿已发生的换位,并夹在列表范围内。
    pub(crate) fn context_menu_settings_drag_offset(&self, index: usize) -> Option<f32> {
        let drag = self.context_menu_settings_drag.as_ref()?;
        let origin = drag.origin?;
        let latest = drag.latest?;
        if index != drag.current_index {
            return None;
        }
        let count = self
            .user_config
            .context_menus
            .settings_rows(drag.page)
            .len();
        let cursor_offset = latest.y - origin.y;
        let layout_offset = (drag.source_index as f32 - drag.current_index as f32)
            * CONTEXT_MENU_SETTINGS_ROW_PITCH;
        let min_offset = -(drag.current_index as f32) * CONTEXT_MENU_SETTINGS_ROW_PITCH;
        let max_offset =
            (count.saturating_sub(1) - drag.current_index) as f32 * CONTEXT_MENU_SETTINGS_ROW_PITCH;
        Some((cursor_offset + layout_offset).clamp(min_offset, max_offset))
    }

    /// 恢复默认需要行内确认:先记录待确认的页,确认后才真正重置当前页。
    pub(super) fn request_context_menu_settings_reset(
        &mut self,
        page: ContextMenuSettingsPage,
    ) -> Task<Message> {
        self.context_menu_reset_confirmation = Some(page);
        Task::none()
    }

    pub(super) fn confirm_context_menu_settings_reset(
        &mut self,
        page: ContextMenuSettingsPage,
    ) -> Task<Message> {
        self.context_menu_reset_confirmation = None;
        self.user_config.context_menus.reset_settings_page(page);
        self.persist_user_preferences_command()
    }
}
