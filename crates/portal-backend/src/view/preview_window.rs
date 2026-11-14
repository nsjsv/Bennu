//! 预览窗视图适配层（app-ui view/preview_panel.rs + 窗口 chrome 包装的
//! portal 对应物）：面板本体在 `bennu_preview::preview_panel`，这里只做
//! 宿主注入——查看器实例化（渲染器敏感部件）、Markdown 模式切换行与
//! SQLite 标签行、
//! SQLite 拖选结束事件、七个滚动区域接线（smooth-scroll 包装 + 预览窗
//! 命名空间 id + 视口回传）。窗口外壳是共享的 `window_shell::single_window`
//! （常驻顶栏：文件名 + 固定按钮 + 窗口按钮）。

use iced::{window, Element, Theme};

use bennu_localization::tr;
use bennu_preview::document_preview::{
    DocumentPreviewMessage, DocumentPreviewRequestKey, DocumentRenderKey, DocumentViewportKey,
};
use bennu_preview::engine::SqlitePreviewState;
use bennu_preview::preview::{PreviewContent, PreviewState};
use bennu_preview::preview_message::PreviewMessage;
use bennu_preview::preview_panel;
use bennu_preview::preview_window_chrome::preview_pin_button;
use bennu_preview::scroll_wiring::ScrollRegionWiring;
use bennu_preview::sqlite_preview::{SqlitePreviewMessage, SqlitePreviewTab};
use bennu_preview::text_preview::TextPreviewDocument;
use bennu_preview::text_preview_viewer::text_preview_viewer;
use bennu_preview::window_shell::{single_window, HeaderBar};
use bennu_theme::controls::{segmented_choice_row, SegmentedChoice};
use bennu_theme::smooth_scroll::{SmoothScrollArea, SmoothScrollAxis};

use super::window_frame::window_shell;
use crate::preview_scroll::{preview_scroll_id, PreviewScrollRegion};
use crate::{Message, PickerDaemon};

/// 预览窗视图入口：面板放进共享外壳。
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
        directory_scroll_wiring(),
        archive_scroll_wiring(),
        document_scroll_wiring(engine.preview.as_ref()),
        markdown_scroll_wiring(),
        sqlite_tables_scroll_wiring(),
        sqlite_data_scroll_wiring(),
        // 查看器是渲染器敏感部件，宿主实例化注入；它自己就是滚动区，部件 id 用平滑
        // 滚动管线的区域 id（app-ui 适配层同构）。
        |document: &TextPreviewDocument| {
            text_preview_viewer(
                document,
                preview_scroll_id(PreviewScrollRegion::Text),
                |delta| Message::PreviewWheelScrolled {
                    region: PreviewScrollRegion::Text,
                    delta,
                },
                |event| Message::from(PreviewMessage::TextViewer(event)),
            )
        },
        sqlite_tabs_row(engine.sqlite_preview.as_ref()),
        || Message::PreviewSqliteDragFinished,
    );

    let title = engine
        .preview_shown_path
        .as_deref()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "预览".to_owned());
    single_window(
        HeaderBar::new()
            .title(title)
            .start(preview_pin_button(engine.preview_window_pinned)),
        panel,
        &window_shell(daemon, window),
    )
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
        on_scroll: None,
    }
}

fn archive_scroll_wiring() -> ScrollRegionWiring<'static, Message> {
    ScrollRegionWiring {
        smooth_scroll_wrap: smooth_scroll_wrap(PreviewScrollRegion::Archive),
        scrollable_id: preview_scroll_id(PreviewScrollRegion::Archive),
        on_scroll: None,
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
        on_scroll: Some(Box::new(move |viewport| {
            Message::from(PreviewMessage::DocumentPreview(
                DocumentPreviewMessage::Scrolled {
                    key: key.clone(),
                    offset_y: viewport.absolute_offset().y,
                    viewport_height: viewport.bounds().height,
                    content_height: viewport.content_bounds().height,
                },
            ))
        })),
    }
}

// Markdown 渲染体接线：整篇已经在内存里，不需要视口回传。
fn markdown_scroll_wiring() -> ScrollRegionWiring<'static, Message> {
    ScrollRegionWiring {
        smooth_scroll_wrap: smooth_scroll_wrap(PreviewScrollRegion::Markdown),
        scrollable_id: preview_scroll_id(PreviewScrollRegion::Markdown),
        on_scroll: None,
    }
}

fn sqlite_tables_scroll_wiring<'a>() -> ScrollRegionWiring<'a, Message> {
    ScrollRegionWiring {
        smooth_scroll_wrap: smooth_scroll_wrap(PreviewScrollRegion::SqliteTables),
        scrollable_id: preview_scroll_id(PreviewScrollRegion::SqliteTables),
        on_scroll: None,
    }
}

fn sqlite_data_scroll_wiring<'a>() -> ScrollRegionWiring<'a, Message> {
    ScrollRegionWiring {
        smooth_scroll_wrap: smooth_scroll_wrap(PreviewScrollRegion::SqliteData),
        scrollable_id: preview_scroll_id(PreviewScrollRegion::SqliteData),
        on_scroll: None,
    }
}

/// SQLite 标签行（Table Data / SQL Query，app-ui 适配层同款）。
fn sqlite_tabs_row(state: Option<&SqlitePreviewState>) -> Element<'static, Message> {
    let active_tab = state
        .map(|state| state.active_tab)
        .unwrap_or(SqlitePreviewTab::Tables);
    segmented_choice_row(vec![
        SegmentedChoice {
            label: tr!("Table Data"),
            selected: active_tab == SqlitePreviewTab::Tables,
            message: Some(Message::from(PreviewMessage::SqlitePreview(
                SqlitePreviewMessage::TabSelected(SqlitePreviewTab::Tables),
            ))),
            tooltip: None,
        },
        SegmentedChoice {
            label: tr!("SQL Query"),
            selected: active_tab == SqlitePreviewTab::Sql,
            message: Some(Message::from(PreviewMessage::SqlitePreview(
                SqlitePreviewMessage::TabSelected(SqlitePreviewTab::Sql),
            ))),
            tooltip: None,
        },
    ])
}
