//! 预览窗视图适配层（app-ui view/preview_panel.rs + 窗口 chrome 包装的
//! portal 对应物）：面板本体在 `bennu_preview::preview_panel`，这里只做
//! 宿主注入——查看器实例化（渲染器敏感部件）、Markdown 模式切换行与
//! SQLite 标签行（segmented 行是宿主词汇，portal 自备最小实现）、
//! SQLite 拖选结束事件、七个滚动区域接线（smooth-scroll 包装 + 预览窗
//! 命名空间 id + 视口回传）。窗口 chrome：媒体类走共享浮动 chrome，
//! 文本/SQLite 走标准顶栏（engine.preview_window_uses_window_chrome 的
//! 分派，对齐主软件）；两者都叠加标题拖动面与手动 resize 边缘
//! （预览窗 decorations:false，主软件同款自绘机制）。

use iced::widget::{button, container, mouse_area, row, Space, Stack};
use iced::{mouse, window, Element, Length, Theme};

use bennu_preview::document_preview::{
    DocumentPreviewMessage, DocumentPreviewRequestKey, DocumentRenderKey, DocumentViewportKey,
};
use bennu_preview::engine::SqlitePreviewState;
use bennu_preview::panel_text::localized_text;
use bennu_preview::preview::{PreviewContent, PreviewState};
use bennu_preview::preview_message::PreviewMessage;
use bennu_preview::preview_panel;
use bennu_preview::preview_window_chrome::{
    floating_preview_window_content, preview_pin_button, window_control_group,
};
use bennu_preview::scroll_wiring::{ScrollRegionState, ScrollRegionWiring};
use bennu_preview::sqlite_preview::{SqlitePreviewMessage, SqlitePreviewTab};
use bennu_preview::text_preview::{MarkdownPreviewMode, TextPreviewDocument};
use bennu_preview::text_preview_viewer::text_preview_viewer;
use bennu_theme::smooth_scroll::{SmoothScrollArea, SmoothScrollAxis};
use bennu_theme::styles::{base_text_color, subtle_border_color};
use bennu_theme::ui_colors;
use bennu_theme::window_controls::{
    WindowControlKind, WindowControlSide, WindowControlsConfig, WindowFrameState,
    WINDOW_TOP_BAR_HEIGHT,
};

use crate::preview_host::PreviewHost;
use crate::preview_scroll::{preview_scroll_id, scrollbar_viewport_from, PreviewScrollRegion};
use crate::view::window_drag_region::window_drag_region;
use crate::{Message, PickerDaemon};

/// 无装饰窗口手动 resize 的边缘/拐角命中宽度（主软件同值）。
const WINDOW_RESIZE_EDGE_WIDTH: f32 = 5.0;
const WINDOW_RESIZE_CORNER_WIDTH: f32 = 11.0;

/// 预览窗视图入口：面板 + chrome + 拖动面 + resize 边缘。
pub(crate) fn preview_window_view(
    daemon: &PickerDaemon,
    window: window::Id,
) -> Element<'_, Message> {
    let host = &daemon.preview;
    let engine = &host.engine;
    let panel = preview_panel::view_preview_window(
        engine.preview.as_ref(),
        engine.text_preview_document.as_ref(),
        engine.sqlite_preview.as_ref(),
        engine.preview_size,
        &engine.preview_image_viewport,
        engine.audio_preview.as_ref(),
        engine.video_preview.as_ref(),
        engine.preview_window_bottom_controls.opacity(),
        // 不定进度动画帧：portal 不移植远程下载管线，无生产者，恒 0。
        0,
        engine.text_preview_content_height,
        scroll_region_state(
            host,
            PreviewScrollRegion::Directory,
            directory_scroll_wiring(),
        ),
        scroll_region_state(host, PreviewScrollRegion::Archive, archive_scroll_wiring()),
        scroll_region_state(
            host,
            PreviewScrollRegion::Document,
            document_scroll_wiring(engine.preview.as_ref()),
        ),
        scroll_region_state(host, PreviewScrollRegion::Text, text_scroll_wiring()),
        scroll_region_state(
            host,
            PreviewScrollRegion::Markdown,
            markdown_scroll_wiring(),
        ),
        scroll_region_state(
            host,
            PreviewScrollRegion::SqliteTables,
            sqlite_tables_scroll_wiring(),
        ),
        scroll_region_state(
            host,
            PreviewScrollRegion::SqliteData,
            sqlite_data_scroll_wiring(),
        ),
        // 查看器是渲染器敏感部件，宿主实例化注入；内容事件走
        // PreviewMessage，滚轮走宿主滚动管线（app-ui 适配层同构）。
        |document: &TextPreviewDocument, scroll_height| {
            let wheel_region = PreviewScrollRegion::Text;
            text_preview_viewer(
                document,
                scroll_height,
                // 滚动几何宿主是输入源：只镜像文档副本预取分块，不回推宿主。
                |lines, _offset_y, viewport_height| {
                    Message::from(PreviewMessage::TextPreviewContentScrolled {
                        lines,
                        viewport_height,
                    })
                },
                move |delta| Message::PreviewWheelScrolled {
                    region: wheel_region,
                    delta,
                },
                |content_height| {
                    Message::from(PreviewMessage::TextPreviewContentHeightChanged(
                        content_height,
                    ))
                },
                // 查看器内部滚动（键盘/光标跟随）才回推宿主滚动位置。
                |lines, offset_y, viewport_height| {
                    Message::from(PreviewMessage::TextPreviewViewerScrolled {
                        lines,
                        offset_y,
                        viewport_height,
                    })
                },
            )
        },
        markdown_mode_switch_row,
        sqlite_tabs_row(engine.sqlite_preview.as_ref()),
        || Message::PreviewSqliteDragFinished,
    );

    let frame_state = if host.preview_window_maximized {
        WindowFrameState::Maximized
    } else {
        WindowFrameState::Restored
    };
    let controls = WindowControlsConfig::default();
    let content = if engine.preview_window_uses_window_chrome() {
        preview_window_top_bar(
            panel,
            &controls,
            window,
            frame_state,
            engine.preview_window_pinned,
        )
    } else {
        floating_preview_window_content(
            panel,
            &controls,
            window,
            frame_state,
            engine.preview_window_chrome.opacity(),
            engine.preview_window_pinned,
            &window_control_message,
            // 标题拖动面（拖动/双击最大化）是宿主窗口管理语义，闭包注入。
            move |top_bar| {
                window_drag_region(
                    top_bar,
                    window,
                    |dragged| Message::PreviewWindowTitlePressed {
                        window: dragged,
                        double_click: false,
                    },
                    |maximized| Message::PreviewWindowTitlePressed {
                        window: maximized,
                        double_click: true,
                    },
                )
            },
        )
    };
    window_resize_frame(content, window, frame_state)
}

/// chrome 控制按钮 → 宿主窗口动作（最小化/最大化切换/关闭预览窗）。
fn window_control_message(kind: WindowControlKind, window: window::Id) -> Message {
    Message::PreviewWindowControl { window, kind }
}

fn scroll_region_state<'a>(
    host: &PreviewHost,
    region: PreviewScrollRegion,
    wiring: ScrollRegionWiring<'a, Message>,
) -> ScrollRegionState<'a, Message> {
    ScrollRegionState {
        visibility: host.scroll.visibility_for(region),
        viewport: host.scroll.viewport_for(region),
        wiring,
    }
}

/// 预览七区全为竖向滚动；shift 换算与主软件 preview 区域同语义
/// （app-ui 的 smooth_scroll_content 传 false，轴向换算在处理端）。
type SmoothScrollWrapFn<'a> =
    Box<dyn Fn(Element<'a, Message, Theme>) -> Element<'a, Message, Theme> + 'a>;

fn smooth_scroll_wrap<'a>(region: PreviewScrollRegion) -> SmoothScrollWrapFn<'a> {
    Box::new(move |content| {
        Element::new(SmoothScrollArea::new(
            content,
            SmoothScrollAxis::Vertical,
            false,
            move |delta| Message::PreviewWheelScrolled { region, delta },
        ))
    })
}

fn directory_scroll_wiring() -> ScrollRegionWiring<'static, Message> {
    ScrollRegionWiring {
        smooth_scroll_wrap: smooth_scroll_wrap(PreviewScrollRegion::Directory),
        scrollable_id: preview_scroll_id(PreviewScrollRegion::Directory),
        on_scroll: Box::new(|viewport| Message::PreviewScrollbarViewportChanged {
            region: PreviewScrollRegion::Directory,
            viewport: scrollbar_viewport_from(&viewport),
            event: Box::new(PreviewMessage::PreviewDirectoryScrolled),
        }),
    }
}

fn archive_scroll_wiring() -> ScrollRegionWiring<'static, Message> {
    ScrollRegionWiring {
        smooth_scroll_wrap: smooth_scroll_wrap(PreviewScrollRegion::Archive),
        scrollable_id: preview_scroll_id(PreviewScrollRegion::Archive),
        on_scroll: Box::new(|viewport| Message::PreviewScrollbarViewportChanged {
            region: PreviewScrollRegion::Archive,
            viewport: scrollbar_viewport_from(&viewport),
            event: Box::new(PreviewMessage::PreviewArchiveScrolled),
        }),
    }
}

// 文档分页滚动接线：视口事件按当前文档会话的视口 key 归档到对应渲染
// 代。非文档会话的接线不会被面板消费，占位 key 不可能与真实会话匹配
// （真实 generation 开档时先自增、至少为 1，且 source_path 非空）。
fn document_scroll_wiring(preview: Option<&PreviewState>) -> ScrollRegionWiring<'static, Message> {
    let key = match preview {
        Some(PreviewState::Ready(PreviewContent::PagedDocument(document))) => {
            document.viewport_key()
        }
        _ => DocumentViewportKey {
            render: DocumentRenderKey {
                request: DocumentPreviewRequestKey {
                    source_path: std::path::PathBuf::new(),
                    document_generation: 0,
                },
                render_generation: 0,
                width_bucket: 0,
            },
            layout_generation: 0,
        },
    };
    ScrollRegionWiring {
        smooth_scroll_wrap: smooth_scroll_wrap(PreviewScrollRegion::Document),
        scrollable_id: preview_scroll_id(PreviewScrollRegion::Document),
        on_scroll: Box::new(move |viewport| Message::PreviewScrollbarViewportChanged {
            region: PreviewScrollRegion::Document,
            viewport: scrollbar_viewport_from(&viewport),
            event: Box::new(PreviewMessage::DocumentPreview(
                DocumentPreviewMessage::Scrolled {
                    key: key.clone(),
                    offset_y: viewport.absolute_offset().y,
                    viewport_height: viewport.bounds().height,
                    content_height: viewport.content_bounds().height,
                },
            )),
        }),
    }
}

// 纯文本几何宿主接线：视口回传把滚动位置同步给查看器（查看器从动不
// 回推）。'a 出现在 Fn 参数位（不变），须生命周期泛型实例化（教训⑥）。
fn text_scroll_wiring<'a>() -> ScrollRegionWiring<'a, Message> {
    ScrollRegionWiring {
        smooth_scroll_wrap: smooth_scroll_wrap(PreviewScrollRegion::Text),
        scrollable_id: preview_scroll_id(PreviewScrollRegion::Text),
        on_scroll: Box::new(|viewport| Message::PreviewScrollbarViewportChanged {
            region: PreviewScrollRegion::Text,
            viewport: scrollbar_viewport_from(&viewport),
            event: Box::new(PreviewMessage::TextPreviewViewportSynced {
                offset_y: viewport.absolute_offset().y,
                viewport_height: viewport.bounds().height,
            }),
        }),
    }
}

// Markdown 渲染体接线：滚动偏移随视口回传归档到文档滚动状态。
fn markdown_scroll_wiring() -> ScrollRegionWiring<'static, Message> {
    ScrollRegionWiring {
        smooth_scroll_wrap: smooth_scroll_wrap(PreviewScrollRegion::Markdown),
        scrollable_id: preview_scroll_id(PreviewScrollRegion::Markdown),
        on_scroll: Box::new(|viewport| Message::PreviewScrollbarViewportChanged {
            region: PreviewScrollRegion::Markdown,
            viewport: scrollbar_viewport_from(&viewport),
            event: Box::new(PreviewMessage::MarkdownPreviewScrolled {
                offset_y: viewport.absolute_offset().y,
                viewport_height: viewport.bounds().height,
                content_height: viewport.content_bounds().height,
            }),
        }),
    }
}

fn sqlite_tables_scroll_wiring<'a>() -> ScrollRegionWiring<'a, Message> {
    ScrollRegionWiring {
        smooth_scroll_wrap: smooth_scroll_wrap(PreviewScrollRegion::SqliteTables),
        scrollable_id: preview_scroll_id(PreviewScrollRegion::SqliteTables),
        on_scroll: Box::new(|viewport| Message::PreviewScrollbarViewportChanged {
            region: PreviewScrollRegion::SqliteTables,
            viewport: scrollbar_viewport_from(&viewport),
            event: Box::new(PreviewMessage::SqlitePreviewTablesScrolled),
        }),
    }
}

fn sqlite_data_scroll_wiring<'a>() -> ScrollRegionWiring<'a, Message> {
    ScrollRegionWiring {
        smooth_scroll_wrap: smooth_scroll_wrap(PreviewScrollRegion::SqliteData),
        scrollable_id: preview_scroll_id(PreviewScrollRegion::SqliteData),
        on_scroll: Box::new(|viewport| Message::PreviewScrollbarViewportChanged {
            region: PreviewScrollRegion::SqliteData,
            viewport: scrollbar_viewport_from(&viewport),
            event: Box::new(PreviewMessage::SqlitePreviewDataScrolled),
        }),
    }
}

// ---------------------------------------------------------------------------
// 宿主词汇：segmented 选择行（app-ui view/option_controls.rs 的最小移植，
// 无 tooltip/未激活变体——预览窗只有这两处消费）。
// ---------------------------------------------------------------------------

struct SegmentedChoice {
    label: &'static str,
    selected: bool,
    message: Message,
}

fn segmented_choice_row(choices: Vec<SegmentedChoice>) -> Element<'static, Message> {
    let mut options = row![].spacing(0).align_y(iced::Alignment::Center);
    for choice in choices {
        options = options.push(segmented_choice_button(choice));
    }
    container(options)
        .padding(2)
        .width(Length::Fill)
        .style(segmented_choice_group_style)
        .into()
}

const SEGMENTED_CHOICE_HEIGHT: f32 = 30.0;

fn segmented_choice_button(choice: SegmentedChoice) -> Element<'static, Message> {
    let label = container(localized_text(choice.label).size(12))
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill);
    button(label)
        .on_press(choice.message)
        .width(Length::FillPortion(1))
        .height(Length::Fixed(SEGMENTED_CHOICE_HEIGHT))
        .padding([4, 8])
        .style(segmented_choice_button_style(choice.selected))
        .into()
}

fn segmented_choice_group_style(theme: &Theme) -> container::Style {
    let colors = ui_colors(theme);
    container::Style {
        background: Some(colors.surface_container_low.into()),
        text_color: Some(colors.on_surface),
        border: subtle_border(theme),
        ..container::Style::default()
    }
}

fn segmented_choice_button_style(
    selected: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style + Clone {
    move |theme, status| {
        let colors = ui_colors(theme);
        let background = match (selected, status) {
            (true, button::Status::Hovered) => colors.primary_container.scale_alpha(0.9),
            (true, button::Status::Pressed) => colors.primary_container.scale_alpha(0.8),
            (true, _) => colors.primary_container,
            (false, button::Status::Hovered | button::Status::Pressed) => {
                colors.surface_container_high
            }
            (false, _) => colors.surface_container,
        };
        button::Style {
            background: Some(background.into()),
            text_color: if selected {
                colors.on_primary_container
            } else {
                base_text_color(theme)
            },
            border: iced::Border {
                color: if selected {
                    colors.primary
                } else {
                    subtle_border_color(theme)
                },
                width: 1.0,
                radius: 6.0.into(),
            },
            ..button::Style::default()
        }
    }
}

fn subtle_border(theme: &Theme) -> iced::Border {
    iced::Border {
        color: subtle_border_color(theme),
        width: 1.0,
        radius: 8.0.into(),
    }
}

/// Markdown 渲染/原文切换行（app-ui 适配层同款两段）。
fn markdown_mode_switch_row(mode: MarkdownPreviewMode) -> Element<'static, Message> {
    segmented_choice_row(vec![
        SegmentedChoice {
            label: "Rendered",
            selected: mode == MarkdownPreviewMode::Rendered,
            message: Message::from(PreviewMessage::MarkdownPreviewModeSelected(
                MarkdownPreviewMode::Rendered,
            )),
        },
        SegmentedChoice {
            label: "Raw",
            selected: mode == MarkdownPreviewMode::Raw,
            message: Message::from(PreviewMessage::MarkdownPreviewModeSelected(
                MarkdownPreviewMode::Raw,
            )),
        },
    ])
}

/// SQLite 标签行（Table Data / SQL Query，app-ui 适配层同款）。
fn sqlite_tabs_row(state: Option<&SqlitePreviewState>) -> Element<'static, Message> {
    let active_tab = state
        .map(|state| state.active_tab)
        .unwrap_or(SqlitePreviewTab::Tables);
    segmented_choice_row(vec![
        SegmentedChoice {
            label: "Table Data",
            selected: active_tab == SqlitePreviewTab::Tables,
            message: Message::from(PreviewMessage::SqlitePreview(
                SqlitePreviewMessage::TabSelected(SqlitePreviewTab::Tables),
            )),
        },
        SegmentedChoice {
            label: "SQL Query",
            selected: active_tab == SqlitePreviewTab::Sql,
            message: Message::from(PreviewMessage::SqlitePreview(
                SqlitePreviewMessage::TabSelected(SqlitePreviewTab::Sql),
            )),
        },
    ])
}

// ---------------------------------------------------------------------------
// 窗口 chrome：标准顶栏（文本/SQLite）与手动 resize 边缘。
// ---------------------------------------------------------------------------

/// 标准顶栏（app-ui window_top_bar 的移植）：拖动面 + 左控制组 + 固定
/// 按钮 + 标题 + 右控制组，横贯 48px；内容列在顶栏下方。
fn preview_window_top_bar<'a>(
    content: Element<'a, Message>,
    config: &WindowControlsConfig,
    window: window::Id,
    frame_state: WindowFrameState,
    pinned: bool,
) -> Element<'a, Message> {
    let drag_surface = container(Space::new().width(Length::Fill).height(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fixed(WINDOW_TOP_BAR_HEIGHT))
        .style(window_top_bar_style);
    let drag_surface = window_drag_region(
        drag_surface.into(),
        window,
        |dragged| Message::PreviewWindowTitlePressed {
            window: dragged,
            double_click: false,
        },
        |maximized| Message::PreviewWindowTitlePressed {
            window: maximized,
            double_click: true,
        },
    );
    let mut controls = row![]
        .spacing(10)
        .padding([8, 12])
        .align_y(iced::Alignment::Center)
        .push(window_control_group(
            config,
            WindowControlSide::Left,
            window,
            frame_state,
            &window_control_message,
        ))
        .push(preview_pin_button(pinned))
        .push(localized_text("预览").size(16))
        .push(Space::new().width(Length::Fill))
        .push(window_control_group(
            config,
            WindowControlSide::Right,
            window,
            frame_state,
            &window_control_message,
        ));
    controls = controls
        .width(Length::Fill)
        .height(Length::Fixed(WINDOW_TOP_BAR_HEIGHT));

    let top_bar = Stack::with_children([drag_surface, controls.into()])
        .width(Length::Fill)
        .height(Length::Fixed(WINDOW_TOP_BAR_HEIGHT));

    iced::widget::Column::new()
        .spacing(0)
        .push(top_bar)
        .push(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn window_top_bar_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(ui_colors(theme).background.into()),
        text_color: Some(base_text_color(theme)),
        ..container::Style::default()
    }
}

/// 手动 resize 边缘（主软件 window_resize_frame 的移植）：最大化时无
/// 边缘，8 个方向的命中区叠在内容之上。
fn window_resize_frame<'a>(
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
    use iced::alignment::{Horizontal, Vertical};
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
        .on_press(Message::PreviewWindowResizeEdgePressed { window, direction })
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

    /// chrome 按钮消息映射：三类控制动作必须落到统一的
    /// PreviewWindowControl 变体（update 侧按 kind 分派窗口动作）。
    #[test]
    fn chrome_controls_map_to_preview_window_control() {
        let window = window::Id::unique();
        for kind in [
            WindowControlKind::Minimize,
            WindowControlKind::MaximizeRestore,
            WindowControlKind::Close,
        ] {
            assert!(matches!(
                window_control_message(kind, window),
                Message::PreviewWindowControl {
                    window: target,
                    kind: sent,
                } if target == window && sent == kind
            ));
        }
    }
}
