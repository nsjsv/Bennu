use std::fmt;

use desktop_linux::{TerminalEmulator, TERMINAL_EMULATOR_OPTIONS};
use iced::widget::Button;
use iced::{window, Element, Length};

use crate::app::FileBrowser;
use crate::config::{
    ColumnWidthAdjustMode, LaunchWindowPolicy, StartupLocationPolicy, UiLanguageSetting,
};
use crate::model::{Message, SettingsCategory, SettingsSubpage};
use bennu_theme::icons::IconSymbol;
use bennu_theme::ThemeMode;

use super::about_settings::about_settings_sections;
use super::application_logs::application_logs_sections;
use super::auxiliary_window_layout::{
    auxiliary_sidebar_button, auxiliary_sidebar_list, AUXILIARY_SIDEBAR_WIDTH,
};
use super::color_scheme_picker::color_scheme_row;
use super::context_menu_settings::context_menu_settings_section;
use super::file_operation_verification_settings::file_operation_verification_options;
use super::network_settings::network_thumbnails_row;
use super::preview_settings::{preview_settings_entry_card, preview_settings_subpage_sections};
use super::rendering_settings::rendering_gpu_preference_row;
use super::search_settings::search_settings_sections;
use super::settings_group::{
    choice_row, control_row, custom_row, entry_row, settings_card, settings_group, settings_page,
    switch_row, EntryRow, RowNote, SettingsRow,
};
use super::shortcut_settings::shortcut_settings_section;
use super::transfer_settings::{favorite_devices_group, transfer_receive_rows};
use super::window_chrome::window_shell;
use super::window_control_settings::window_control_settings_row;
use super::IconTone;
use bennu_preview::window_shell::{split_window, HeaderBar, SplitPane};
use bennu_theme::controls::dropdown;
use bennu_theme::controls::icon_button;
use bennu_theme::controls::SegmentedChoice;

const SETTINGS_DROPDOWN_WIDTH: f32 = 220.0;

/// 顶栏里图标按钮的图标尺寸。
const HEADER_ICON_SIZE: f32 = 14.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TerminalEmulatorPickOption(TerminalEmulator);

impl fmt::Display for TerminalEmulatorPickOption {
    /// 终端名是产品名，不翻译；只有"自动"是界面文案。
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            TerminalEmulator::Automatic => formatter.write_str(&tr!("Auto")),
            emulator => formatter.write_str(emulator.label()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalShellPickOption(String);

impl fmt::Display for TerminalShellPickOption {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return formatter.write_str(&tr!("System default"));
        }
        formatter.write_str(
            std::path::Path::new(&self.0)
                .file_name()
                .map(|name| name.to_string_lossy())
                .unwrap_or_else(|| self.0.as_str().into())
                .as_ref(),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LanguageSettingPickOption(UiLanguageSetting);

impl fmt::Display for LanguageSettingPickOption {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&language_setting_label(self.0))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StartupLocationPickOption(StartupLocationPolicy);

impl fmt::Display for StartupLocationPickOption {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&startup_location_label(self.0))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LaunchWindowPickOption(LaunchWindowPolicy);

impl fmt::Display for LaunchWindowPickOption {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&launch_window_label(self.0))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VisibleColumnCountPickOption(usize);

impl fmt::Display for VisibleColumnCountPickOption {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&visible_column_count_label(self.0))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ColumnWidthAdjustModePickOption(ColumnWidthAdjustMode);

impl fmt::Display for ColumnWidthAdjustModePickOption {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&column_width_adjust_mode_label(self.0))
    }
}

pub(crate) fn view_settings_window(
    browser: &FileBrowser,
    window: window::Id,
) -> Element<'_, Message> {
    let frame_state = browser.window_frame_state(window);
    let shell = window_shell(&browser.user_config().window_controls, window, frame_state);
    let selected = browser.selected_settings_category;
    let sidebar = SplitPane {
        header: HeaderBar::new().title(tr!("Settings")),
        body: auxiliary_sidebar_list(
            SettingsCategory::ALL
                .into_iter()
                .map(|category| settings_category_button(category, selected).into())
                .collect(),
        ),
    };
    let content = SplitPane {
        header: settings_content_header(browser),
        body: settings_category_detail(browser),
    };
    split_window(sidebar, content, AUXILIARY_SIDEBAR_WIDTH, &shell)
}

/// 内容顶栏：标题是当前分类；二级页左边放后退，回上一级的入口和标题在同一条上。
fn settings_content_header(browser: &FileBrowser) -> HeaderBar<'static, Message> {
    match browser.settings_subpage {
        Some(SettingsSubpage::Preview) => HeaderBar::new().title(tr!("Preview settings")).start(
            icon_button(super::themed_icon(
                IconSymbol::ArrowLeft,
                IconTone::Normal,
                HEADER_ICON_SIZE,
            ))
            .on_press(Message::SettingsSubpageClosed),
        ),
        None => HeaderBar::new().title(browser.selected_settings_category.label()),
    }
}

fn settings_category_button(
    category: SettingsCategory,
    selected: SettingsCategory,
) -> Button<'static, Message> {
    auxiliary_sidebar_button(
        category.label(),
        category == selected,
        Message::SettingsCategorySelected(category),
    )
}

fn settings_category_detail(browser: &FileBrowser) -> Element<'_, Message> {
    match browser.selected_settings_category {
        SettingsCategory::General => settings_page(browser, general_settings_sections(browser)),
        SettingsCategory::Appearance => {
            settings_page(browser, appearance_settings_sections(browser))
        }
        SettingsCategory::Files if browser.settings_subpage == Some(SettingsSubpage::Preview) => {
            settings_page(browser, preview_settings_subpage_sections(browser))
        }
        SettingsCategory::Files => settings_page(browser, files_settings_sections(browser)),
        SettingsCategory::Transfer => settings_page(browser, transfer_settings_sections(browser)),
        SettingsCategory::Search => settings_page(browser, search_settings_sections(browser)),
        SettingsCategory::Shortcuts => {
            settings_page(browser, vec![shortcut_settings_section(browser)])
        }
        SettingsCategory::Logs => settings_page(browser, application_logs_sections(browser)),
        SettingsCategory::About => settings_page(browser, about_settings_sections()),
    }
}

fn general_settings_sections(browser: &FileBrowser) -> Vec<Element<'_, Message>> {
    let mut rows = vec![
        control_row(tr!("Language"), None, language_setting_dropdown(browser)),
        control_row(
            tr!("Startup location"),
            None,
            startup_location_dropdown(browser),
        ),
        control_row(
            tr!("Launch behavior"),
            None,
            launch_window_dropdown(browser),
        ),
    ];
    if browser.user_config().startup_location_policy == StartupLocationPolicy::CustomDirectory {
        rows.push(startup_custom_directory_row(browser));
    }
    rows.push(control_row(
        tr!("Terminal"),
        None,
        terminal_emulator_dropdown(browser.terminal_emulator),
    ));
    rows.push(control_row(
        tr!("Shell"),
        None,
        terminal_shell_dropdown(browser.user_config().terminal_shell.clone()),
    ));
    vec![settings_card(rows)]
}

fn appearance_settings_sections(browser: &FileBrowser) -> Vec<Element<'_, Message>> {
    vec![
        settings_group(
            tr!("Theme"),
            None,
            vec![theme_mode_row(browser), color_scheme_row(browser)],
        ),
        settings_group(
            tr!("Window controls"),
            None,
            vec![custom_row(window_control_settings_row(browser))],
        ),
        settings_group(
            tr!("Columns view"),
            None,
            vec![
                control_row(
                    tr!("Column width"),
                    None,
                    column_width_adjust_mode_dropdown(browser),
                ),
                control_row(
                    tr!("Visible columns"),
                    None,
                    visible_column_count_dropdown(browser),
                ),
            ],
        ),
        settings_group(
            tr!("Rendering"),
            None,
            vec![rendering_gpu_preference_row(
                browser.rendering_gpu_preference,
            )],
        ),
    ]
}

fn files_settings_sections(browser: &FileBrowser) -> Vec<Element<'_, Message>> {
    vec![
        settings_group(
            tr!("File display"),
            None,
            vec![
                switch_row(
                    tr!("Show Hidden Files"),
                    None,
                    browser.options.include_hidden,
                    Message::ShowHiddenFilesToggled,
                ),
                switch_row(
                    tr!("Show Recursive Folder Size In List View"),
                    None,
                    browser
                        .user_config()
                        .list_directory_size_display_mode
                        .uses_recursive_total_size(),
                    Message::ListDirectorySizeDisplayModeToggled,
                ),
            ],
        ),
        settings_group(
            tr!("Context menus"),
            None,
            vec![custom_row(context_menu_settings_section(browser))],
        ),
        preview_settings_entry_card(),
        settings_group(
            tr!("Verification"),
            None,
            vec![file_operation_verification_options(
                browser.file_operation_verification(),
            )],
        ),
        settings_group(tr!("Network"), None, vec![network_thumbnails_row(browser)]),
    ]
}

/// 传输设置页：接收开关与端口、接收目录、设备名，收藏设备。
fn transfer_settings_sections(browser: &FileBrowser) -> Vec<Element<'_, Message>> {
    vec![
        settings_group(tr!("Receive"), None, transfer_receive_rows(browser)),
        favorite_devices_group(browser),
    ]
}

fn theme_mode_row(browser: &FileBrowser) -> SettingsRow<'static> {
    let selected = browser.user_config().theme_mode;
    let choices = ThemeMode::ALL
        .into_iter()
        .map(|mode| SegmentedChoice {
            label: mode.label(),
            selected: mode == selected,
            message: Some(Message::ThemeModeSelected(mode)),
            tooltip: None,
        })
        .collect();
    choice_row(tr!("Mode"), None, choices)
}

/// 语言名用它自己的写法（同 GNOME、KDE），不随界面语言翻译。
fn language_setting_label(setting: UiLanguageSetting) -> String {
    match setting {
        UiLanguageSetting::System => tr!("Auto"),
        UiLanguageSetting::English => "English".to_owned(),
        UiLanguageSetting::Chinese => "中文".to_owned(),
    }
}

fn startup_location_label(policy: StartupLocationPolicy) -> String {
    match policy {
        StartupLocationPolicy::Home => tr!("Home directory"),
        StartupLocationPolicy::CustomDirectory => tr!("Custom directory"),
        StartupLocationPolicy::PreviousSession => tr!("Previous state"),
    }
}

fn language_setting_dropdown(browser: &FileBrowser) -> Element<'static, Message> {
    let options = [
        UiLanguageSetting::System,
        UiLanguageSetting::English,
        UiLanguageSetting::Chinese,
    ]
    .into_iter()
    .map(LanguageSettingPickOption)
    .collect::<Vec<_>>();

    dropdown(
        options,
        Some(LanguageSettingPickOption(
            browser.user_config().language_setting,
        )),
        |selected| Message::LanguageSettingSelected(selected.0),
    )
    .width(Length::Fixed(SETTINGS_DROPDOWN_WIDTH))
    .into()
}

fn startup_location_dropdown(browser: &FileBrowser) -> Element<'static, Message> {
    let options = [
        StartupLocationPolicy::Home,
        StartupLocationPolicy::CustomDirectory,
        StartupLocationPolicy::PreviousSession,
    ]
    .into_iter()
    .map(StartupLocationPickOption)
    .collect::<Vec<_>>();

    dropdown(
        options,
        Some(StartupLocationPickOption(
            browser.user_config().startup_location_policy,
        )),
        |selected| Message::StartupLocationPolicySelected(selected.0),
    )
    .width(Length::Fixed(SETTINGS_DROPDOWN_WIDTH))
    .into()
}

fn launch_window_label(policy: LaunchWindowPolicy) -> String {
    match policy {
        LaunchWindowPolicy::MergeIntoExisting => tr!("Merge into existing window"),
        LaunchWindowPolicy::OpenNewWindow => tr!("Open a new window"),
    }
}

fn visible_column_count_label(count: usize) -> String {
    tr!("%1 columns", count)
}

fn column_width_adjust_mode_label(mode: ColumnWidthAdjustMode) -> String {
    match mode {
        ColumnWidthAdjustMode::Uniform => tr!("All columns together"),
        ColumnWidthAdjustMode::PerColumn => tr!("Adjust each column separately"),
    }
}

fn column_width_adjust_mode_dropdown(browser: &FileBrowser) -> Element<'static, Message> {
    let options = [
        ColumnWidthAdjustMode::PerColumn,
        ColumnWidthAdjustMode::Uniform,
    ]
    .into_iter()
    .map(ColumnWidthAdjustModePickOption)
    .collect::<Vec<_>>();

    dropdown(
        options,
        Some(ColumnWidthAdjustModePickOption(
            browser.user_config().column_width_adjust_mode,
        )),
        |selected| Message::ColumnWidthAdjustModeSelected(selected.0),
    )
    .width(Length::Fixed(SETTINGS_DROPDOWN_WIDTH))
    .into()
}

fn launch_window_dropdown(browser: &FileBrowser) -> Element<'static, Message> {
    let options = [
        LaunchWindowPolicy::MergeIntoExisting,
        LaunchWindowPolicy::OpenNewWindow,
    ]
    .into_iter()
    .map(LaunchWindowPickOption)
    .collect::<Vec<_>>();

    dropdown(
        options,
        Some(LaunchWindowPickOption(
            browser.user_config().launch_window_policy,
        )),
        |selected| Message::LaunchWindowPolicySelected(selected.0),
    )
    .width(Length::Fixed(SETTINGS_DROPDOWN_WIDTH))
    .into()
}

fn visible_column_count_dropdown(browser: &FileBrowser) -> Element<'static, Message> {
    let options = [3, 4, 5]
        .into_iter()
        .map(VisibleColumnCountPickOption)
        .collect::<Vec<_>>();

    dropdown(
        options,
        Some(VisibleColumnCountPickOption(
            browser.user_config().visible_column_count,
        )),
        |selected| Message::VisibleColumnCountSelected(selected.0),
    )
    .width(Length::Fixed(SETTINGS_DROPDOWN_WIDTH))
    .into()
}

fn terminal_emulator_dropdown(selected: TerminalEmulator) -> Element<'static, Message> {
    let options = TERMINAL_EMULATOR_OPTIONS
        .iter()
        .copied()
        .map(TerminalEmulatorPickOption)
        .collect::<Vec<_>>();

    dropdown(
        options,
        Some(TerminalEmulatorPickOption(selected)),
        |selected| Message::TerminalEmulatorSelected(selected.0),
    )
    .width(Length::Fixed(SETTINGS_DROPDOWN_WIDTH))
    .into()
}

fn terminal_shell_dropdown(selected: String) -> Element<'static, Message> {
    let mut shells = vec![String::new()];
    shells.extend(
        crate::terminal_panel::session::available_shells()
            .iter()
            .cloned(),
    );
    // 手写进配置文件的路径不在 /etc/shells 里也要能显示选中态。
    if !selected.is_empty() && !shells.contains(&selected) {
        shells.push(selected.clone());
    }
    let options: Vec<TerminalShellPickOption> =
        shells.into_iter().map(TerminalShellPickOption).collect();

    dropdown(
        options,
        Some(TerminalShellPickOption(selected)),
        |selected| Message::TerminalShellSelected(selected.0),
    )
    .width(Length::Fixed(SETTINGS_DROPDOWN_WIDTH))
    .into()
}

fn startup_custom_directory_row(browser: &FileBrowser) -> SettingsRow<'_> {
    entry_row(EntryRow {
        title: tr!("Custom Startup Directory"),
        saved: browser
            .user_config()
            .startup_custom_directory
            .to_string_lossy()
            .into_owned(),
        draft: &browser.startup_custom_directory_input,
        placeholder: tr!("Directory"),
        on_input: Box::new(Message::StartupCustomDirectoryInputChanged),
        submit: Message::StartupCustomDirectoryCommitted,
        note: browser
            .startup_custom_directory_error
            .clone()
            .map(RowNote::Error),
        unit: None,
        trailing: Vec::new(),
    })
}
