use iced::Task;

use super::FileBrowser;
use crate::model::{Message, WindowControlKind, WindowControlMoveDirection, WindowControlSide};

impl FileBrowser {
    pub(super) fn toggle_window_control_visibility(
        &mut self,
        kind: WindowControlKind,
    ) -> Task<Message> {
        let visibility = self
            .user_config
            .window_controls
            .placement(kind)
            .visibility()
            .toggled();
        if self
            .user_config
            .window_controls
            .set_visibility(kind, visibility)
        {
            self.persist_user_preferences_command()
        } else {
            Task::none()
        }
    }

    pub(super) fn select_window_control_side(
        &mut self,
        kind: WindowControlKind,
        side: WindowControlSide,
    ) -> Task<Message> {
        if self.user_config.window_controls.move_to_side(kind, side) {
            self.persist_user_preferences_command()
        } else {
            Task::none()
        }
    }

    pub(super) fn move_window_control_within_side(
        &mut self,
        kind: WindowControlKind,
        direction: WindowControlMoveDirection,
    ) -> Task<Message> {
        // The model refuses boundary moves, and the view hides those arrows,
        // so a persisting click always follows a real reorder.
        if self
            .user_config
            .window_controls
            .move_within_side(kind, direction)
        {
            self.persist_user_preferences_command()
        } else {
            Task::none()
        }
    }

    pub(super) fn reset_window_controls(&mut self) -> Task<Message> {
        if self.user_config.window_controls.reset() {
            self.persist_user_preferences_command()
        } else {
            Task::none()
        }
    }

    /// 所有带侧栏的窗口下一帧就换样子（外壳每帧按设置画）；文件选择对话框下次打开时读。
    pub(super) fn select_sidebar_style(
        &mut self,
        style: bennu_theme::window_controls::SidebarStyle,
    ) -> Task<Message> {
        if self.user_config.sidebar_style == style {
            return Task::none();
        }
        self.user_config.sidebar_style = style;
        self.persist_user_preferences_command()
    }
}

#[cfg(test)]
mod sidebar_style_tests {
    use super::*;
    use bennu_theme::window_controls::SidebarStyle;

    #[test]
    fn choosing_a_sidebar_style_saves_only_a_real_change() {
        let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());

        drop(browser.select_sidebar_style(SidebarStyle::Embedded));
        assert!(!browser.user_preferences_save_in_flight);

        drop(browser.select_sidebar_style(SidebarStyle::Floating));
        assert_eq!(browser.user_config.sidebar_style, SidebarStyle::Floating);
        assert!(browser.user_preferences_save_in_flight);
    }
}

#[cfg(test)]
mod tests {
    use iced_runtime::task::into_stream;

    use super::*;
    use crate::config;
    use crate::model::WindowControlVisibility;

    fn kinds_on(browser: &FileBrowser, side: WindowControlSide) -> Vec<WindowControlKind> {
        browser
            .user_config()
            .window_controls
            .placements_on(side)
            .map(|placement| placement.kind())
            .collect()
    }

    #[test]
    fn visibility_and_side_changes_persist_only_real_updates() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());

        assert!(
            into_stream(browser.toggle_window_control_visibility(WindowControlKind::Minimize))
                .is_some()
        );
        drop(browser.accept_user_preferences_saved(Ok(())));
        assert_eq!(
            browser
                .user_config()
                .window_controls
                .placement(WindowControlKind::Minimize)
                .visibility(),
            WindowControlVisibility::Hidden
        );
        assert!(into_stream(
            browser.select_window_control_side(WindowControlKind::Close, WindowControlSide::Left)
        )
        .is_some());
        drop(browser.accept_user_preferences_saved(Ok(())));
        assert!(into_stream(
            browser.select_window_control_side(WindowControlKind::Close, WindowControlSide::Left)
        )
        .is_none());
    }

    #[test]
    fn close_visibility_toggle_is_rejected_without_persistence() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());

        assert!(
            into_stream(browser.toggle_window_control_visibility(WindowControlKind::Close))
                .is_none()
        );
        assert!(browser
            .user_config()
            .window_controls
            .placement(WindowControlKind::Close)
            .visibility()
            .is_visible());
    }

    #[test]
    fn move_within_side_persists_only_real_moves() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());

        // First row of its side: the up arrow is not rendered, and the model
        // refusal must skip persistence too.
        assert!(into_stream(browser.move_window_control_within_side(
            WindowControlKind::Minimize,
            WindowControlMoveDirection::Up
        ))
        .is_none());

        assert!(into_stream(browser.move_window_control_within_side(
            WindowControlKind::Minimize,
            WindowControlMoveDirection::Down
        ))
        .is_some());
        drop(browser.accept_user_preferences_saved(Ok(())));
        assert_eq!(
            kinds_on(&browser, WindowControlSide::Right),
            vec![
                WindowControlKind::MaximizeRestore,
                WindowControlKind::Minimize,
                WindowControlKind::Close,
            ]
        );

        // Each further down move keeps persisting until the side tail, where
        // the model refuses again.
        assert!(into_stream(browser.move_window_control_within_side(
            WindowControlKind::Minimize,
            WindowControlMoveDirection::Down
        ))
        .is_some());
        drop(browser.accept_user_preferences_saved(Ok(())));
        assert!(into_stream(browser.move_window_control_within_side(
            WindowControlKind::Minimize,
            WindowControlMoveDirection::Down
        ))
        .is_none());
        assert_eq!(
            kinds_on(&browser, WindowControlSide::Right),
            vec![
                WindowControlKind::MaximizeRestore,
                WindowControlKind::Close,
                WindowControlKind::Minimize,
            ]
        );
    }

    #[test]
    fn settings_and_pointer_lifecycle_leave_window_controls_untouched() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());

        drop(browser.select_settings_category(crate::model::SettingsCategory::Logs));
        drop(browser.close_settings_window());
        drop(browser.finish_pointer_drag_interactions());
        browser.clear_pointer_driven_interaction_state();

        assert_eq!(
            browser.user_config().window_controls,
            crate::model::WindowControlsConfig::default()
        );
    }

    #[test]
    fn reset_restores_confirmed_default() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        drop(browser.select_window_control_side(WindowControlKind::Close, WindowControlSide::Left));
        drop(browser.accept_user_preferences_saved(Ok(())));

        assert!(into_stream(browser.reset_window_controls()).is_some());
        assert_eq!(
            browser.user_config().window_controls,
            crate::model::WindowControlsConfig::default()
        );
        assert!(into_stream(browser.reset_window_controls()).is_none());
    }
}
