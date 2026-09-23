//! 预览内容回流：引擎不处理消息的宿主回退路由（From 散装分支的
//! 对齐物）、图片尺寸/原图/动图/文档/目录子级回流与首帧缩略图接线。
//! 生命周期/开关/窗口簿记在父模块（[`super`]），本模块只做内容态的
//! 接收与升级。

use std::path::PathBuf;

use file_core::{DirectoryEntry, FileKind, ScanOptions};
use iced::window;
use iced::Task;

use bennu_preview::commands::preview::original_image_preview_command;
use bennu_preview::document_preview::DocumentPreviewMessage;
use bennu_preview::engine::{
    image_preview_size_from_dimensions, PendingPreviewThumbnailDisplay, PreviewLoadSurface,
};
use bennu_preview::preview::{ImagePreviewContent, PreviewContent, PreviewSize, PreviewState};
use bennu_preview::preview_message::PreviewMessage;
use bennu_preview::text_preview_viewer::TEXT_PREVIEW_VIEWER_ID;
use thumbnails::{CachedThumbnail, ThumbnailRequest, ThumbnailSourceMetadata};

use crate::picker_session::thumbnails::ThumbnailLoadFailed;
use crate::preview_scroll::{preview_scroll_id, PreviewScrollRegion};
use crate::thumbnail_dispatch::spawn_thumbnail_tasks;
use crate::{Message, PickerDaemon};

use super::PreviewHost;

/// 预览档位缩略图的最小/最大边长与升级阈值（主软件同值：512/2048/
/// 128；PREVIEW_THUMBNAIL_MAX_EDGE 主软件在 thumbnail_cache.rs）。
const PREVIEW_THUMBNAIL_MIN_EDGE: u32 = 512;
const PREVIEW_THUMBNAIL_MAX_EDGE: u32 = 2048;
const PREVIEW_RESIZE_EXTRA_PIXELS: u32 = 128;

/// Message::Preview 的 update 入口（app-ui update.rs:38 模式）：引擎
/// 直连变体优先；None 回退经本层路由到宿主侧接收器；剩余变体随
/// 步骤 3/4 接入，当前安全丢弃。
pub(crate) fn handle_preview_message(
    daemon: &mut PickerDaemon,
    message: PreviewMessage,
) -> Task<Message> {
    let task = match message {
        // 尺寸探测回流需要会话缩略图体系（Task 已是 Message 域：含缩略
        // 图泵送），在进入引擎前拦截并提前返回。
        PreviewMessage::ImagePreviewDimensionsLoaded(path, generation, outcome) => {
            let task =
                PreviewHost::accept_image_preview_dimensions(daemon, path, generation, outcome);
            daemon.preview.sync_focused_window();
            return task;
        }
        other => {
            let host = &mut daemon.preview;
            match host.engine.handle(other.clone()) {
                Some(task) => task,
                None => host.route_engine_fallback(other),
            }
        }
    };
    daemon.preview.sync_focused_window();
    task.map(Message::Preview)
}

/// 预览窗尺寸变化的 update 入口（main.rs 直转）：pending resize 匹配
/// 语义 + 底部控件刷新（引擎侧）；文档重排与缩略图档位刷新需会话
/// 体系，在此收口（主软件 handle_auxiliary_window_resized 预览分支的
/// resize_document_preview + refresh_preview_thumbnail_for_size 同批）。
pub(crate) fn handle_window_resized(
    daemon: &mut PickerDaemon,
    window: window::Id,
    width: f32,
    height: f32,
) -> Task<Message> {
    if let Some(task) = daemon
        .preview
        .handle_window_resized_engine(window, width, height)
    {
        return task;
    }
    Task::batch([
        daemon
            .preview
            .engine
            .resize_document_preview(preview_scroll_id(PreviewScrollRegion::Document))
            .map(Message::Preview),
        refresh_preview_thumbnail_after_resize(daemon),
    ])
}

/// 用户放大预览窗后按新尺寸升级缩略图档位（主软件 refresh_preview_
/// thumbnail_for_size + request_preview_thumbnail_refresh 的 portal 版）：
/// 档位增量不足阈值跳过；就绪表已有足够档位直接换显示；否则入队
/// 预览档位（升级回流走 accept_session_thumbnail_outcome 的档位比较，
/// 不设等待标记——展示已是缩略图，无 Loading 可替换）。
fn refresh_preview_thumbnail_after_resize(daemon: &mut PickerDaemon) -> Task<Message> {
    let host = &daemon.preview;
    let Some((path, current_edge)) = host.displayed_thumbnail_edge() else {
        return Task::none();
    };
    let desired_edge = preview_thumbnail_edge_for_size(host.engine.preview_size);
    if desired_edge <= current_edge.saturating_add(PREVIEW_RESIZE_EXTRA_PIXELS) {
        return Task::none();
    }
    let Some(request_window) = host.request_window else {
        return Task::none();
    };
    let Some(session) = daemon.windows.get_mut(&request_window) else {
        return Task::none();
    };
    if let Some(ready) = session.thumbnail_ready(&path).cloned() {
        let ready_edge = ready.width.max(ready.height);
        if ready_edge >= desired_edge {
            daemon
                .preview
                .set_thumbnail_display(path, &ready, ready_edge);
            return Task::none();
        }
    }
    let Some(entry) = session.directory_entry_for_path(&path).cloned() else {
        return Task::none();
    };
    let Some(request) = preview_thumbnail_request(&entry, desired_edge) else {
        return Task::none();
    };
    session.enqueue_preview_thumbnail_request(request);
    Task::batch(spawn_thumbnail_tasks(request_window, session))
}

/// 主软件 request_for_entry 的对齐物：非文件/非缩略图支持路径不产生
/// 预览档位请求（视频归缩略图体系但 portal 行内不生成，档位请求与
/// 主软件同门禁）。
fn preview_thumbnail_request(entry: &DirectoryEntry, max_edge: u32) -> Option<ThumbnailRequest> {
    if entry.kind != FileKind::File || !thumbnails::is_supported_thumbnail_path(&entry.path) {
        return None;
    }
    Some(ThumbnailRequest::new(
        &entry.path,
        ThumbnailSourceMetadata::from(&entry.metadata),
        max_edge,
    ))
}

/// 预览档位边长（主软件 preview_thumbnail_edge_for_size 同式）：窗口
/// 内容尺寸长边，钳在 [512, 2048]。
pub(super) fn preview_thumbnail_edge_for_size(size: PreviewSize) -> u32 {
    size.width
        .max(size.height)
        .ceil()
        .max(PREVIEW_THUMBNAIL_MIN_EDGE as f32)
        .min(PREVIEW_THUMBNAIL_MAX_EDGE as f32) as u32
}

impl PreviewHost {
    /// 图片尺寸探测回流（主软件 accept_image_preview_dimensions 的
    /// portal 版；宿主侧逻辑，引擎未迁）：按内容尺寸开窗 + 「缩略图
    /// 先行」首帧接线 + 并行发起原图加载。
    pub(super) fn accept_image_preview_dimensions(
        daemon: &mut PickerDaemon,
        path: PathBuf,
        generation: u64,
        dimensions: Result<(u32, u32), String>,
    ) -> Task<Message> {
        let host = &mut daemon.preview;
        let active_loading = generation == host.engine.original_image_preview_generation
            && matches!(
                &host.engine.preview,
                Some(PreviewState::Loading(current)) if current == &path
            );
        if !active_loading {
            return Task::none();
        }
        let (width, height) = match dimensions {
            Ok((width, height)) if width > 0 && height > 0 => (width, height),
            Ok(_) => {
                return host
                    .fail_image_preview_dimensions(
                        path,
                        "Image preview has invalid dimensions".to_owned(),
                    )
                    .map(Message::Preview)
            }
            Err(error) => {
                return host
                    .fail_image_preview_dimensions(path, error)
                    .map(Message::Preview)
            }
        };
        let window_command =
            if host.engine.preview_load_surface == PreviewLoadSurface::StandaloneWindow {
                host.engine
                    .open_image_preview_window_for_dimensions(width, height)
                    .map(Message::Preview)
            } else {
                Task::none()
            };
        let cancellation = host
            .engine
            .original_image_preview_cancel
            .clone()
            .expect("image preview generation must own cancellation");
        let max_file_bytes = host.preferences.size_limits.image_bytes;
        let preview_edge =
            preview_thumbnail_edge_for_size(image_preview_size_from_dimensions(width, height));
        let original_command = |placeholder: Option<iced::widget::image::Handle>| {
            original_image_preview_command(
                path.clone(),
                generation,
                max_file_bytes,
                placeholder,
                cancellation.clone(),
            )
            .map(Message::Preview)
        };

        // 请求窗已亡或已不含该路径（导航走了）：无档位可查，原图直解
        // （主软件 entry_for_path 为 None 的同款回落）。
        let Some(request_window) = daemon.preview.request_window else {
            return Task::batch([window_command, original_command(None)]);
        };
        let Some(session) = daemon.windows.get_mut(&request_window) else {
            return Task::batch([window_command, original_command(None)]);
        };
        let Some(entry) = session.directory_entry_for_path(&path).cloned() else {
            return Task::batch([window_command, original_command(None)]);
        };
        let Some(request) = preview_thumbnail_request(&entry, preview_edge) else {
            return Task::batch([window_command, original_command(None)]);
        };

        // 首帧不变量（主软件同款）：任一就绪缩略图（含列表阶段的
        // 128 档小图）立即铺满上屏，原图并行解码；预览档位缩略图只作
        // 展示替换，永不阻塞原图启动。
        if let Some(ready) = session.thumbnail_ready(&path).cloned() {
            let ready_edge = ready.width.max(ready.height);
            daemon
                .preview
                .set_thumbnail_display(path.clone(), &ready, ready_edge);
            let placeholder = Some(iced::widget::image::Handle::from_path(&ready.output));
            if preview_edge > ready_edge.saturating_add(PREVIEW_RESIZE_EXTRA_PIXELS) {
                // 已有缩略图明显小于预览档位：并行生成预览档位升级展示。
                session.enqueue_preview_thumbnail_request(request);
                let pump = Task::batch(spawn_thumbnail_tasks(request_window, session));
                return Task::batch([window_command, pump, original_command(placeholder)]);
            }
            return Task::batch([window_command, original_command(placeholder)]);
        }

        // 无就绪缩略图：并行生成预览档位，到达即铺满上屏；原图解码
        // 互不等待。等待标记只决定探测回流是否替换 Loading 展示
        // （命中判定与标记清理在引擎，见 pending_preview_thumbnail_
        // display_matches）。
        let waits_for_thumbnail = session.enqueue_preview_thumbnail_request(request.clone());
        if waits_for_thumbnail {
            daemon.preview.engine.pending_preview_thumbnail_display = Some(
                PendingPreviewThumbnailDisplay::new(path.clone(), generation, request.key()),
            );
        }
        let pump = Task::batch(spawn_thumbnail_tasks(request_window, session));
        Task::batch([window_command, pump, original_command(None)])
    }

    /// 尺寸无效/读取失败的统一出口：主软件另弹全局错误 Toast，portal
    /// 无全局通知域，降级为日志 + 关闭会话（窗口此时尚未开启）。
    fn fail_image_preview_dimensions(
        &mut self,
        path: PathBuf,
        error: String,
    ) -> Task<PreviewMessage> {
        tracing::warn!(
            target: "portal::preview",
            path = ?path,
            error = %error,
            "image preview dimensions failed"
        );
        self.close_preview_session()
    }

    /// 引擎不处理变体的回退路由（app-ui 散装分支的对齐物：视图/滚动/
    /// 宿主簿记留在宿主侧的部分）。
    pub(super) fn route_engine_fallback(
        &mut self,
        message: PreviewMessage,
    ) -> Task<PreviewMessage> {
        match message {
            PreviewMessage::PreviewLoaded(path, outcome) => {
                let (_, task) = self.engine.accept_preview(
                    path,
                    outcome,
                    ScanOptions::default(),
                    self.preferences.directory_expand_levels,
                );
                task
            }
            PreviewMessage::ImagePreviewDimensionsLoaded(..) => {
                unreachable!("尺寸探测回流在 handle_preview_message 入口拦截")
            }
            PreviewMessage::OriginalImagePreviewLoaded(path, generation, outcome) => self
                .engine
                .accept_original_image_preview(path, generation, outcome),
            PreviewMessage::AnimatedImagePreviewLoaded(path, generation, outcome) => {
                self.engine
                    .accept_animated_image_preview_loaded(path, generation, outcome)
                    .1
            }
            // SQLite 表格列宽拖拽起点：横坐标来自宿主指针簿记（主软件
            // cursor_position.x 的预览对应物；引擎不持有宿主输入）。
            PreviewMessage::SqliteTablesResizeStarted => {
                match self.preview_window_pointer.map(|point| point.x) {
                    Some(x) => self.engine.start_sqlite_tables_resize_drag(x),
                    None => Task::none(),
                }
            }
            PreviewMessage::DocumentPreview(inner) => self.route_document_preview_message(inner),
            PreviewMessage::PreviewDirectoryChildrenLoaded(parent_path, outcome) => {
                self.engine.accept_preview_directory_children(
                    parent_path,
                    outcome,
                    ScanOptions::default(),
                    self.preferences.directory_expand_levels,
                )
            }
            // 图片缩放/平移/重置：面板事件直达引擎（主软件
            // update_preview_image_viewport 的转发）。
            PreviewMessage::PreviewImageViewport(viewport_message) => self
                .engine
                .update_preview_image_viewport(viewport_message, self.engine.preview_size),
            // 预览树目录展开开关（主软件 toggle_preview_tree_directory）。
            PreviewMessage::PreviewTreeDirectoryToggled(entry_id) => self
                .engine
                .toggle_preview_tree_directory(entry_id, ScanOptions::default()),
            // 解码失败重试：重新走分类分发（主软件 retry_image_preview）。
            PreviewMessage::RetryImagePreview(path) => {
                self.open_preview_for_resolved_path(path, FileKind::File)
            }
            // 滚动几何宿主 → 查看器从动（scroll_to 穿透到查看器内部滚动）。
            PreviewMessage::TextPreviewViewportSynced { offset_y, .. } => {
                iced::widget::operation::scroll_to(
                    iced::widget::Id::new(TEXT_PREVIEW_VIEWER_ID),
                    iced::widget::scrollable::AbsoluteOffset {
                        x: 0.0,
                        y: offset_y,
                    },
                )
            }
            // 查看器内部滚动：镜像预取分块 + 把总偏移同步回几何宿主滚动区。
            PreviewMessage::TextPreviewViewerScrolled {
                lines,
                offset_y,
                viewport_height,
            } => {
                let chunk_task = self
                    .engine
                    .handle_text_preview_content_scrolled(lines, viewport_height);
                let sync_task = iced::widget::operation::scroll_to(
                    preview_scroll_id(PreviewScrollRegion::Text),
                    iced::widget::scrollable::AbsoluteOffset {
                        x: 0.0,
                        y: offset_y,
                    },
                );
                Task::batch([chunk_task, sync_task])
            }
            PreviewMessage::TextPreviewContentHeightChanged(content_height) => {
                self.engine.text_preview_content_height = content_height;
                Task::none()
            }
            PreviewMessage::MarkdownPreviewScrolled {
                offset_y,
                viewport_height,
                content_height,
            } => {
                // 主软件：滚动条淡入与文档预取分块同批（update.rs 的
                // MarkdownPreviewScrolled 分支）。
                let reveal = self
                    .scroll
                    .layout_probe_task(PreviewScrollRegion::Markdown)
                    .discard();
                let engine_task = self.engine.handle_markdown_preview_scrolled(
                    offset_y,
                    viewport_height,
                    content_height,
                );
                Task::batch([engine_task, reveal])
            }
            PreviewMessage::MarkdownPreviewModeSelected(mode) => {
                if let Some(document) = self.engine.text_preview_document.as_mut() {
                    document.select_markdown_preview_mode(mode);
                }
                Task::none()
            }
            // 滚动条显隐的裸标记：显示入口唯一（先探布局再淡入，探针回信
            // 走全局消息路由）。
            PreviewMessage::PreviewDirectoryScrolled => self
                .scroll
                .layout_probe_task(PreviewScrollRegion::Directory)
                .discard(),
            PreviewMessage::PreviewArchiveScrolled => self
                .scroll
                .layout_probe_task(PreviewScrollRegion::Archive)
                .discard(),
            PreviewMessage::SqlitePreviewTablesScrolled => self
                .scroll
                .layout_probe_task(PreviewScrollRegion::SqliteTables)
                .discard(),
            PreviewMessage::SqlitePreviewDataScrolled => self
                .scroll
                .layout_probe_task(PreviewScrollRegion::SqliteData)
                .discard(),
            // 远程下载/右侧面板/设置输入域变体 portal 无对应物（远程管线
            // 不移植、无停靠面板与设置窗），持续丢弃。
            _ => Task::none(),
        }
    }

    fn route_document_preview_message(
        &mut self,
        message: DocumentPreviewMessage,
    ) -> Task<PreviewMessage> {
        match message {
            DocumentPreviewMessage::Prepared(outcome) => {
                // 独立窗口会话的排版视口 = 当前预览窗尺寸（主软件
                // preview_surface_viewport 的 StandaloneWindow 分支）。
                let layout_size = self.engine.preview_size;
                self.engine.accept_document_preview_prepared(
                    outcome,
                    layout_size,
                    preview_scroll_id(PreviewScrollRegion::Document),
                )
            }
            DocumentPreviewMessage::PageRendered(outcome) => {
                self.engine.accept_document_page_rendered(outcome)
            }
            // 文档滚动偏移归档到渲染代（None = 过期/非法丢弃）。
            DocumentPreviewMessage::Scrolled {
                key,
                offset_y,
                viewport_height,
                content_height,
            } => match self.engine.handle_document_preview_scrolled(
                key,
                offset_y,
                viewport_height,
                content_height,
            ) {
                // 引擎接受时同批淡入滚动条（主软件 handle_document_
                // preview_scrolled 的 batch）。
                Some(command) => Task::batch([
                    self.scroll
                        .layout_probe_task(PreviewScrollRegion::Document)
                        .discard(),
                    command,
                ]),
                None => Task::none(),
            },
        }
    }

    /// 当前展示若为缩略图首帧，返回（源路径，档位边长）。
    pub(super) fn displayed_thumbnail_edge(&self) -> Option<(PathBuf, u32)> {
        match &self.engine.preview {
            Some(PreviewState::Ready(PreviewContent::Image(ImagePreviewContent::Thumbnail {
                path,
                max_edge,
                ..
            }))) => Some((path.clone(), *max_edge)),
            _ => None,
        }
    }

    /// 缩略图首帧上屏（主软件 thumbnail_preview_content 的 portal 版：
    /// handle 从磁盘缓存路径惰性加载，不驻留像素）。
    fn set_thumbnail_display(&mut self, path: PathBuf, ready: &CachedThumbnail, max_edge: u32) {
        self.engine.preview = Some(PreviewState::Ready(PreviewContent::Image(
            ImagePreviewContent::Thumbnail {
                path,
                handle: iced::widget::image::Handle::from_path(&ready.output),
                width: ready.width,
                height: ready.height,
                max_edge,
            },
        )));
    }

    /// 会话缩略图回信的抄送入口（main 层在 SessionMessage::ThumbnailReady
    /// 路径调用；主软件 accept_preview_thumbnail_ready/unavailable 的
    /// portal 版）：命中等待标记 → Loading 换首帧；档位升级 → 替换展示；
    /// 小图不替换大图（迟到不回退；命中判定与标记清理在引擎）。
    pub(crate) fn accept_session_thumbnail_outcome(
        &mut self,
        request: &ThumbnailRequest,
        outcome: &Result<CachedThumbnail, ThumbnailLoadFailed>,
    ) -> Task<Message> {
        match outcome {
            Ok(ready) => {
                let ready_edge = ready.width.max(ready.height);
                if self
                    .engine
                    .pending_preview_thumbnail_display_matches(request)
                {
                    self.engine.pending_preview_thumbnail_display = None;
                    self.set_thumbnail_display(request.source.clone(), ready, ready_edge);
                } else if let Some((path, current_edge)) = self.displayed_thumbnail_edge() {
                    if path == request.source && ready_edge >= current_edge {
                        self.set_thumbnail_display(path, ready, ready_edge);
                    }
                }
            }
            Err(_) => {
                if self
                    .engine
                    .pending_preview_thumbnail_display_matches(request)
                {
                    self.engine.pending_preview_thumbnail_display = None;
                }
            }
        }
        Task::none()
    }
}
