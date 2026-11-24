use file_core::FileKind;
use iced::widget::{column, container, row, scrollable};
use iced::{Alignment, Element, Length};

use crate::app::right_preview_panel::PANEL_CONTENT_PADDING;
use crate::app::FileBrowser;
use crate::appearance::muted_text_color;
use crate::formatting::{format_middle_ellipsized_text, format_system_time};
use crate::model::{Message, RightPreviewPanelInfoSnapshot};

/// 文件信息区:参照 Windows 资源管理器/macOS 访达详情栏的只读元数据。
/// 数据按预览目标路径异步读自文件系统,与目录列表快照解耦——列视图
/// 深层选中、搜索结果同样有完整字段。快照未到位(加载中/读取失败/
/// 无选中)时字段降级为占位,名称尽力从路径推导。
pub(super) fn info_panel_content(browser: &FileBrowser) -> Element<'_, Message> {
    let snapshot = browser
        .info_panel_snapshot
        .as_ref()
        .filter(|snapshot| Some(snapshot.path.as_path()) == browser.tab().selected.as_deref());
    let name = match snapshot {
        Some(snapshot) => format_middle_ellipsized_text(&snapshot.name.to_string_lossy(), 48),
        None => browser
            .tab()
            .selected
            .as_ref()
            .map(|path| format_middle_ellipsized_text(&fallback_name(path), 48))
            .unwrap_or_else(|| "—".to_owned()),
    };
    let type_label = snapshot
        .map(|snapshot| bennu_preview::formatting::file_kind_label(snapshot.kind))
        .unwrap_or_else(|| "—".to_owned());
    let size_label = match snapshot {
        Some(snapshot) if snapshot.kind != FileKind::Directory => display_size(snapshot.size_bytes),
        // 目录体积需聚合统计,与访达一致留空。
        _ => "—".to_owned(),
    };
    let info_rows = column![
        info_row(tr!("Name"), name),
        info_row(tr!("Type"), type_label),
        info_row(tr!("Size"), size_label),
        info_row(tr!("Location"), location_label(snapshot)),
        info_row(tr!("Modified"), optional_time(snapshot.map(|s| s.modified))),
        info_row(tr!("Created"), optional_time(snapshot.map(|s| s.created))),
        info_row(tr!("Accessed"), optional_time(snapshot.map(|s| s.accessed))),
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
    tr!(
        "%1 (%2 bytes)",
        crate::formatting::format_file_size(bytes),
        bytes
    )
}

fn info_row(label: String, value: String) -> Element<'static, Message> {
    row![
        bennu_preview::panel_text::readable_text(label).style(|theme| iced::widget::text::Style {
            color: Some(muted_text_color(theme)),
        }),
        iced::widget::text(value),
    ]
    .spacing(10)
    .width(Length::Fill)
    .into()
}
