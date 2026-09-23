//! 预览宿主单测：空格 toggle 状态机、会话偏好回落、窗口生命周期
//! （Esc/失焦/chrome 控制按钮/pending resize）、滚动管线复位与图片
//! 首帧缩略图三态（命中/未命中/迟到不回退）。
//!
//! Task 返回值统一用 `let _ =` 收掉：这些入口的 Task 只在真实 daemon
//! 循环里路由，单测断言对象是宿主/引擎状态本身。

use std::path::{Path, PathBuf};

use file_core::entry::{DirectoryEntry, EntryMetadata};
use file_core::FileKind;
use iced::window;

use bennu_preview::engine::SqlitePreviewState;
use bennu_preview::preview::PreviewWindowProfile;
use bennu_preview::preview::{ImagePreviewContent, PreviewContent, PreviewState};
use bennu_preview::preview_loading::classify_preview_path;
use bennu_preview::preview_message::PreviewMessage;
use bennu_preview::sqlite_preview::{SqliteDatabasePreview, SqlitePreviewTab};
use bennu_theme::scrollbar::ScrollbarViewport;
use bennu_theme::window_controls::WindowControlKind;
use thumbnails::{CachedThumbnail, ThumbnailRequest, ThumbnailSourceMetadata};

use crate::dbus_file_chooser::PickerResolution;
use crate::picker_request::{PickerKind, PickerRequestSpec};
use crate::picker_session::scan::{DirectoryScanOutcome, DirectoryScanResult};
use crate::picker_session::{PickerSession, SessionMessage, SessionScrollRegion};
use crate::preview_scroll::PreviewScrollRegion;
use crate::{PickerDaemon, Theme};

use super::content::preview_thumbnail_edge_for_size;
use super::{PreviewHost, PreviewSelection};

fn host() -> PreviewHost {
    // 指向不存在的库路径：配置读取失败 → 默认规则回落（design 决策
    // #2），同时隔离测试机上的真实主软件配置。
    let missing = tempfile::tempdir().unwrap().keep().join("state.sqlite");
    PreviewHost::new(missing)
}

fn file_selection(path: &Path, file_bytes: u64) -> PreviewSelection {
    PreviewSelection {
        path: path.to_path_buf(),
        kind: FileKind::File,
        file_bytes,
    }
}

#[test]
fn space_without_selection_is_silent() {
    let mut host = host();
    let _ = host.request_preview(None, None);
    assert!(host.engine.preview.is_none());
    assert!(host.engine.preview_window.is_none());
}

#[test]
fn space_with_selection_enters_loading() {
    let mut host = host();
    let selection = file_selection(Path::new("/tmp/notes.txt"), 16);
    let _ = host.request_preview(None, Some(&selection));
    assert!(matches!(
        &host.engine.preview,
        Some(PreviewState::Loading(path)) if path == Path::new("/tmp/notes.txt")
    ));
    assert_eq!(
        host.engine.preview_shown_path.as_deref(),
        Some(Path::new("/tmp/notes.txt"))
    );
    assert!(host.engine.preview_window.is_some());
}

#[test]
fn space_without_selection_closes_stale_session_instead_of_orphaning() {
    // portal 特有态：预览活跃期间选中被清空（导航换目录/清选）。
    // 再按空格必须收回预览窗，而不是丢掉关窗 Task 留孤儿窗。
    let mut host = host();
    let _ = host.request_preview(None, Some(&file_selection(Path::new("/tmp/a.txt"), 16)));
    assert!(host.engine.preview_window.is_some());
    let _ = host.request_preview(None, None);
    assert!(host.engine.preview.is_none());
    assert!(host.engine.preview_window.is_none());
}

#[test]
fn space_on_same_path_toggles_close() {
    let mut host = host();
    let selection = file_selection(Path::new("/tmp/notes.txt"), 16);
    let _ = host.request_preview(None, Some(&selection));
    assert!(host.standalone_session_active());
    let _ = host.request_preview(None, Some(&selection));
    assert!(host.engine.preview.is_none());
    assert!(host.engine.preview_window.is_none());
    assert!(host.engine.preview_shown_path.is_none());
    assert!(host.request_window.is_none());
}

#[test]
fn space_on_different_path_replaces_session() {
    let mut host = host();
    let _ = host.request_preview(None, Some(&file_selection(Path::new("/tmp/a.txt"), 16)));
    let _ = host.request_preview(None, Some(&file_selection(Path::new("/tmp/b.txt"), 16)));
    assert!(matches!(
        &host.engine.preview,
        Some(PreviewState::Loading(path)) if path == Path::new("/tmp/b.txt")
    ));
    assert!(host.engine.preview_window.is_some());
}

#[test]
fn inside_archive_member_is_silent_but_browsing_root_previews() {
    let base = tempfile::tempdir().unwrap();
    let archive = base.path().join("pack.zip");
    std::fs::write(&archive, b"").unwrap();

    // 包内成员：静默拦截（虚拟路径读不了文件）。
    let mut host = host();
    let member = file_selection(&archive.join("inner.txt"), 16);
    let _ = host.request_preview(None, Some(&member));
    assert!(host.engine.preview.is_none());
    assert!(host.engine.preview_window.is_none());

    // 包根本身（BrowsingRoot）：真实文件，归档预览照常发起。
    let root = file_selection(&archive, 16);
    let _ = host.request_preview(None, Some(&root));
    assert!(matches!(
        &host.engine.preview,
        Some(PreviewState::Loading(path)) if path == &archive
    ));
}

#[test]
fn oversized_file_is_rejected_with_error_window() {
    let mut host = host();
    // 默认文本上限 25 MiB（preview_config 默认值），26 MiB 必超限。
    let oversized = 26 * 1024 * 1024;
    let selection = file_selection(Path::new("/tmp/big.txt"), oversized);
    let _ = host.request_preview(None, Some(&selection));
    let Some(PreviewState::Error(message)) = &host.engine.preview else {
        panic!("超限文件必须进入错误态");
    };
    assert!(message.contains("too large to preview"));
    assert!(host.engine.preview_window.is_some());
}

#[test]
fn zero_limit_disables_size_guard() {
    // 0 = 不限制（与历史行为相反）；判定源是会话偏好。
    let mut host = host();
    let oversized = 26 * 1024 * 1024;
    let selection = file_selection(Path::new("/tmp/big.txt"), oversized);
    host.preferences.size_limits.text_bytes = 0;
    assert!(host.reject_oversized_file_preview(&selection).is_none());
    host.preferences.size_limits.text_bytes = 1024;
    assert!(host.reject_oversized_file_preview(&selection).is_some());
}

#[test]
fn unpreviewable_type_shows_error_window() {
    let mut host = host();
    let selection = file_selection(Path::new("/tmp/blob.xyz"), 16);
    let _ = host.request_preview(None, Some(&selection));
    let Some(PreviewState::Error(message)) = &host.engine.preview else {
        panic!("不可预览类型必须进入错误态");
    };
    assert_eq!(message, "No preview available for this file type.");
    assert!(host.engine.preview_window.is_some());
}

#[test]
fn image_selection_waits_for_dimensions_before_window() {
    let mut host = host();
    let selection = file_selection(Path::new("/tmp/photo.png"), 16);
    let _ = host.request_preview(None, Some(&selection));
    // 图片窗口按内容尺寸开启：尺寸探测回流前不开窗（主软件同款）。
    assert!(matches!(
        host.engine.preview,
        Some(PreviewState::Loading(_))
    ));
    assert!(host.engine.preview_window.is_none());
}

#[test]
fn audio_selection_uses_audio_profile_window() {
    let mut host = host();
    let selection = file_selection(Path::new("/tmp/track.flac"), 16);
    let _ = host.request_preview(None, Some(&selection));
    assert!(matches!(
        host.engine.preview,
        Some(PreviewState::Loading(_))
    ));
    assert_eq!(
        host.engine.preview_window_profile,
        PreviewWindowProfile::Audio
    );
    assert!(host.engine.audio_preview.is_some());
}

#[test]
fn pinned_state_survives_session_switch() {
    let mut host = host();
    let _ = host.request_preview(None, Some(&file_selection(Path::new("/tmp/a.txt"), 16)));
    host.engine.preview_window_pinned = true;
    let _ = host.request_preview(None, Some(&file_selection(Path::new("/tmp/b.txt"), 16)));
    assert!(host.engine.preview_window_pinned);
}

#[test]
fn missing_preferences_database_falls_back_to_default_rules() {
    // host() 的库路径不存在：默认规则仍把 .txt 分类为文本预览。
    let mut host = host();
    let selection = file_selection(Path::new("/tmp/notes.txt"), 16);
    let _ = host.request_preview(None, Some(&selection));
    assert!(matches!(
        host.engine.preview,
        Some(PreviewState::Loading(_))
    ));
}

#[test]
fn session_preferences_drive_classification_and_limits() {
    // 会话偏好是分类/上限的唯一判定源（open_preview 会话开始时从
    // 主软件库重读；库→域类型的解析在 bennu-preview 侧已测）。
    let mut host = host();
    host.preferences
        .extension_rules
        .text
        .retain(|extension| extension != "txt");
    assert!(classify_preview_path(
        Path::new("/tmp/notes.txt"),
        &host.preferences.extension_rules
    )
    .is_none());
    host.preferences.size_limits.text_bytes = 10;
    assert_eq!(host.file_size_limit_for(Path::new("/tmp/notes.md")), 10);
    // 未识别扩展名兜底 Text 上限。
    assert_eq!(host.file_size_limit_for(Path::new("/tmp/blob.xyz")), 10);
}

#[test]
fn window_close_replaces_session_state() {
    let mut host = host();
    let _ = host.request_preview(None, Some(&file_selection(Path::new("/tmp/a.txt"), 16)));
    let Some(preview_window) = host.engine.preview_window else {
        panic!("会话必须持有预览窗");
    };
    // 返回的关窗 Task 只负责对已亡窗口 no-op；状态复位才是断言对象。
    let _ = host.handle_window_closed(preview_window);
    assert!(host.engine.preview_window.is_none());
    assert!(host.engine.preview.is_none());
}

#[test]
fn esc_close_clears_session_and_host_bookkeeping() {
    // Esc 分层的域内收尾：会话/请求窗/滚动管线全部复位。
    let mut host = host();
    let _ = host.request_preview(None, Some(&file_selection(Path::new("/tmp/a.txt"), 16)));
    let preview_window = host.engine.preview_window.unwrap();
    host.focused_window = Some(preview_window);
    let _ = host.scroll.handle_layout_verified(
        PreviewScrollRegion::Directory,
        bennu_theme::scrollbar::ScrollbarViewport {
            offset_x: 0.0,
            offset_y: 0.0,
            viewport_width: 400.0,
            viewport_height: 300.0,
            content_width: 400.0,
            content_height: 900.0,
        },
    );
    assert!(host.scroll.is_animating());

    let _ = host.close_focused_preview();
    assert!(host.engine.preview_window.is_none());
    assert!(host.engine.preview.is_none());
    assert!(host.request_window.is_none());
    assert_eq!(host.focused_window, None);
    assert!(!host.scroll.is_animating());
}

#[test]
fn unfocused_close_honors_pinned_exception() {
    // 失焦自动关闭门禁：未钉住关闭，钉住保留（主软件
    // should_close_unfocused_window 同语义）。
    let mut host = host();
    let _ = host.request_preview(None, Some(&file_selection(Path::new("/tmp/a.txt"), 16)));
    let preview_window = host.engine.preview_window.unwrap();

    let _ = host.handle_window_unfocused(preview_window);
    assert!(host.engine.preview_window.is_none());

    let _ = host.request_preview(None, Some(&file_selection(Path::new("/tmp/b.txt"), 16)));
    let preview_window = host.engine.preview_window.unwrap();
    host.engine.preview_window_pinned = true;
    let _ = host.handle_window_unfocused(preview_window);
    assert_eq!(host.engine.preview_window, Some(preview_window));
    // 非预览窗失焦（选择窗）：不动预览会话。
    let _ = host.handle_window_unfocused(window::Id::unique());
    assert_eq!(host.engine.preview_window, Some(preview_window));
}

#[test]
fn window_control_close_only_acts_on_preview_window() {
    // chrome 关闭按钮：预览窗收尾会话；选择窗 id 一律 no-op。
    let mut host = host();
    let _ = host.request_preview(None, Some(&file_selection(Path::new("/tmp/a.txt"), 16)));
    let preview_window = host.engine.preview_window.unwrap();

    let _ = host.handle_window_control(window::Id::unique(), WindowControlKind::Close);
    assert_eq!(host.engine.preview_window, Some(preview_window));

    let _ = host.handle_window_control(preview_window, WindowControlKind::Close);
    assert!(host.engine.preview_window.is_none());
    assert!(host.engine.preview.is_none());
}

#[test]
fn resize_follows_pending_target_before_accepting_user_size() {
    // pending resize 匹配语义：未到目标尺寸时重发 resize 且不采纳
    // 当帧尺寸；匹配后清 pending 并采纳。
    let mut host = host();
    let _ = host.request_preview(None, Some(&file_selection(Path::new("/tmp/a.txt"), 16)));
    let preview_window = host.engine.preview_window.unwrap();
    let pending = host.engine.pending_preview_resize.unwrap();

    // 假 WM 回了一个错误尺寸：保持 pending（Some = 提前返回重发 resize）。
    let _ = host.handle_window_resized_engine(preview_window, 300.0, 200.0);
    let still_pending = host.engine.pending_preview_resize.unwrap();
    assert!(bennu_preview::engine::preview_size_matches(
        still_pending,
        pending,
    ));

    // 目标尺寸到达：采纳并清 pending（None = 走文档重排/档位刷新路径）。
    let _ = host.handle_window_resized_engine(preview_window, pending.width, pending.height);
    assert!(host.engine.pending_preview_resize.is_none());
    assert!(bennu_preview::engine::preview_size_matches(
        host.engine.preview_size,
        pending,
    ));

    // 无 pending 时：用户手动 resize 直接采纳。
    let _ = host.handle_window_resized_engine(preview_window, 512.0, 640.0);
    assert_eq!(host.engine.preview_size.width, 512.0);
}

#[test]
fn title_bar_press_starts_drag_reveal_and_double_click_toggles_maximize() {
    // 标题栏按下：单击进拖动态并取消初始 chrome 计时；双击切换最大化。
    let mut host = host();
    let _ = host.request_preview(None, Some(&file_selection(Path::new("/tmp/a.txt"), 16)));
    let preview_window = host.engine.preview_window.unwrap();
    host.engine.preview_window_chrome.reset_hidden();

    let _ = host.handle_title_bar_pressed(preview_window, false);
    assert!(host.engine.preview_window_drag_active);
    assert!(host.engine.preview_window_chrome.target_is_visible());

    host.engine.preview_window_drag_active = false;
    let _ = host.handle_title_bar_pressed(preview_window, true);
    assert!(!host.engine.preview_window_drag_active);
}

#[test]
fn pointer_motion_drives_chrome_and_window_chrome_tracks_sqlite_drag() {
    let mut host = host();
    let _ = host.request_preview(None, Some(&file_selection(Path::new("/tmp/a.txt"), 16)));
    let preview_window = host.engine.preview_window.unwrap();
    let picker_window = window::Id::unique();

    // 非预览窗指针事件：零动作。
    let _ = host.handle_pointer_moved(picker_window, iced::Point::new(10.0, 10.0));
    assert_eq!(host.engine.preview_window_pointer_y, None);

    // 媒体类预览：指针位置驱动 chrome/底部控件。
    let _ = host.handle_pointer_moved(preview_window, iced::Point::new(10.0, 8.0));
    assert_eq!(host.engine.preview_window_pointer_y, Some(8.0));
    assert!(host.engine.preview_window_chrome.target_is_visible());

    // 指针离开：隐藏 chrome、结束平移。
    let _ = host.handle_pointer_left(preview_window);
    assert_eq!(host.engine.preview_window_pointer_y, None);
    assert!(!host.engine.preview_window_chrome.target_is_visible());
}

#[test]
fn maximize_observation_updates_chrome_frame_state() {
    let mut host = host();
    let _ = host.request_preview(None, Some(&file_selection(Path::new("/tmp/a.txt"), 16)));
    let preview_window = host.engine.preview_window.unwrap();
    assert!(!host.preview_window_maximized);
    host.accept_maximize_observed(preview_window, true);
    assert!(host.preview_window_maximized);
    // 非预览窗的观测不落簿记。
    host.accept_maximize_observed(window::Id::unique(), false);
    assert!(host.preview_window_maximized);
}

// ---------------------------------------------------------------------------
// 图片首帧缩略图三态（主软件 request_preview_thumbnail_for_entry /
// accept_preview_thumbnail_ready 的 portal 对齐锚点）。
// ---------------------------------------------------------------------------

/// 测试专用 daemon：单张图片行的选择窗会话（绕过 new() 的主题解析，
/// 其需 tokio reactor）。
fn daemon_with_image_row(file_name: &str) -> (PickerDaemon, window::Id, PathBuf) {
    let (reply, _receiver) = tokio::sync::oneshot::channel::<PickerResolution>();
    let base = tempfile::tempdir().unwrap();
    let mut session = PickerSession::new(
        &PickerRequestSpec {
            kind: PickerKind::OpenFile {
                multiple: false,
                directory: false,
            },
            accept_label: None,
            title: None,
            filters: Vec::new(),
            active_filter: None,
            start_folder: None,
        },
        "/req/preview".to_string(),
        base.keep(),
        reply,
    );
    let path = session.directory().join(file_name);
    session.apply_scan(DirectoryScanResult {
        directory: session.directory().to_path_buf(),
        outcome: Ok(DirectoryScanOutcome {
            entries: vec![DirectoryEntry::new(
                path.clone(),
                FileKind::File,
                EntryMetadata::default(),
                false,
                false,
                false,
            )],
        }),
    });
    let mut daemon = PickerDaemon {
        windows: std::collections::HashMap::new(),
        theme: Theme::Light,
        keyboard_modifiers: iced::keyboard::Modifiers::default(),
        cursor_position: None,
        sidebar_resize: std::collections::HashMap::new(),
        preview: PreviewHost::new(PathBuf::new()),
    };
    let window = window::Id::unique();
    daemon.windows.insert(window, session);
    (daemon, window, path)
}

/// 走真实行内管线种入一张 128 档就绪缩略图：装视口 → 可见区入队 →
/// drain → 回信（与 picker_session::thumbnails 测试同手法）。
fn seed_row_thumbnail(daemon: &mut PickerDaemon, window: window::Id) {
    let session = daemon.windows.get_mut(&window).unwrap();
    session.handle_scroll_message(SessionMessage::ScrollbarLayoutVerified {
        region: SessionScrollRegion::List,
        viewport: ScrollbarViewport {
            offset_x: 0.0,
            offset_y: 0.0,
            viewport_width: 600.0,
            viewport_height: 300.0,
            content_width: 600.0,
            content_height: 60.0,
        },
    });
    session.schedule_visible_thumbnails();
    for request in session.drain_pending_thumbnail_requests() {
        session.accept_thumbnail_ready(request.clone(), Ok(fake_cached(&request, 128, 128)));
    }
}

fn fake_cached(request: &ThumbnailRequest, width: u32, height: u32) -> CachedThumbnail {
    CachedThumbnail {
        key: request.key(),
        source: request.source.clone(),
        output: PathBuf::from("/tmp/fake-thumbnail.png"),
        width,
        height,
        cache_hit: true,
    }
}

fn open_image_preview_loading(daemon: &mut PickerDaemon, window: window::Id, path: &Path) -> u64 {
    let _ = daemon
        .preview
        .request_preview(Some(window), Some(&file_selection(path, 16)));
    daemon.preview.engine.original_image_preview_generation
}

fn displayed_thumbnail_edge(host: &PreviewHost) -> Option<u32> {
    host.displayed_thumbnail_edge().map(|(_, edge)| edge)
}

fn preview_edge_for_test_dimensions() -> u32 {
    preview_thumbnail_edge_for_size(bennu_preview::engine::image_preview_size_from_dimensions(
        2000, 3000,
    ))
}

fn test_metadata() -> ThumbnailSourceMetadata {
    ThumbnailSourceMetadata {
        len: 0,
        modified: None,
    }
}

fn preview_tier_request(path: &Path) -> ThumbnailRequest {
    ThumbnailRequest::new(path, test_metadata(), preview_edge_for_test_dimensions())
}

#[test]
fn image_first_frame_hit_shows_thumbnail_without_pending_mark() {
    // 命中：就绪表已有 128 档小图 → 首帧立即上屏，无等待标记（档位
    // 升级入队与原图加载已随 Task 发起，不在此断言）。
    let (mut daemon, window, path) = daemon_with_image_row("photo.png");
    seed_row_thumbnail(&mut daemon, window);
    let generation = open_image_preview_loading(&mut daemon, window, &path);

    let _ = PreviewHost::accept_image_preview_dimensions(
        &mut daemon,
        path.clone(),
        generation,
        Ok((2000, 3000)),
    );

    let host = &daemon.preview;
    assert!(matches!(
        host.engine.preview,
        Some(PreviewState::Ready(PreviewContent::Image(
            ImagePreviewContent::Thumbnail { max_edge: 128, .. }
        )))
    ));
    assert!(host.engine.pending_preview_thumbnail_display.is_none());
    // 等待标记不命中（命中分支已展示，无需替换 Loading）。
    assert!(!host
        .engine
        .pending_preview_thumbnail_display_matches(&preview_tier_request(&path)));
}

#[test]
fn session_enqueues_preview_tier_with_dedup() {
    // 预览档位入队原语：新请求入队等待；同 key 重复入队合并（仍等待）；
    // drain 出队的正是预览档位。
    let (mut daemon, window, path) = daemon_with_image_row("photo.png");
    let request = preview_tier_request(&path);
    let session = daemon.windows.get_mut(&window).unwrap();
    assert!(session.enqueue_preview_thumbnail_request(request.clone()));
    assert!(session.enqueue_preview_thumbnail_request(request.clone()));
    let drained = session.drain_pending_thumbnail_requests();
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].max_edge, preview_edge_for_test_dimensions());
    assert_eq!(drained[0].source, path);
}

#[test]
fn image_first_frame_miss_marks_pending_and_cleared_by_failure() {
    // 未命中：无就绪缩略图 → 并行生成预览档位 + 等待标记（key 与档位
    // 请求一致）；失败回信清除标记，Loading 保持（原图解码兑底，不因
    // 缩略图卡死）。
    let (mut daemon, window, path) = daemon_with_image_row("photo.png");
    let generation = open_image_preview_loading(&mut daemon, window, &path);

    let _ = PreviewHost::accept_image_preview_dimensions(
        &mut daemon,
        path.clone(),
        generation,
        Ok((2000, 3000)),
    );

    let host = &mut daemon.preview;
    assert!(matches!(
        host.engine.preview,
        Some(PreviewState::Loading(_))
    ));
    let request = preview_tier_request(&path);
    assert!(host
        .engine
        .pending_preview_thumbnail_display_matches(&request));

    let _ = host.accept_session_thumbnail_outcome(
        &request,
        &Err(crate::picker_session::thumbnails::ThumbnailLoadFailed),
    );
    assert!(host.engine.pending_preview_thumbnail_display.is_none());
    assert!(matches!(
        host.engine.preview,
        Some(PreviewState::Loading(_))
    ));
}

#[test]
fn image_first_frame_pending_replaced_by_ready_reply() {
    // 未命中后的成功回信：等待标记命中 → Loading 换首帧。
    let (mut daemon, _window, path) = daemon_with_image_row("photo.png");
    let generation = open_image_preview_loading(&mut daemon, _window, &path);
    let _ = PreviewHost::accept_image_preview_dimensions(
        &mut daemon,
        path.clone(),
        generation,
        Ok((2000, 3000)),
    );

    let request = preview_tier_request(&path);
    let edge = preview_edge_for_test_dimensions();
    let ready = fake_cached(&request, edge, edge);
    let _ = daemon
        .preview
        .accept_session_thumbnail_outcome(&request, &Ok(ready));
    assert_eq!(displayed_thumbnail_edge(&daemon.preview), Some(edge));
    assert!(daemon
        .preview
        .engine
        .pending_preview_thumbnail_display
        .is_none());
}

#[test]
fn late_smaller_thumbnail_never_downgrades_display() {
    // 迟到不回退：展示已是预览档位首帧后，更小的回信（128）不得替换；
    // 更大的回信才升级。
    let (mut daemon, window, path) = daemon_with_image_row("photo.png");
    let generation = open_image_preview_loading(&mut daemon, window, &path);
    let _ = PreviewHost::accept_image_preview_dimensions(
        &mut daemon,
        path.clone(),
        generation,
        Ok((2000, 3000)),
    );

    let edge = preview_edge_for_test_dimensions();
    let tier_request = preview_tier_request(&path);
    let _ = daemon.preview.accept_session_thumbnail_outcome(
        &tier_request,
        &Ok(fake_cached(&tier_request, edge, edge)),
    );
    assert_eq!(displayed_thumbnail_edge(&daemon.preview), Some(edge));

    // 迟到的小图：不回退。
    let small = ThumbnailRequest::new(&path, test_metadata(), 128);
    let _ = daemon
        .preview
        .accept_session_thumbnail_outcome(&small, &Ok(fake_cached(&small, 128, 128)));
    assert_eq!(displayed_thumbnail_edge(&daemon.preview), Some(edge));

    // 更大：升级。
    let bigger = ThumbnailRequest::new(&path, test_metadata(), 1024);
    let _ = daemon
        .preview
        .accept_session_thumbnail_outcome(&bigger, &Ok(fake_cached(&bigger, 1024, 1024)));
    assert_eq!(displayed_thumbnail_edge(&daemon.preview), Some(1024));
}

#[test]
fn image_dimensions_without_session_falls_back_to_original_only() {
    // 请求窗已亡（会话不在）：无缩略图档位可查，原图直解，状态仍就绪。
    let (mut daemon, window, path) = daemon_with_image_row("photo.png");
    let generation = open_image_preview_loading(&mut daemon, window, &path);
    daemon.preview.request_window = None;

    let _ = PreviewHost::accept_image_preview_dimensions(
        &mut daemon,
        path.clone(),
        generation,
        Ok((2000, 3000)),
    );

    // 仍处 Loading（原图命令已发起，回信才上屏）；无等待标记。
    let host = &daemon.preview;
    assert!(matches!(
        host.engine.preview,
        Some(PreviewState::Loading(_))
    ));
    assert!(host.engine.pending_preview_thumbnail_display.is_none());
}

#[test]
fn sqlite_drag_start_uses_tracked_pointer_x() {
    // SQLite 表格列宽拖拽：起点横坐标来自宿主指针簿记（标准窗口
    // chrome 分支同样追踪）。
    let mut host = host();
    let _ = host.request_preview(None, Some(&file_selection(Path::new("/tmp/a.txt"), 16)));
    let preview_window = host.engine.preview_window.unwrap();
    let _ = host.handle_pointer_moved(preview_window, iced::Point::new(240.0, 10.0));

    host.engine.sqlite_preview = Some(SqlitePreviewState {
        path: PathBuf::from("/tmp/a.txt"),
        generation: 1,
        active_tab: SqlitePreviewTab::Tables,
        selected_table: None,
        table_loading: false,
        table_data: None,
        sql_text: String::new(),
        sql_running: false,
        sql_result: None,
        table_filter: String::new(),
        tables_width: 240.0,
    });
    // active_sqlite_preview_mut 还要求展示态为 Sqlite 内容。
    host.engine.preview = Some(PreviewState::Ready(PreviewContent::Sqlite(
        SqliteDatabasePreview { tables: Vec::new() },
    )));
    let _ = host.route_engine_fallback(PreviewMessage::SqliteTablesResizeStarted);
    assert!(host.engine.sqlite_tables_resize_drag.is_some());
    // 拖拽跟随：指针移到 x=300 → 宽度 = 240 + (300-240) = 300（未触
    // 上下限，直接验证起点坐标被正确采样）。
    let _ = host.handle_pointer_moved(preview_window, iced::Point::new(300.0, 10.0));
    let width = host
        .engine
        .sqlite_preview
        .as_ref()
        .expect("sqlite preview")
        .tables_width;
    assert_eq!(width, 300.0);
    // 无指针簿记（未移动过即按下，或指针已离开）：拖拽不启动。
    let _ = host.finish_window_drags();
    host.preview_window_pointer = None;
    let _ = host.route_engine_fallback(PreviewMessage::SqliteTablesResizeStarted);
    assert!(host.engine.sqlite_tables_resize_drag.is_none());
}
