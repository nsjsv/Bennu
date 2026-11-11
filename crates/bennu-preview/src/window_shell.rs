//! 窗口外壳：全部窗口（主窗口、设置、属性、传输、LocalSend、预览、portal 对话框）
//! 只从这里的两种骨架里选一种——只有一块内容的 [`single_window`]，侧栏 + 内容的
//! [`split_window`]。标题和窗口按钮都在窗口自己的顶栏里，不用系统标题栏。
//!
//! 窗口按钮的位置只在这里决定：靠左的按钮放在最左那条顶栏的最前面，靠右的放在
//! 最右那条顶栏的最后面。调用方不自己拼按钮。窗口管理（按钮、拖动、缩放的消息）
//! 属于宿主进程，经 [`WindowShell`] 注入。

mod title_drag;

use iced::widget::{column, container, mouse_area, row, Row, Space, Stack};
use iced::{window, Background, Border, Element, Length, Padding, Theme};

use bennu_theme::metrics::CARD_RADIUS;
use bennu_theme::window_controls::{
    SidebarStyle, WindowControlKind, WindowControlSide, WindowControlsConfig, WindowFrameState,
};
use bennu_theme::{typography::BOLD, ui_colors};

use crate::panel_text::readable_text;
use crate::preview_window_chrome::{window_control_group, WINDOW_CONTROL_SPACING};
use bennu_theme::window_chrome_styles::WINDOW_CONTROL_SIZE;

/// 顶栏高度：32 高的控件上下各留 8。
pub const HEADER_BAR_HEIGHT: f32 = 48.0;
const HEADER_PADDING_X: f32 = 8.0;
const HEADER_ITEM_SPACING: f32 = 6.0;
/// 左右区里一个普通项（图标按钮）按这个宽度给标题让位。
const HEADER_ITEM_WIDTH: f32 = 32.0;
const DIVIDER_WIDTH: f32 = 1.0;
/// 悬浮侧栏和窗口边缘之间的空；和内容区之间只留一半，卡片离内容更近。
const FLOATING_SIDEBAR_GAP: f32 = 8.0;
const RESIZE_EDGE_WIDTH: f32 = 5.0;
const RESIZE_CORNER_WIDTH: f32 = 11.0;

/// 宿主注入的窗口管理：按钮配置与最大化状态，以及按钮、顶栏、边缘发出的消息。
pub struct WindowShell<'a, Message> {
    pub config: &'a WindowControlsConfig,
    pub sidebar_style: SidebarStyle,
    pub window: window::Id,
    pub frame_state: WindowFrameState,
    pub on_control: Box<dyn Fn(WindowControlKind, window::Id) -> Message + 'a>,
    /// 按下顶栏空白处：宿主发起原生拖动。
    pub title_drag: Message,
    /// 双击顶栏空白处：宿主切换最大化。
    pub title_double_click: Message,
    /// 按下窗口边缘 / 拐角：宿主发起原生缩放。
    pub on_resize: Box<dyn Fn(window::Direction) -> Message + 'a>,
}

/// 一条顶栏：左区、居中标题、右区。
pub struct HeaderBar<'a, Message> {
    start: Vec<Element<'a, Message>>,
    title: Option<String>,
    /// 占满左右区之间的整段（主窗口工具栏）；有它时不画标题。
    center: Option<Element<'a, Message>>,
    end: Vec<Element<'a, Message>>,
    /// 折成两行时的第二行：在窗口按钮下面，横跨整条顶栏。
    below: Option<Element<'a, Message>>,
    height: f32,
}

impl<'a, Message> HeaderBar<'a, Message> {
    pub fn new() -> Self {
        Self {
            start: Vec::new(),
            title: None,
            center: None,
            end: Vec::new(),
            below: None,
            height: HEADER_BAR_HEIGHT,
        }
    }

    pub fn title(mut self, title: String) -> Self {
        self.title = Some(title);
        self
    }

    pub fn center(mut self, content: impl Into<Element<'a, Message>>) -> Self {
        self.center = Some(content.into());
        self
    }

    pub fn start(mut self, item: impl Into<Element<'a, Message>>) -> Self {
        self.start.push(item.into());
        self
    }

    pub fn end(mut self, item: impl Into<Element<'a, Message>>) -> Self {
        self.end.push(item.into());
        self
    }

    /// 第一行（按钮、`center`）留在最上面一条，窗口按钮也在那条里；第二行从它下面开始，
    /// 不用给窗口按钮让位。顶栏总高仍由 `height` 定，第二行在剩下的高度里靠上。
    pub fn below(mut self, row: impl Into<Element<'a, Message>>) -> Self {
        self.below = Some(row.into());
        self
    }

    /// 主窗口窄时工具栏折成两行，顶栏跟着变高。
    pub fn height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }
}

impl<Message> Default for HeaderBar<'_, Message> {
    fn default() -> Self {
        Self::new()
    }
}

/// split 窗口里的一栏：顶栏 + 下面的内容。
pub struct SplitPane<'a, Message> {
    pub header: HeaderBar<'a, Message>,
    pub body: Element<'a, Message>,
}

/// 这条顶栏碰到窗口的哪条竖边：决定靠左 / 靠右的窗口按钮放不放进来。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeaderEdges {
    Both,
    Leading,
    Trailing,
}

impl HeaderEdges {
    fn controls_sides(self) -> (bool, bool) {
        match self {
            Self::Both => (true, true),
            Self::Leading => (true, false),
            Self::Trailing => (false, true),
        }
    }
}

pub fn single_window<'a, Message>(
    header: HeaderBar<'a, Message>,
    body: impl Into<Element<'a, Message>>,
    shell: &WindowShell<'a, Message>,
) -> Element<'a, Message>
where
    Message: Clone + 'static,
{
    resize_frame(
        container(column![
            header_bar(header, HeaderEdges::Both, shell),
            body.into()
        ])
        .width(Length::Fill)
        .height(Length::Fill)
        .style(content_surface_style)
        .into(),
        shell,
    )
}

/// 侧栏贴左边从顶到底，侧栏顶栏 + 内容顶栏两条，中间一条细分隔线。
pub fn split_window<'a, Message>(
    sidebar: SplitPane<'a, Message>,
    content: SplitPane<'a, Message>,
    sidebar_width: f32,
    shell: &WindowShell<'a, Message>,
) -> Element<'a, Message>
where
    Message: Clone + 'static,
{
    // 两种样式都让内容区从 sidebar_width 开始（分隔线、卡片四周的空都算在侧栏宽度里），
    // 宿主按这个宽度算的命中区域不偏。
    let sidebar_column = |header: HeaderBar<'a, Message>| {
        container(column![
            header_bar(header, HeaderEdges::Leading, shell),
            sidebar.body,
        ])
        .width(Length::Fill)
        .height(Length::Fill)
        .clip(true)
    };
    let sidebar: Element<'a, Message> = match shell.sidebar_style {
        SidebarStyle::Embedded => row![
            sidebar_column(sidebar.header).style(sidebar_surface_style),
            container(Space::new())
                .width(Length::Fixed(DIVIDER_WIDTH))
                .height(Length::Fill)
                .style(divider_style),
        ]
        .width(Length::Fixed(sidebar_width))
        .into(),
        SidebarStyle::Floating => {
            // 卡片的顶栏少一段上边的空，和内容顶栏下沿对齐。
            let mut header = sidebar.header;
            header.height = (header.height - FLOATING_SIDEBAR_GAP).max(0.0);
            container(sidebar_column(header).style(floating_sidebar_style))
                .padding(Padding {
                    top: FLOATING_SIDEBAR_GAP,
                    right: FLOATING_SIDEBAR_GAP / 2.0,
                    bottom: FLOATING_SIDEBAR_GAP,
                    left: FLOATING_SIDEBAR_GAP,
                })
                .width(Length::Fixed(sidebar_width))
                .height(Length::Fill)
                .into()
        }
    };
    // 两栏各自裁到自己的范围：内容区横向滚出去的东西（多栏视图左边的栏）不会画到侧栏上。
    let content = container(column![
        header_bar(content.header, HeaderEdges::Trailing, shell),
        content.body,
    ])
    .width(Length::Fill)
    .height(Length::Fill)
    .clip(true)
    .style(content_surface_style);

    resize_frame(
        // 底色垫满整窗：悬浮侧栏四周的空露出的是内容区的底色。
        container(row![sidebar, content])
            .width(Length::Fill)
            .height(Length::Fill)
            .style(content_surface_style)
            .into(),
        shell,
    )
}

const RESIZE_DIRECTIONS: [window::Direction; 8] = [
    window::Direction::North,
    window::Direction::South,
    window::Direction::East,
    window::Direction::West,
    window::Direction::NorthEast,
    window::Direction::NorthWest,
    window::Direction::SouthEast,
    window::Direction::SouthWest,
];

/// 没有系统边框，缩放靠窗口四边四角的命中区；最大化时不画。两种骨架都必须经过这里，
/// 否则那类窗口拖边缘没反应。
fn resize_frame<'a, Message>(
    content: Element<'a, Message>,
    shell: &WindowShell<'a, Message>,
) -> Element<'a, Message>
where
    Message: Clone + 'static,
{
    if shell.frame_state == WindowFrameState::Maximized {
        return content;
    }
    let mut frame = Stack::with_children([content])
        .width(Length::Fill)
        .height(Length::Fill);
    for direction in RESIZE_DIRECTIONS {
        frame = frame.push(resize_zone(direction, (shell.on_resize)(direction)));
    }
    frame.into()
}

fn resize_zone<'a, Message: Clone + 'a>(
    direction: window::Direction,
    message: Message,
) -> Element<'a, Message> {
    use iced::alignment::{Horizontal, Vertical};
    use iced::mouse::Interaction;
    let edge = Length::Fixed(RESIZE_EDGE_WIDTH);
    let corner = Length::Fixed(RESIZE_CORNER_WIDTH);
    let (width, height, horizontal, vertical, interaction) = match direction {
        window::Direction::North => (
            Length::Fill,
            edge,
            Horizontal::Center,
            Vertical::Top,
            Interaction::ResizingVertically,
        ),
        window::Direction::South => (
            Length::Fill,
            edge,
            Horizontal::Center,
            Vertical::Bottom,
            Interaction::ResizingVertically,
        ),
        window::Direction::East => (
            edge,
            Length::Fill,
            Horizontal::Right,
            Vertical::Center,
            Interaction::ResizingHorizontally,
        ),
        window::Direction::West => (
            edge,
            Length::Fill,
            Horizontal::Left,
            Vertical::Center,
            Interaction::ResizingHorizontally,
        ),
        window::Direction::NorthEast => (
            corner,
            corner,
            Horizontal::Right,
            Vertical::Top,
            Interaction::ResizingDiagonallyUp,
        ),
        window::Direction::NorthWest => (
            corner,
            corner,
            Horizontal::Left,
            Vertical::Top,
            Interaction::ResizingDiagonallyDown,
        ),
        window::Direction::SouthEast => (
            corner,
            corner,
            Horizontal::Right,
            Vertical::Bottom,
            Interaction::ResizingDiagonallyDown,
        ),
        window::Direction::SouthWest => (
            corner,
            corner,
            Horizontal::Left,
            Vertical::Bottom,
            Interaction::ResizingDiagonallyUp,
        ),
    };
    container(
        mouse_area(Space::new().width(width).height(height))
            .on_press(message)
            .interaction(interaction),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .align_x(horizontal)
    .align_y(vertical)
    .into()
}

fn header_bar<'a, Message>(
    header: HeaderBar<'a, Message>,
    edges: HeaderEdges,
    shell: &WindowShell<'a, Message>,
) -> Element<'a, Message>
where
    Message: Clone + 'static,
{
    let HeaderBar {
        start,
        title,
        center,
        end,
        below,
        height,
    } = header;
    let (leading_controls, trailing_controls) = edges.controls_sides();
    let band = HEADER_BAR_HEIGHT.min(height);
    // 标题绝对居中，两边按较宽的一侧让位，左右区的按钮不会压到标题上。
    let side_width = |item_count: usize, controls: Option<WindowControlSide>| {
        let control_count = controls.map_or(0, |side| visible_control_count(shell, side));
        item_count as f32 * (HEADER_ITEM_WIDTH + HEADER_ITEM_SPACING)
            + control_count as f32 * (WINDOW_CONTROL_SIZE + WINDOW_CONTROL_SPACING)
    };
    let title_reserve = HEADER_PADDING_X
        + side_width(
            start.len(),
            leading_controls.then_some(WindowControlSide::Left),
        )
        .max(side_width(
            end.len(),
            trailing_controls.then_some(WindowControlSide::Right),
        ));

    // 顶栏比标准高（主窗口工具栏折两行）时，两边的按钮和标题留在最上面一条，
    // 跟第一行对齐；没有第二行时中间的内容占满整个高度。
    let mut items = Row::new()
        .spacing(HEADER_ITEM_SPACING)
        .padding([0.0, HEADER_PADDING_X]);
    if leading_controls {
        items = items.push(top_band(controls(shell, WindowControlSide::Left), band));
    }
    for item in start {
        items = items.push(top_band(item, band));
    }
    let title = match center {
        Some(center) => {
            items = items.push(container(center).center_y(Length::Fill));
            None
        }
        None => {
            items = items.push(Space::new().width(Length::Fill));
            title
        }
    };
    for item in end {
        items = items.push(top_band(item, band));
    }
    if trailing_controls {
        items = items.push(top_band(controls(shell, WindowControlSide::Right), band));
    }
    let items: Element<'a, Message> = match below {
        Some(below) => column![
            items.width(Length::Fill).height(Length::Fixed(band)),
            container(below)
                .padding([0.0, HEADER_PADDING_X])
                .width(Length::Fill)
                .height(Length::Fill),
        ]
        .height(Length::Fixed(height))
        .into(),
        None => items
            .width(Length::Fill)
            .height(Length::Fixed(height))
            .into(),
    };

    // 拖动面垫在最底下：左右区和标题不捕获的空白处才会开始拖动窗口。
    let drag_surface = title_drag::title_drag_surface(
        height,
        shell.title_drag.clone(),
        shell.title_double_click.clone(),
    );
    let mut layers = vec![drag_surface];
    if let Some(title) = title {
        layers.push(
            container(
                readable_text(title)
                    .font(BOLD)
                    .wrapping(iced::widget::text::Wrapping::None),
            )
            .padding([0.0, title_reserve])
            .center_x(Length::Fill)
            .center_y(Length::Fixed(band))
            .clip(true)
            .into(),
        );
    }
    layers.push(items);
    Stack::with_children(layers)
        .width(Length::Fill)
        .height(Length::Fixed(height))
        .into()
}

/// 两边的按钮和标题所在的那一条：标准顶栏高；顶栏比它矮（悬浮侧栏的顶栏）时就是整条。
fn top_band<'a, Message: 'a>(item: Element<'a, Message>, band: f32) -> Element<'a, Message> {
    container(item).center_y(Length::Fixed(band)).into()
}

fn visible_control_count<Message>(
    shell: &WindowShell<'_, Message>,
    side: WindowControlSide,
) -> usize {
    shell
        .config
        .placements_on(side)
        .filter(|placement| placement.visibility().is_visible())
        .count()
}

fn controls<'a, Message>(
    shell: &WindowShell<'a, Message>,
    side: WindowControlSide,
) -> Element<'a, Message>
where
    Message: Clone + 'static,
{
    window_control_group(
        shell.config,
        side,
        shell.window,
        shell.frame_state,
        shell.on_control.as_ref(),
    )
}

fn content_surface_style(theme: &Theme) -> container::Style {
    let colors = ui_colors(theme);
    container::Style {
        background: Some(Background::Color(colors.background)),
        text_color: Some(colors.on_background),
        ..container::Style::default()
    }
}

/// 侧栏面：贴边平铺、没有圆角和描边，只靠底色和内容区区分（层次由配色决定）。
/// 侧栏比内容区“陷下去”一层（深色下更暗），导航和内容一眼分得开。
fn sidebar_surface_style(theme: &Theme) -> container::Style {
    let colors = ui_colors(theme);
    container::Style {
        background: Some(Background::Color(colors.surface_dim)),
        text_color: Some(colors.on_surface),
        ..container::Style::default()
    }
}

/// 悬浮侧栏：同一层底色，圆角卡片，没有描边。
fn floating_sidebar_style(theme: &Theme) -> container::Style {
    container::Style {
        border: Border {
            radius: CARD_RADIUS.into(),
            ..Border::default()
        },
        ..sidebar_surface_style(theme)
    }
}

fn divider_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(ui_colors(theme).outline_variant)),
        ..container::Style::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone)]
    enum ShellMessage {
        Control,
        Drag,
        ToggleMaximize,
        Resize,
    }

    fn shell(
        config: &WindowControlsConfig,
        frame_state: WindowFrameState,
        sidebar_style: SidebarStyle,
    ) -> WindowShell<'_, ShellMessage> {
        WindowShell {
            config,
            sidebar_style,
            window: window::Id::unique(),
            frame_state,
            on_control: Box::new(|_, _| ShellMessage::Control),
            title_drag: ShellMessage::Drag,
            title_double_click: ShellMessage::ToggleMaximize,
            on_resize: Box::new(|_| ShellMessage::Resize),
        }
    }

    /// 最外层是“内容 + 八个缩放命中区”的叠层时，子节点正好 9 个。
    fn has_resize_zones(element: &Element<'_, ShellMessage>) -> bool {
        element.as_widget().children().len() == 1 + RESIZE_DIRECTIONS.len()
    }

    fn body() -> Element<'static, ShellMessage> {
        Space::new().into()
    }

    #[test]
    fn both_skeletons_are_resizable_from_the_edges_only_while_restored() {
        let config = WindowControlsConfig::default();
        let pane = || SplitPane {
            header: HeaderBar::new(),
            body: body(),
        };
        for (frame_state, sidebar_style) in [
            (WindowFrameState::Restored, SidebarStyle::Embedded),
            (WindowFrameState::Restored, SidebarStyle::Floating),
            (WindowFrameState::Maximized, SidebarStyle::Embedded),
            (WindowFrameState::Maximized, SidebarStyle::Floating),
        ] {
            let shell = shell(&config, frame_state, sidebar_style);
            let expected = frame_state == WindowFrameState::Restored;
            assert_eq!(
                has_resize_zones(&single_window(HeaderBar::new(), body(), &shell)),
                expected,
                "single_window {frame_state:?}"
            );
            assert_eq!(
                has_resize_zones(&split_window(pane(), pane(), 200.0, &shell)),
                expected,
                "split_window {frame_state:?} {sidebar_style:?}"
            );
        }
    }

    #[test]
    fn a_second_header_row_sits_under_the_window_controls_not_beside_them() {
        let config = WindowControlsConfig::default();
        let shell = shell(&config, WindowFrameState::Restored, SidebarStyle::Embedded);
        let header = HeaderBar::new()
            .center(body())
            .below(row![body(), body()])
            .height(HEADER_BAR_HEIGHT * 2.0);

        let header = header_bar(header, HeaderEdges::Trailing, &shell);
        // 叠层：拖动面在下，按钮在上。
        let layers = header.as_widget().children();
        let rows = &layers.last().unwrap().children;
        // 第一行是工具栏第一行 + 右边的窗口按钮；第二行只有它自己的两项，旁边没有按钮。
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].children.len(), 2);
        assert_eq!(rows[1].children.len(), 2);
    }

    #[test]
    fn window_controls_go_to_the_header_touching_their_window_edge() {
        assert_eq!(HeaderEdges::Both.controls_sides(), (true, true));
        assert_eq!(HeaderEdges::Leading.controls_sides(), (true, false));
        assert_eq!(HeaderEdges::Trailing.controls_sides(), (false, true));
    }
}
