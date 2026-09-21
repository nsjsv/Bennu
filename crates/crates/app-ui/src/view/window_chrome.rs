use iced::alignment::{Horizontal, Vertical};
use iced::widget::{button, container, mouse_area, tooltip, Column, Row, Space, Stack};
use iced::{mouse, window, Background, Element, Length};

use crate::appearance::{
    context_menu_style, floating_window_close_button_style, floating_window_control_button_style,
    preview_window_top_gradient_style, window_close_button_style, window_control_button_style,
    window_title_bar_style, window_top_bar_style,
};
use crate::icons::IconSymbol;
use crate::matugen_theme::ui_colors;
use crate::model::{
    Message, PreviewWindowChromeState, WindowChromeLayout, WindowControlKind, WindowControlSide,
    WindowControlsConfig, WindowFrameState, WINDOW_TITLE_BAR_HEIGHT, WINDOW_TOP_BAR_HEIGHT,
};

use super::{themed_icon, window_drag_region::window_drag_region, IconTone};
use crate::typography::{localized_text, readable_text};

const WINDOW_CONTROL_WIDTH: f32 = 36.0;
const WINDOW_CONTROL_HEIGHT: f32 = 32.0;
const WINDOW_CONTROL_ICON_SIZE: f32 = 12.0;
const WINDOW_CONTROL_VERTICAL_PADDING: f32 =
    (WINDOW_CONTROL_HEIGHT - WINDOW_CONTROL_ICON_SIZE) / 2.0;
const WINDOW_CONTROL_HORIZONTAL_PADDING: f32 =
    (WINDOW_CONTROL_WIDTH - WINDOW_CONTROL_ICON_SIZE) / 2.0;
const WINDOW_CONTROL_SPACING: f32 = 2.0;
const WINDOW_TITLE_SIDE_RESERVE: u16 = 116;
const WINDOW_RESIZE_EDGE_WIDTH: f32 = 5.0;
const WINDOW_RESIZE_CORNER_WIDTH: f32 = 11.0;
const STACKED_PANE_NAVIGATION_MAX_WIDTH: f32 = 500.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PaneNavigationLayout {
    SingleRow,
    StackedRows,
}

pub(crate) fn pane_navigation_layout(main_window_width: f32) -> PaneNavigationLayout {
    // 工具栏横贯整个窗口宽度,侧栏与预览面板都在它下方,不再参与
    // 宽度预算;只有窗口本身过窄时才折行。
    if main_window_width < STACKED_PANE_NAVIGATION_MAX_WIDTH {
        PaneNavigationLayout::StackedRows
    } else {
        PaneNavigationLayout::SingleRow
    }
}

/// 全窗顶栏的 chrome 形态:集成导航时顶栏两端承载窗口控制并作为
/// 拖拽区;独立标题栏布局下控制归标题栏,顶栏只承载工具栏本体。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MainPaneWindowChromeRole {
    Complete,
    NoChrome,
}

impl MainPaneWindowChromeRole {
    pub(crate) fn shows_left_controls(self) -> bool {
        matches!(self, Self::Complete)
    }

    pub(crate) fn shows_right_controls(self) -> bool {
        matches!(self, Self::Complete)
    }

    pub(crate) fn owns_window_drag_region(self) -> bool {
        self != Self::NoChrome
    }
}

#[derive(Debug, Clone, Copy)]
enum WindowControlPresentation {
    Standard,
    Floating { opacity: f32 },
}

pub(crate) fn window_control_group(
    config: &WindowControlsConfig,
    side: WindowControlSide,
    window: window::Id,
    frame_state: WindowFrameState,
) -> Element<'static, Message> {
    window_control_group_with_presentation(
        config,
        side,
        window,
        frame_state,
        WindowControlPresentation::Standard,
    )
}

pub(crate) fn floating_window_control_group(
    config: &WindowControlsConfig,
    side: WindowControlSide,
    window: window::Id,
    frame_state: WindowFrameState,
    opacity: f32,
) -> Element<'static, Message> {
    window_control_group_with_presentation(
        config,
        side,
        window,
        frame_state,
        WindowControlPresentation::Floating {
            opacity: opacity.clamp(0.0, 1.0),
        },
    )
}

fn window_control_group_with_presentation(
    config: &WindowControlsConfig,
    side: WindowControlSide,
    window: window::Id,
    frame_state: WindowFrameState,
    presentation: WindowControlPresentation,
) -> Element<'static, Message> {
    let mut controls = Row::new()
        .spacing(WINDOW_CONTROL_SPACING)
        .height(Length::Fixed(WINDOW_CONTROL_HEIGHT));
    for placement in config
        .placements_on(side)
        .filter(|placement| placement.visibility().is_visible())
    {
        controls = controls.push(window_control_button(
            placement.kind(),
            window,
            frame_state,
            presentation,
        ));
    }
    controls.into()
}

fn window_control_button(
    kind: WindowControlKind,
    window: window::Id,
    frame_state: WindowFrameState,
    presentation: WindowControlPresentation,
) -> Element<'static, Message> {
    let (icon, label, message) = match kind {
        WindowControlKind::Minimize => (
            IconSymbol::Minus,
            "Minimize",
            Message::WindowMinimizeRequested(window),
        ),
        WindowControlKind::MaximizeRestore => match frame_state {
            WindowFrameState::Restored => (
                IconSymbol::Square,
                "Maximize",
                Message::WindowMaximizeToggled(window),
            ),
            WindowFrameState::Maximized => (
                IconSymbol::RestoreWindow,
                "Restore",
                Message::WindowMaximizeToggled(window),
            ),
        },
        WindowControlKind::Close => (
            IconSymbol::Close,
            "Close",
            Message::AuxiliaryWindowCloseRequested(window),
        ),
    };
    let (style, opacity): (fn(&iced::Theme, button::Status) -> button::Style, f32) =
        match (presentation, kind) {
            (WindowControlPresentation::Floating { opacity }, WindowControlKind::Close) => {
                (floating_window_close_button_style, opacity)
            }
            (WindowControlPresentation::Floating { opacity }, _) => {
                (floating_window_control_button_style, opacity)
            }
            (WindowControlPresentation::Standard, WindowControlKind::Close) => {
                (window_close_button_style, 1.0)
            }
            (WindowControlPresentation::Standard, _) => (window_control_button_style, 1.0),
        };
    let control =
        button(themed_icon(icon, IconTone::Normal, WINDOW_CONTROL_ICON_SIZE).opacity(opacity))
            .on_press(message)
            .padding([
                WINDOW_CONTROL_VERTICAL_PADDING,
                WINDOW_CONTROL_HORIZONTAL_PADDING,
            ])
            .width(Length::Fixed(WINDOW_CONTROL_WIDTH))
            .height(Length::Fixed(WINDOW_CONTROL_HEIGHT))
            .style(move |theme, status| {
                let mut style = style(theme, status);
                style.background = style
                    .background
                    .map(|background| background.scale_alpha(opacity));
                style.text_color = style.text_color.scale_alpha(opacity);
                style
            });

    tooltip(
        control,
        container(readable_text(label).size(11))
            .padding([5, 7])
            .style(context_menu_style),
        tooltip::Position::Bottom,
    )
    .into()
}

pub(crate) fn auxiliary_window_content<'a>(
    integrated_title: &'static str,
    separate_title: String,
    content: Element<'a, Message>,
    config: &WindowControlsConfig,
    window: window::Id,
    frame_state: WindowFrameState,
    preview_pin: Option<bool>,
) -> Element<'a, Message> {
    match config.layout() {
        WindowChromeLayout::IntegratedNavigation => window_content_with_top_bar(
            integrated_title,
            content,
            config,
            window,
            frame_state,
            preview_pin,
        ),
        WindowChromeLayout::SeparateTitleBar => separate_window_content_with_height(
            separate_title,
            content,
            config,
            window,
            frame_state,
            WINDOW_TOP_BAR_HEIGHT,
            preview_pin,
        ),
    }
}
pub(crate) fn floating_preview_window_content<'a>(
    content: Element<'a, Message>,
    config: &WindowControlsConfig,
    window: window::Id,
    frame_state: WindowFrameState,
    chrome_opacity: f32,
    pinned: bool,
) -> Element<'a, Message> {
    let chrome_opacity = chrome_opacity.clamp(0.0, 1.0);
    let top_bar: Element<'a, Message> = if chrome_opacity > f32::EPSILON {
        let controls = Row::new()
            .spacing(10)
            .padding(iced::Padding {
                top: 6.0,
                right: 8.0,
                bottom: 10.0,
                left: 8.0,
            })
            .align_y(iced::Alignment::Center)
            .push(floating_window_control_group(
                config,
                WindowControlSide::Left,
                window,
                frame_state,
                chrome_opacity,
            ))
            .push(preview_pin_button(
                pinned,
                WindowControlPresentation::Floating {
                    opacity: chrome_opacity,
                },
            ))
            .push(Space::new().width(Length::Fill))
            .push(floating_window_control_group(
                config,
                WindowControlSide::Right,
                window,
                frame_state,
                chrome_opacity,
            ))
            .width(Length::Fill)
            .height(Length::Fixed(PreviewWindowChromeState::REVEAL_HEIGHT));
        let gradient = container(Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |theme| preview_window_top_gradient_style(theme, chrome_opacity));

        Stack::with_children([gradient.into(), controls.into()])
            .width(Length::Fill)
            .height(Length::Fixed(PreviewWindowChromeState::REVEAL_HEIGHT))
            .into()
    } else {
        container(Space::new())
            .width(Length::Fill)
            .height(Length::Fixed(PreviewWindowChromeState::REVEAL_HEIGHT))
            .into()
    };
    let drag_surface = window_drag_region(top_bar, window);

    Stack::with_children([content, drag_surface])
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn window_content_with_top_bar<'a>(
    title: &'static str,
    content: Element<'a, Message>,
    config: &WindowControlsConfig,
    window: window::Id,
    frame_state: WindowFrameState,
    preview_pin: Option<bool>,
) -> Element<'a, Message> {
    Column::new()
        .spacing(0)
        .push(window_top_bar(
            title,
            config,
            window,
            frame_state,
            preview_pin,
        ))
        .push(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn window_top_bar(
    title: &'static str,
    config: &WindowControlsConfig,
    window: window::Id,
    frame_state: WindowFrameState,
    preview_pin: Option<bool>,
) -> Element<'static, Message> {
    let drag_surface = container(Space::new())
        .width(Length::Fill)
        .height(Length::Fixed(WINDOW_TOP_BAR_HEIGHT))
        .style(window_top_bar_style);
    let drag_surface = window_drag_region(drag_surface.into(), window);
    let mut content = Row::new()
        .spacing(10)
        .padding([8, 12])
        .align_y(iced::Alignment::Center)
        .push(window_control_group(
            config,
            WindowControlSide::Left,
            window,
            frame_state,
        ));
    // 固定按钮放标题旁边：紧跟左侧控制组，位于标题左侧。
    if let Some(pinned) = preview_pin {
        content = content.push(preview_pin_button(
            pinned,
            WindowControlPresentation::Standard,
        ));
    }
    let content = content
        .push(localized_text(title).size(16))
        .push(Space::new().width(Length::Fill))
        .push(window_control_group(
            config,
            WindowControlSide::Right,
            window,
            frame_state,
        ))
        .width(Length::Fill)
        .height(Length::Fixed(WINDOW_TOP_BAR_HEIGHT));

    Stack::with_children([drag_surface, content.into()])
        .width(Length::Fill)
        .height(Length::Fixed(WINDOW_TOP_BAR_HEIGHT))
        .into()
}

pub(crate) fn separate_window_content<'a>(
    title: String,
    content: Element<'a, Message>,
    config: &WindowControlsConfig,
    window: window::Id,
    frame_state: WindowFrameState,
) -> Element<'a, Message> {
    separate_window_content_with_height(
        title,
        content,
        config,
        window,
        frame_state,
        WINDOW_TITLE_BAR_HEIGHT,
        None,
    )
}

fn separate_window_content_with_height<'a>(
    title: String,
    content: Element<'a, Message>,
    config: &WindowControlsConfig,
    window: window::Id,
    frame_state: WindowFrameState,
    height: f32,
    preview_pin: Option<bool>,
) -> Element<'a, Message> {
    Column::new()
        .spacing(0)
        .push(separate_window_title_bar(
            title,
            config,
            window,
            frame_state,
            height,
            preview_pin,
        ))
        .push(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn separate_window_title_bar(
    title: String,
    config: &WindowControlsConfig,
    window: window::Id,
    frame_state: WindowFrameState,
    height: f32,
    preview_pin: Option<bool>,
) -> Element<'static, Message> {
    let drag_surface = container(Space::new())
        .width(Length::Fill)
        .height(Length::Fixed(height))
        .style(window_title_bar_style);
    let drag_surface = window_drag_region(drag_surface.into(), window);
    let title = container(localized_text(title).size(13))
        .padding([0, WINDOW_TITLE_SIDE_RESERVE])
        .center_x(Length::Fill)
        .center_y(Length::Fixed(height))
        .clip(true);
    let mut controls = Row::new()
        .spacing(0)
        .padding([4, 4])
        .align_y(iced::Alignment::Center)
        .push(window_control_group(
            config,
            WindowControlSide::Left,
            window,
            frame_state,
        ));
    // 标题居中覆盖；固定按钮紧跟左侧控制组，落在标题左侧。
    if let Some(pinned) = preview_pin {
        controls = controls.push(preview_pin_button(
            pinned,
            WindowControlPresentation::Standard,
        ));
    }
    let controls = controls
        .push(Space::new().width(Length::Fill))
        .push(window_control_group(
            config,
            WindowControlSide::Right,
            window,
            frame_state,
        ))
        .width(Length::Fill)
        .height(Length::Fixed(height));

    Stack::with_children([drag_surface, title.into(), controls.into()])
        .width(Length::Fill)
        .height(Length::Fixed(height))
        .into()
}

// 预览固定按钮：固定时用主题 primary 高亮，点击发送 PreviewWindowPinToggled。
fn preview_pin_button(
    pinned: bool,
    presentation: WindowControlPresentation,
) -> Element<'static, Message> {
    let (base_style, opacity): (fn(&iced::Theme, button::Status) -> button::Style, f32) =
        match presentation {
            WindowControlPresentation::Floating { opacity } => (
                floating_window_control_button_style,
                opacity.clamp(0.0, 1.0),
            ),
            WindowControlPresentation::Standard => (window_control_button_style, 1.0),
        };
    let control = button(
        themed_icon(IconSymbol::Pin, IconTone::Normal, WINDOW_CONTROL_ICON_SIZE).opacity(opacity),
    )
    .on_press(Message::PreviewWindowPinToggled)
    .padding([
        WINDOW_CONTROL_VERTICAL_PADDING,
        WINDOW_CONTROL_HORIZONTAL_PADDING,
    ])
    .width(Length::Fixed(WINDOW_CONTROL_WIDTH))
    .height(Length::Fixed(WINDOW_CONTROL_HEIGHT))
    .style(move |theme, status| {
        let mut style = base_style(theme, status);
        if pinned {
            let colors = ui_colors(theme);
            style.background = Some(Background::Color(colors.primary));
            style.text_color = colors.on_primary;
        }
        style.background = style
            .background
            .map(|background| background.scale_alpha(opacity));
        style.text_color = style.text_color.scale_alpha(opacity);
        style
    });

    tooltip(
        control,
        container(readable_text(if pinned { "Unpin" } else { "Pin" }).size(11))
            .padding([5, 7])
            .style(context_menu_style),
        tooltip::Position::Bottom,
    )
    .into()
}

pub(crate) fn window_resize_frame<'a>(
    content: Element<'a, Message>,
    window: window::Id,
    frame_state: WindowFrameState,
) -> Element<'a, Message> {
    if frame_state == WindowFrameState::Maximized {
        return content;
    }

    let mut frame = Stack::with_children([content])
        .width(Length::Fill)
        .height(Length::Fill);
    for direction in [
        window::Direction::North,
        window::Direction::South,
        window::Direction::East,
        window::Direction::West,
        window::Direction::NorthEast,
        window::Direction::NorthWest,
        window::Direction::SouthEast,
        window::Direction::SouthWest,
    ] {
        frame = frame.push(window_resize_zone(window, direction));
    }
    frame.into()
}

fn window_resize_zone(
    window: window::Id,
    direction: window::Direction,
) -> Element<'static, Message> {
    let (width, height, horizontal, vertical, interaction) = match direction {
        window::Direction::North => (
            Length::Fill,
            Length::Fixed(WINDOW_RESIZE_EDGE_WIDTH),
            Horizontal::Center,
            Vertical::Top,
            mouse::Interaction::ResizingVertically,
        ),
        window::Direction::South => (
            Length::Fill,
            Length::Fixed(WINDOW_RESIZE_EDGE_WIDTH),
            Horizontal::Center,
            Vertical::Bottom,
            mouse::Interaction::ResizingVertically,
        ),
        window::Direction::East => (
            Length::Fixed(WINDOW_RESIZE_EDGE_WIDTH),
            Length::Fill,
            Horizontal::Right,
            Vertical::Center,
            mouse::Interaction::ResizingHorizontally,
        ),
        window::Direction::West => (
            Length::Fixed(WINDOW_RESIZE_EDGE_WIDTH),
            Length::Fill,
            Horizontal::Left,
            Vertical::Center,
            mouse::Interaction::ResizingHorizontally,
        ),
        window::Direction::NorthEast => (
            Length::Fixed(WINDOW_RESIZE_CORNER_WIDTH),
            Length::Fixed(WINDOW_RESIZE_CORNER_WIDTH),
            Horizontal::Right,
            Vertical::Top,
            mouse::Interaction::ResizingDiagonallyUp,
        ),
        window::Direction::NorthWest => (
            Length::Fixed(WINDOW_RESIZE_CORNER_WIDTH),
            Length::Fixed(WINDOW_RESIZE_CORNER_WIDTH),
            Horizontal::Left,
            Vertical::Top,
            mouse::Interaction::ResizingDiagonallyDown,
        ),
        window::Direction::SouthEast => (
            Length::Fixed(WINDOW_RESIZE_CORNER_WIDTH),
            Length::Fixed(WINDOW_RESIZE_CORNER_WIDTH),
            Horizontal::Right,
            Vertical::Bottom,
            mouse::Interaction::ResizingDiagonallyDown,
        ),
        window::Direction::SouthWest => (
            Length::Fixed(WINDOW_RESIZE_CORNER_WIDTH),
            Length::Fixed(WINDOW_RESIZE_CORNER_WIDTH),
            Horizontal::Left,
            Vertical::Bottom,
            mouse::Interaction::ResizingDiagonallyUp,
        ),
    };
    let target = mouse_area(Space::new().width(width).height(height))
        .on_press(Message::WindowResizeRequested(window, direction))
        .interaction(interaction);

    container(target)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(horizontal)
        .align_y(vertical)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_navigation_stacks_only_when_window_is_narrow() {
        // 顶栏横贯全窗后,折行只取决于窗口自身宽度;侧栏/面板在顶栏
        // 下方,不再参与宽度预算。
        assert_eq!(
            pane_navigation_layout(499.0),
            PaneNavigationLayout::StackedRows
        );
        assert_eq!(
            pane_navigation_layout(500.0),
            PaneNavigationLayout::SingleRow
        );
    }
}
