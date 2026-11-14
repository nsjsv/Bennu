//! 预览窗口宿主适配层：面板本体已下沉 `bennu_preview::preview_panel`，
//! 这里只负责把宿主专属词汇与滚动管线接线拼装后注入——查看器实例化
//! （渲染器敏感部件）、SQLite 标签行（segmented_choice_row 是设置窗口
//! 共用词汇，5a 结论留 app-ui）、SQLite 拖选结束事件、smooth-scroll 与
//! 视口回传闭包（scrollbar-guidelines 的宿主分工）。

use iced::Element;

use crate::app::smooth_scroll::{smooth_scroll_content, smooth_scroll_id};
use crate::model::{
    AudioPreviewPlayback, ImagePreviewViewport, Message, PreviewContent, PreviewSize, PreviewState,
    ScrollbarRegion, SqlitePreviewMessage, SqlitePreviewTab, TextPreviewDocument,
    VideoPreviewPlayback,
};
use bennu_preview::document_preview::{
    DocumentPreviewMessage, DocumentPreviewRequestKey, DocumentRenderKey, DocumentViewportKey,
};
use bennu_preview::engine::SqlitePreviewState;
use bennu_theme::controls::{segmented_choice_row, SegmentedChoice};

use bennu_preview::preview_message::PreviewMessage;
use bennu_preview::scroll_wiring::ScrollRegionWiring;
use bennu_preview::text_preview_viewer::text_preview_viewer;

pub(crate) fn view_preview_window<'a>(
    preview: Option<&'a PreviewState>,
    text_preview_document: Option<&'a TextPreviewDocument>,
    sqlite_preview_state: Option<&'a SqlitePreviewState>,
    size: PreviewSize,
    image_preview_viewport: &ImagePreviewViewport,
    audio_preview: Option<&'a AudioPreviewPlayback>,
    video_preview: Option<&'a VideoPreviewPlayback>,
    preview_bottom_controls_opacity: f32,
    operation_progress_animation_frame: u8,
) -> Element<'a, Message> {
    bennu_preview::preview_panel::view_preview_window(
        preview,
        text_preview_document,
        sqlite_preview_state,
        size,
        image_preview_viewport,
        audio_preview,
        video_preview,
        preview_bottom_controls_opacity,
        operation_progress_animation_frame,
        directory_scroll_wiring(),
        archive_scroll_wiring(),
        // 文档视口 key 依赖当前文档，接线在此（而不是 wiring 辅助内）构造。
        document_scroll_wiring(preview),
        markdown_scroll_wiring(),
        sqlite_tables_scroll_wiring(),
        sqlite_data_scroll_wiring(),
        // 查看器是渲染器敏感部件（Paragraph 泛型），宿主实例化注入；它自己就是滚动区，
        // 部件 id 用平滑滚动管线的区域 id，滚轮的 scroll_by 直接作用到它。
        |document: &TextPreviewDocument| {
            text_preview_viewer(
                document,
                smooth_scroll_id(&ScrollbarRegion::TextPreview),
                |delta| Message::SmoothScrollWheel(ScrollbarRegion::TextPreview, delta),
                |event| Message::from(PreviewMessage::TextViewer(event)),
            )
        },
        sqlite_tabs_row(sqlite_preview_state),
        || Message::DragSelectionFinished,
    )
}

// SQLite 标签行：segmented_choice_row 是设置窗口共用词汇（5a 结论：
// Message 泛化前留 app-ui 单源），宿主按当前激活标签构造后注入。
fn sqlite_tabs_row(state: Option<&SqlitePreviewState>) -> Element<'static, Message> {
    let active_tab = state
        .map(|state| state.active_tab)
        .unwrap_or(SqlitePreviewTab::Tables);
    segmented_choice_row(vec![
        SegmentedChoice {
            label: tr!("Table Data"),
            selected: active_tab == SqlitePreviewTab::Tables,
            message: Some(Message::Preview(PreviewMessage::SqlitePreview(
                SqlitePreviewMessage::TabSelected(SqlitePreviewTab::Tables),
            ))),
            tooltip: None,
        },
        SegmentedChoice {
            label: tr!("SQL Query"),
            selected: active_tab == SqlitePreviewTab::Sql,
            message: Some(Message::Preview(PreviewMessage::SqlitePreview(
                SqlitePreviewMessage::TabSelected(SqlitePreviewTab::Sql),
            ))),
            tooltip: None,
        },
    ])
}

fn directory_scroll_wiring() -> ScrollRegionWiring<'static, Message> {
    ScrollRegionWiring {
        smooth_scroll_wrap: Box::new(|content| {
            smooth_scroll_content(content, ScrollbarRegion::PreviewDirectory)
        }),
        scrollable_id: smooth_scroll_id(&ScrollbarRegion::PreviewDirectory),
        on_scroll: None,
    }
}

fn archive_scroll_wiring() -> ScrollRegionWiring<'static, Message> {
    ScrollRegionWiring {
        smooth_scroll_wrap: Box::new(|content| {
            smooth_scroll_content(content, ScrollbarRegion::PreviewArchive)
        }),
        scrollable_id: smooth_scroll_id(&ScrollbarRegion::PreviewArchive),
        on_scroll: None,
    }
}

// 文档分页滚动接线：视口事件合成闭包留在宿主，key 是文档会话的视口身份，滚轮/拖动偏移
// 都按它归档到对应渲染代。非文档会话的接线不会被面板消费，占位 key
// 用空路径 + 0 代（真实会话 generation 在开档时先自增、至少为 1，且
// source_path 非空），不可能与任何真实会话匹配。
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
        smooth_scroll_wrap: Box::new(|content| {
            smooth_scroll_content(content, ScrollbarRegion::PreviewDocument)
        }),
        scrollable_id: smooth_scroll_id(&ScrollbarRegion::PreviewDocument),
        on_scroll: Some(Box::new(move |viewport| {
            Message::Preview(PreviewMessage::DocumentPreview(
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
fn markdown_scroll_wiring<'a>() -> ScrollRegionWiring<'a, Message> {
    ScrollRegionWiring {
        smooth_scroll_wrap: Box::new(|content| {
            smooth_scroll_content(content, ScrollbarRegion::MarkdownPreview)
        }),
        scrollable_id: smooth_scroll_id(&ScrollbarRegion::MarkdownPreview),
        on_scroll: None,
    }
}

// 表列表/数据网格各自的滚动接线：不关心滚动位置，没有视口回传。
fn sqlite_tables_scroll_wiring<'a>() -> ScrollRegionWiring<'a, Message> {
    ScrollRegionWiring {
        smooth_scroll_wrap: Box::new(|content| {
            smooth_scroll_content(content, ScrollbarRegion::PreviewSqliteTables)
        }),
        scrollable_id: smooth_scroll_id(&ScrollbarRegion::PreviewSqliteTables),
        on_scroll: None,
    }
}

fn sqlite_data_scroll_wiring<'a>() -> ScrollRegionWiring<'a, Message> {
    ScrollRegionWiring {
        smooth_scroll_wrap: Box::new(|content| {
            smooth_scroll_content(content, ScrollbarRegion::PreviewSqliteData)
        }),
        scrollable_id: smooth_scroll_id(&ScrollbarRegion::PreviewSqliteData),
        on_scroll: None,
    }
}
