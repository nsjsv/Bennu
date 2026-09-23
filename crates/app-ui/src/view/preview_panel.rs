//! 预览窗口宿主适配层：面板本体已下沉 `bennu_preview::preview_panel`，
//! 这里保持 `view_preview_window` 原签名（25 参数）不变，只负责把宿主
//! 专属词汇与滚动管线接线拼装后注入——查看器实例化（渲染器敏感部件）、
//! Markdown 模式切换行与 SQLite 标签行（segmented_choice_row 是设置窗口
//! 共用词汇，5a 结论留 app-ui）、SQLite 拖选结束事件、smooth-scroll 与
//! 视口回传闭包（scrollbar-guidelines 的宿主分工）。

use iced::Element;

use crate::app::scrollbar::scrollbar_on_scroll;
use crate::app::smooth_scroll::{smooth_scroll_content, smooth_scroll_id};
use crate::document_preview::DocumentPreviewMessage;
use crate::model::{
    AudioPreviewPlayback, ImagePreviewViewport, MarkdownPreviewMode, Message, PreviewContent,
    PreviewSize, PreviewState, ScrollbarRegion, ScrollbarViewport, ScrollbarVisibility,
    SqlitePreviewMessage, SqlitePreviewTab, TextPreviewDocument, VideoPreviewPlayback,
};
use crate::view::option_controls::{segmented_choice_row, SegmentedChoice};
use bennu_preview::document_preview::{
    DocumentPreviewRequestKey, DocumentRenderKey, DocumentViewportKey,
};
use bennu_preview::engine::SqlitePreviewState;

use bennu_preview::preview_message::PreviewMessage;
use bennu_preview::scroll_wiring::{ScrollRegionState, ScrollRegionWiring};
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
    directory_scrollbar_visibility: ScrollbarVisibility,
    directory_scrollbar_viewport: Option<ScrollbarViewport>,
    archive_scrollbar_visibility: ScrollbarVisibility,
    archive_scrollbar_viewport: Option<ScrollbarViewport>,
    document_scrollbar_visibility: ScrollbarVisibility,
    document_scrollbar_viewport: Option<ScrollbarViewport>,
    text_scrollbar_visibility: ScrollbarVisibility,
    text_scrollbar_viewport: Option<ScrollbarViewport>,
    text_preview_content_height: f32,
    markdown_scrollbar_visibility: ScrollbarVisibility,
    markdown_scrollbar_viewport: Option<ScrollbarViewport>,
    sqlite_tables_visibility: ScrollbarVisibility,
    sqlite_tables_viewport: Option<ScrollbarViewport>,
    sqlite_data_visibility: ScrollbarVisibility,
    sqlite_data_viewport: Option<ScrollbarViewport>,
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
        text_preview_content_height,
        ScrollRegionState {
            visibility: directory_scrollbar_visibility,
            viewport: directory_scrollbar_viewport,
            wiring: directory_scroll_wiring(),
        },
        ScrollRegionState {
            visibility: archive_scrollbar_visibility,
            viewport: archive_scrollbar_viewport,
            wiring: archive_scroll_wiring(),
        },
        ScrollRegionState {
            visibility: document_scrollbar_visibility,
            viewport: document_scrollbar_viewport,
            // 文档视口 key 依赖当前文档，接线在此（而不是 wiring 辅助内）构造。
            wiring: document_scroll_wiring(preview),
        },
        ScrollRegionState {
            visibility: text_scrollbar_visibility,
            viewport: text_scrollbar_viewport,
            wiring: text_scroll_wiring(),
        },
        ScrollRegionState {
            visibility: markdown_scrollbar_visibility,
            viewport: markdown_scrollbar_viewport,
            wiring: markdown_scroll_wiring(),
        },
        ScrollRegionState {
            visibility: sqlite_tables_visibility,
            viewport: sqlite_tables_viewport,
            wiring: sqlite_tables_scroll_wiring(),
        },
        ScrollRegionState {
            visibility: sqlite_data_visibility,
            viewport: sqlite_data_viewport,
            wiring: sqlite_data_scroll_wiring(),
        },
        // 查看器是渲染器敏感部件（Paragraph 泛型），宿主实例化注入；
        // 内容事件走 PreviewMessage，滚轮走宿主滚动管线。
        |document: &TextPreviewDocument, scroll_height| {
            let wheel_region = ScrollbarRegion::TextPreview;
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
                move |delta| Message::SmoothScrollWheel(wheel_region.clone(), delta),
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
        |mode| {
            segmented_choice_row(vec![
                SegmentedChoice {
                    label: "Rendered",
                    selected: mode == MarkdownPreviewMode::Rendered,
                    message: Message::MarkdownPreviewModeSelected(MarkdownPreviewMode::Rendered),
                    tooltip: None,
                },
                SegmentedChoice {
                    label: "Raw",
                    selected: mode == MarkdownPreviewMode::Raw,
                    message: Message::MarkdownPreviewModeSelected(MarkdownPreviewMode::Raw),
                    tooltip: None,
                },
            ])
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
            label: "Table Data",
            selected: active_tab == SqlitePreviewTab::Tables,
            message: Message::SqlitePreview(SqlitePreviewMessage::TabSelected(
                SqlitePreviewTab::Tables,
            )),
            tooltip: None,
        },
        SegmentedChoice {
            label: "SQL Query",
            selected: active_tab == SqlitePreviewTab::Sql,
            message: Message::SqlitePreview(SqlitePreviewMessage::TabSelected(
                SqlitePreviewTab::Sql,
            )),
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
        on_scroll: Box::new(scrollbar_on_scroll(
            ScrollbarRegion::PreviewDirectory,
            |_| Message::PreviewDirectoryScrolled,
        )),
    }
}

fn archive_scroll_wiring() -> ScrollRegionWiring<'static, Message> {
    ScrollRegionWiring {
        smooth_scroll_wrap: Box::new(|content| {
            smooth_scroll_content(content, ScrollbarRegion::PreviewArchive)
        }),
        scrollable_id: smooth_scroll_id(&ScrollbarRegion::PreviewArchive),
        on_scroll: Box::new(scrollbar_on_scroll(ScrollbarRegion::PreviewArchive, |_| {
            Message::PreviewArchiveScrolled
        })),
    }
}

// 文档分页滚动接线：视口事件合成闭包留在宿主（scrollbar_on_scroll 包
// ScrollbarViewportChanged），key 是文档会话的视口身份，滚轮/拖动偏移
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
        on_scroll: Box::new(scrollbar_on_scroll(
            ScrollbarRegion::PreviewDocument,
            move |viewport| {
                Message::DocumentPreview(DocumentPreviewMessage::Scrolled {
                    key: key.clone(),
                    offset_y: viewport.absolute_offset().y,
                    viewport_height: viewport.bounds().height,
                    content_height: viewport.content_bounds().height,
                })
            },
        )),
    }
}

// 纯文本几何宿主接线：视口回传把几何宿主的滚动位置同步给查看器
// （TextPreviewViewportSynced，查看器从动不回推，见
// text-preview-scrolling-guidelines）。'a 出现在 Fn 参数位（不变），
// 须在面板借用期上实例化。
fn text_scroll_wiring<'a>() -> ScrollRegionWiring<'a, Message> {
    ScrollRegionWiring {
        smooth_scroll_wrap: Box::new(|content| {
            smooth_scroll_content(content, ScrollbarRegion::TextPreview)
        }),
        scrollable_id: smooth_scroll_id(&ScrollbarRegion::TextPreview),
        on_scroll: Box::new(scrollbar_on_scroll(
            ScrollbarRegion::TextPreview,
            |viewport| Message::TextPreviewViewportSynced {
                offset_y: viewport.absolute_offset().y,
                viewport_height: viewport.bounds().height,
            },
        )),
    }
}

// Markdown 渲染体接线：滚动偏移随视口回传归档到文档滚动状态。
fn markdown_scroll_wiring<'a>() -> ScrollRegionWiring<'a, Message> {
    ScrollRegionWiring {
        smooth_scroll_wrap: Box::new(|content| {
            smooth_scroll_content(content, ScrollbarRegion::MarkdownPreview)
        }),
        scrollable_id: smooth_scroll_id(&ScrollbarRegion::MarkdownPreview),
        on_scroll: Box::new(scrollbar_on_scroll(
            ScrollbarRegion::MarkdownPreview,
            |viewport| {
                let offset = viewport.absolute_offset();
                let bounds = viewport.bounds();
                let content_bounds = viewport.content_bounds();
                Message::MarkdownPreviewScrolled {
                    offset_y: offset.y,
                    viewport_height: bounds.height,
                    content_height: content_bounds.height,
                }
            },
        )),
    }
}

// 表列表/数据网格各自的滚动接线：视口回传闭包只做显隐状态机记账，
// 不携带额外载荷。
fn sqlite_tables_scroll_wiring<'a>() -> ScrollRegionWiring<'a, Message> {
    ScrollRegionWiring {
        smooth_scroll_wrap: Box::new(|content| {
            smooth_scroll_content(content, ScrollbarRegion::PreviewSqliteTables)
        }),
        scrollable_id: smooth_scroll_id(&ScrollbarRegion::PreviewSqliteTables),
        on_scroll: Box::new(scrollbar_on_scroll(
            ScrollbarRegion::PreviewSqliteTables,
            |_| Message::SqlitePreviewTablesScrolled,
        )),
    }
}

fn sqlite_data_scroll_wiring<'a>() -> ScrollRegionWiring<'a, Message> {
    ScrollRegionWiring {
        smooth_scroll_wrap: Box::new(|content| {
            smooth_scroll_content(content, ScrollbarRegion::PreviewSqliteData)
        }),
        scrollable_id: smooth_scroll_id(&ScrollbarRegion::PreviewSqliteData),
        on_scroll: Box::new(scrollbar_on_scroll(
            ScrollbarRegion::PreviewSqliteData,
            |_| Message::SqlitePreviewDataScrolled,
        )),
    }
}
