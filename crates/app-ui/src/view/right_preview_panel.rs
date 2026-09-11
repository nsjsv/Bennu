use file_core::FileKind;
use iced::mouse::Interaction;
use iced::widget::{column, container, mouse_area, row, scrollable, space::Space};
use iced::{Alignment, Element, Length};

use crate::app::right_preview_panel::{
    PANEL_CONTENT_PADDING, PANEL_RATIO_DIVIDER_HEIGHT, PANEL_RESIZE_HANDLE_WIDTH,
};
use crate::app::FileBrowser;
use crate::appearance::{app_content_style, column_resize_divider_style, muted_text_color};
use crate::formatting::{format_middle_ellipsized_text, format_system_time};
use crate::model::{Message, RightPreviewPanelInfoSnapshot, SPLIT_PORTION_TOTAL};

/// 右侧停靠预览面板:左缘拖拽热区 + 顶条 + 预览区 + 分隔条 + 文件信息区。
/// 只有开启时才会被挂进 pane_layer 的 row;关闭时整个元素不存在,
/// 布局与没有这个特性时逐位一致。宽度用生效值:窗口过窄时面板
/// 先让路,不允许把浏览器区挤没。
pub(crate) fn right_preview_panel(browser: &FileBrowser) -> Element<'_, Message> {
    let handle: Element<'_, Message> = mouse_area(
        row![
            container(Space::new())
                .width(Length::Fixed(1.0))
                .height(Length::Fill)
                .style(column_resize_divider_style),
            Space::new().width(Length::Fixed(PANEL_RESIZE_HANDLE_WIDTH - 1.0)),
        ]
        .height(Length::Fill),
    )
    .on_press(Message::RightPreviewPanelResizeStarted)
    .interaction(Interaction::ResizingHorizontally)
    .into();
    // 预览/信息区按比例 portion 分割,分隔线位置由布局决定,不依赖
    // 高度像素估算;信息区保底由读取侧生效比例夹取保证。
    // 手柄必须留在外层 row:Fill 高度与 FillPortion 同列会按 1 份
    // 参与分配,整条左缘拖拽带会被压到 ~1px(截图回归)。
    let preview_portion = browser.right_preview_panel_preview_portion();
    let preview_area: Element<'_, Message> = container(browser.right_preview_panel_content())
        .width(Length::Fill)
        .height(Length::FillPortion(preview_portion))
        .clip(true)
        .style(app_content_style)
        .into();
    let info_area: Element<'_, Message> = container(right_preview_info_content(browser))
        .width(Length::Fill)
        .height(Length::FillPortion(SPLIT_PORTION_TOTAL - preview_portion))
        .clip(true)
        .style(app_content_style)
        .into();
    let content: Element<'_, Message> = column![]
        .push(preview_area)
        .push(ratio_resize_divider())
        .push(info_area)
        .width(Length::Fill)
        .height(Length::Fill)
        .into();
    row![handle, content]
        .width(Length::Fixed(browser.right_preview_panel_effective_width()))
        .height(Length::Fill)
        .into()
}

/// 预览/信息区分隔条:整条都是拖拽热区,中缝画 1px 分隔线。
fn ratio_resize_divider() -> Element<'static, Message> {
    let line: Element<'static, Message> = container(Space::new().height(Length::Fixed(1.0)))
        .width(Length::Fill)
        .style(column_resize_divider_style)
        .into();
    let top_padding = (PANEL_RATIO_DIVIDER_HEIGHT - 1.0) / 2.0;
    mouse_area(
        column![
            Space::new()
                .width(Length::Fill)
                .height(Length::Fixed(top_padding)),
            line,
            Space::new().width(Length::Fill).height(Length::Fixed(
                PANEL_RATIO_DIVIDER_HEIGHT - 1.0 - top_padding
            )),
        ]
        .width(Length::Fill),
    )
    .on_press(Message::RightPreviewPanelRatioResizeStarted)
    .interaction(Interaction::ResizingVertically)
    .into()
}

/// 文件信息区:参照 Windows 资源管理器/macOS 访达详情栏的只读元数据。
/// 数据按预览目标路径异步读自文件系统,与目录列表快照解耦——列视图
/// 深层选中、搜索结果同样有完整字段。快照未到位(加载中/读取失败/
/// 无选中)时字段降级为占位,名称尽力从路径推导。
fn right_preview_info_content(browser: &FileBrowser) -> Element<'_, Message> {
    let snapshot = browser
        .right_preview_panel_info
        .as_ref()
        .filter(|snapshot| Some(snapshot.path.as_path()) == browser.selected.as_deref());
    let name = match snapshot {
        Some(snapshot) => format_middle_ellipsized_text(&snapshot.name.to_string_lossy(), 48),
        None => browser
            .selected
            .as_ref()
            .map(|path| format_middle_ellipsized_text(&fallback_name(path), 48))
            .unwrap_or_else(|| "—".to_owned()),
    };
    let type_label = snapshot
        .map(|snapshot| crate::localization::translate_current(&snapshot.type_label))
        .unwrap_or_else(|| "—".to_owned());
    let size_label = match snapshot {
        Some(snapshot) if snapshot.kind != FileKind::Directory => display_size(snapshot.size_bytes),
        // 目录体积需聚合统计,与访达一致留空。
        _ => "—".to_owned(),
    };
    let info_rows = column![
        info_row("Name", name),
        info_row("Type", type_label),
        info_row("Size", size_label),
        info_row("Location", location_label(snapshot)),
        info_row("Modified", optional_time(snapshot.map(|s| s.modified))),
        info_row("Created", optional_time(snapshot.map(|s| s.created))),
        info_row("Accessed", optional_time(snapshot.map(|s| s.accessed))),
    ]
    .spacing(6)
    .width(Length::Fill);
    container(scrollable(info_rows))
        .padding(iced::Padding {
            top: 8.0,
            bottom: 8.0,
            left: PANEL_CONTENT_PADDING,
            right: PANEL_CONTENT_PADDING,
        })
        .width(Length::Fill)
        .align_y(Alignment::Start)
        .height(Length::Fill)
        .into()
}

fn fallback_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn location_label(snapshot: Option<&RightPreviewPanelInfoSnapshot>) -> String {
    snapshot
        .map(|snapshot| format_middle_ellipsized_text(&snapshot.location.to_string_lossy(), 52))
        .unwrap_or_else(|| "—".to_owned())
}

fn optional_time(time: Option<Option<std::time::SystemTime>>) -> String {
    time.flatten()
        .map(format_system_time)
        .unwrap_or_else(|| "—".to_owned())
}

/// 与属性窗口的体积措辞一致:本地化单位 + 原始字节数。
fn display_size(bytes: u64) -> String {
    if crate::localization::current_language_is_chinese() {
        format!(
            "{}（{} 字节）",
            crate::formatting::format_file_size(bytes),
            bytes
        )
    } else {
        format!(
            "{} ({} bytes)",
            crate::formatting::format_file_size(bytes),
            bytes
        )
    }
}

fn info_row(label: &'static str, value: String) -> Element<'static, Message> {
    row![
        crate::typography::readable_text(label)
            .size(13)
            .style(|theme| iced::widget::text::Style {
                color: Some(muted_text_color(theme)),
            }),
        iced::widget::text(value).size(13),
    ]
    .spacing(10)
    .width(Length::Fill)
    .into()
}
