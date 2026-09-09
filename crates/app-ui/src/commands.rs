use std::collections::HashMap;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use desktop_linux::{
    open_path_with_application, open_path_with_terminal_emulator, open_terminal_at_directory,
    open_with_applications, read_desktop_clipboard, write_file_clipboard, FileClipboardSelection,
    OpenWithLaunchMode, TerminalEmulator,
};
use file_core::{
    available_transfer_target_path, check_transfer_conflicts as check_core_transfer_conflicts,
    create_file_with_contents, scan_trash_with_cancellation, ScanOptions, TransferConflictCheck,
    TransferConflictItem, TrashScan,
};
use file_operation_store::{StoreError, TaskQueueStore};
use iced::Task;

use tokio_util::sync::CancellationToken;

use crate::config;
use crate::model::{
    AddressSuggestionRequest, BrowserPaneId, LoadedOperationStore, Message, PendingOperation,
    SidebarLocation, StartupEnvironment, StartupSessionPlanRequest, TransferConflictMode,
};
use crate::operation_queue::QueuedTransfer;
use crate::sidebar::{home_sidebar_location, sidebar_locations};
use crate::startup_rendering::{StartupRenderingEnvironment, StartupRenderingEnvironmentStatus};
use crate::startup_trace;
use crate::thumbnail_cache::{
    ThumbnailLoadOutcome, ThumbnailLoadPolicy, ThumbnailLoadResult, ThumbnailWork,
};

mod archive_creation;
pub(crate) use archive_creation::check_archive_target_command;
mod application_logs;
pub(crate) use application_logs::application_logs_command;
mod application_shutdown;
pub(crate) use application_shutdown::commit_application_shutdown_command;
mod archive_extraction;
pub(crate) use archive_extraction::inspect_archive_extraction_command;
mod batch_rename_operation;
mod bounded_child_output;
mod browser_session;
mod config_persistence;
mod convert_operation;
pub(crate) use config_persistence::{save_app_config_command, save_user_preferences_command};
mod custom_color_scheme;
pub(crate) use custom_color_scheme::custom_color_scheme_import_command;
mod directory_loading;
pub(crate) use directory_loading::{load_directory_command, load_expanded_directory_command};
mod directory_metadata;
pub(crate) use directory_metadata::load_directory_metadata_command;
mod desktop_notifications;
pub(crate) use desktop_notifications::publish_desktop_notification_command;
mod document_preview;
pub(crate) use document_preview::{prepare_document_command, render_document_page_command};
mod list_directory_summary;
pub(crate) use list_directory_summary::load_list_directory_summary_command;
mod network_connections;
pub(crate) use network_connections::{
    network_connection_credentials_clear_command, network_connection_credentials_lookup_command,
    network_connection_credentials_store_command, network_connection_mount_command,
    network_connection_unmount_command, network_mount_states_command,
};
mod preview;
pub(crate) use preview::{
    animated_image_preview_command, image_preview_dimensions_command,
    original_image_preview_command, preview_command, preview_directory_children_command,
    right_preview_panel_info_command,
    remote_preview_cache_command, start_audio_preview_command, start_video_preview_audio_command,
    text_preview_chunk_command, video_preview_frame_command, video_preview_metadata_command,
};
mod properties;
pub(crate) use properties::{
    apply_file_properties_permissions_to_enclosed_items_command, file_properties_command,
    set_file_properties_permissions_command, FilePropertiesPermissionTargets,
};
mod queued_file_operations;
pub(crate) use queued_file_operations::{file_operation_subscription, DurableDirectMoveCommit};
mod search;
pub(crate) use search::{
    directory_fallback_search_command, search_command, search_with_scope_root_check_command,
};
mod search_service;
mod search_service_endpoint;
mod search_service_recovery;
mod search_service_systemd;
pub(crate) use search_service::{
    configure_search_paths_command, ensure_search_service_command, read_search_path_configuration,
    search_path_configuration_command, search_path_directory_chooser_command,
    search_service_status_command,
};
pub(crate) use search_service_recovery::search_service_recovery_command;
mod sidebar_devices;
pub(crate) use sidebar_devices::{sidebar_device_action_command, sidebar_devices_command};
mod startup_paths;
#[cfg(test)]
pub(crate) use startup_paths::classify_startup_session;
pub(crate) use startup_paths::{
    startup_directory_validation_command, startup_session_plan_command,
};
mod wayland_dnd;
pub(crate) use wayland_dnd::wayland_dnd_window_handle_command;
mod x11_dnd;
pub(crate) use x11_dnd::x11_dnd_window_handle_command;

const PATH_SUGGESTION_LIMIT: usize = 6;
const THUMBNAIL_REFRESH_DELAY: Duration = Duration::from_millis(400);

pub(crate) fn startup_environment_command(
    startup_rendering_environment: StartupRenderingEnvironment,
) -> Task<Message> {
    Task::perform(
        load_startup_environment(startup_rendering_environment),
        |startup_environment| Message::StartupEnvironmentLoaded(Box::new(startup_environment)),
    )
}

pub(crate) fn sidebar_locations_command(
    home: PathBuf,
    configured_favorites: Option<Vec<config::SidebarFavoriteConfig>>,
) -> Task<Message> {
    Task::perform(
        load_sidebar_locations(home, configured_favorites),
        Message::SidebarLocationsLoaded,
    )
}

pub(crate) fn operation_store_command(
    path: PathBuf,
    startup_session_request: Option<StartupSessionPlanRequest>,
) -> Task<Message> {
    Task::perform(
        load_operation_store(path, startup_session_request),
        Message::OperationStoreLoaded,
    )
}

pub(crate) fn delayed_thumbnail_refresh_command(
    pane_id: BrowserPaneId,
    directory: PathBuf,
) -> Task<Message> {
    Task::perform(delayed_thumbnail_refresh(directory), move |directory| {
        Message::ThumbnailRefreshRequested(pane_id, directory)
    })
}

pub(crate) fn save_column_width_overrides_command(
    task_queue_store: TaskQueueStore,
    column_width_overrides: HashMap<usize, f32>,
) -> Task<Message> {
    Task::perform(
        persist_column_width_overrides(task_queue_store, column_width_overrides),
        Message::ColumnWidthOverrideSaved,
    )
}

pub(crate) fn save_browser_session_command(
    task_queue_store: TaskQueueStore,
    snapshot: crate::model::BrowserSessionSnapshot,
) -> Task<Message> {
    Task::perform(
        browser_session::persist_browser_session(task_queue_store, snapshot),
        Message::BrowserSessionSaved,
    )
}

pub(crate) fn load_trash_command(
    generation: u64,
    options: ScanOptions,
    cancellation: CancellationToken,
) -> Task<Message> {
    Task::perform(load_trash(options, cancellation), move |scan| {
        Message::TrashLoaded(generation, scan)
    })
}

pub(crate) fn path_suggestions_command(request: AddressSuggestionRequest) -> Task<Message> {
    let issued_request = request.clone();
    Task::perform(
        load_path_suggestions(request.draft, request.current_dir),
        move |suggestions| Message::AddressSuggestionsLoaded(issued_request.clone(), suggestions),
    )
}

pub(crate) fn thumbnail_batch_command(
    cache_dir: PathBuf,
    works: Vec<ThumbnailWork>,
) -> Task<Message> {
    Task::perform(
        load_thumbnail_batch(cache_dir, works),
        Message::ThumbnailBatchLoaded,
    )
}

pub(crate) fn open_file_command(
    path: PathBuf,
    terminal_emulator: TerminalEmulator,
) -> Task<Message> {
    let opened_path = path.clone();
    Task::perform(
        async move {
            open_path_with_terminal_emulator(path, terminal_emulator)
                .await
                .map_err(|error| error.to_string())
        },
        move |result| Message::OpenFileFinished(opened_path.clone(), result),
    )
}

pub(crate) fn open_with_applications_command(path: PathBuf) -> Task<Message> {
    let requested_path = path.clone();
    Task::perform(
        async move {
            open_with_applications(path)
                .await
                .map_err(|error| error.to_string())
        },
        move |applications| {
            Message::OpenWithApplicationsLoaded(requested_path.clone(), applications)
        },
    )
}

pub(crate) fn open_with_application_command(
    path: PathBuf,
    desktop_id: String,
    launch_mode: OpenWithLaunchMode,
) -> Task<Message> {
    Task::perform(
        async move {
            open_path_with_application(path, desktop_id, launch_mode)
                .await
                .map_err(|error| error.to_string())
        },
        Message::OpenWithApplicationFinished,
    )
}

pub(crate) fn open_terminal_command(
    directory: PathBuf,
    terminal_emulator: TerminalEmulator,
) -> Task<Message> {
    Task::perform(
        async move {
            open_terminal_at_directory(directory, terminal_emulator)
                .await
                .map_err(|error| error.to_string())
        },
        Message::OpenTerminalFinished,
    )
}

pub(crate) fn write_file_clipboard_command(selection: FileClipboardSelection) -> Task<Message> {
    Task::perform(
        async move {
            write_file_clipboard(selection)
                .await
                .map_err(|error| error.to_string())
        },
        Message::FileClipboardWriteFinished,
    )
}

pub(crate) fn read_desktop_clipboard_command(
    paste_directory: PathBuf,
    fallback_operation: Option<PendingOperation>,
) -> Task<Message> {
    let issued_directory = paste_directory.clone();
    let issued_fallback = fallback_operation.clone();
    Task::perform(
        async move {
            read_desktop_clipboard()
                .await
                .map_err(|error| error.to_string())
        },
        move |content| Message::DesktopClipboardReadFinished {
            paste_directory: issued_directory.clone(),
            fallback_operation: issued_fallback.clone(),
            content,
        },
    )
}

pub(crate) fn create_clipboard_file_command(path: PathBuf, contents: Vec<u8>) -> Task<Message> {
    Task::perform(
        create_clipboard_file_at_available_path(path, contents),
        Message::ClipboardFileCreated,
    )
}

pub(crate) fn check_transfer_conflicts_command(
    mode: TransferConflictMode,
    transfers: Vec<QueuedTransfer>,
) -> Task<Message> {
    let issued_transfers = transfers.clone();
    Task::perform(
        async move { check_transfer_conflicts(&transfers).await },
        move |conflicts| {
            Message::TransferConflictsChecked {
                mode,
                transfers: issued_transfers.clone(),
                conflicts,
            }
        },
    )
}

pub(crate) fn expand_transfer_conflict_merges_command(
    mode: TransferConflictMode,
    merge_pairs: Vec<(PathBuf, PathBuf)>,
    remaining_transfers: Vec<QueuedTransfer>,
    remaining_conflicts: Vec<TransferConflictItem>,
) -> Task<Message> {
    Task::perform(
        expand_transfer_conflict_merges(merge_pairs, remaining_transfers, remaining_conflicts),
        move |expansion| {
            Message::TransferConflictMergesExpanded {
                mode,
                expansion,
            }
        },
    )
}

/// 把每个合并对(source 目录 → target 目录)展开成「子项 → target/子项」传输,
/// 只对新子项做冲突检查:剩余传输的冲突状态已在对话框里逐项敲定,不能重查。
async fn expand_transfer_conflict_merges(
    merge_pairs: Vec<(PathBuf, PathBuf)>,
    mut transfers: Vec<QueuedTransfer>,
    mut conflicts: Vec<TransferConflictItem>,
) -> Result<(Vec<QueuedTransfer>, Vec<TransferConflictItem>), String> {
    let mut reserved_targets: std::collections::HashSet<PathBuf> = transfers
        .iter()
        .map(|transfer| transfer.target.clone())
        .collect();
    let mut child_transfers = Vec::new();
    for (source_directory, target_directory) in merge_pairs {
        let mut entries = tokio::fs::read_dir(&source_directory)
            .await
            .map_err(|error| error.to_string())?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|error| error.to_string())?
        {
            let child_source = entry.path();
            let Some(name) = child_source.file_name() else {
                continue;
            };
            let candidate = target_directory.join(name);
            let child_target = if reserved_targets.contains(&candidate) {
                unique_reserved_target(&candidate, &reserved_targets)
            } else {
                candidate
            };
            reserved_targets.insert(child_target.clone());
            child_transfers.push(QueuedTransfer::new(child_source, child_target));
        }
    }

    let new_conflicts = check_transfer_conflicts(&child_transfers).await;
    transfers.extend(child_transfers);
    conflicts.extend(new_conflicts);
    Ok((transfers, conflicts))
}

/// 批内目标撞名时的后备名;与磁盘是否占用无关,那由冲突检查统一裁决。
fn unique_reserved_target(target: &Path, reserved: &std::collections::HashSet<PathBuf>) -> PathBuf {
    let parent = target.parent().map(Path::to_path_buf).unwrap_or_default();
    let name = target
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_else(|| std::ffi::OsString::from("item"));
    for index in 1..1000u32 {
        let mut next = name.clone();
        next.push(format!(".copy{index}"));
        let candidate = parent.join(next);
        if !reserved.contains(&candidate) {
            return candidate;
        }
    }
    target.to_path_buf()
}

async fn create_clipboard_file_at_available_path(
    path: PathBuf,
    contents: Vec<u8>,
) -> Result<PathBuf, String> {
    let target = available_transfer_target_path(path)
        .await
        .map_err(|error| error.to_string())?;
    create_file_with_contents(target, contents)
        .await
        .map_err(|error| error.to_string())
}

async fn check_transfer_conflicts(transfers: &[QueuedTransfer]) -> Vec<TransferConflictItem> {
    let conflict_checks = transfers
        .iter()
        .map(|transfer| {
            TransferConflictCheck::new(transfer.source.clone(), transfer.target.clone())
        })
        .collect();
    check_core_transfer_conflicts(conflict_checks).await
}

async fn load_trash(
    options: ScanOptions,
    cancellation: CancellationToken,
) -> Result<TrashScan, String> {
    scan_trash_with_cancellation(options, cancellation)
        .await
        .map_err(|error| error.to_string())
}

async fn load_startup_environment(
    startup_rendering_environment: StartupRenderingEnvironment,
) -> StartupEnvironment {
    startup_trace::mark_once("startup_environment_started");
    let fallback_rendering_environment = startup_rendering_environment.clone();
    tokio::task::spawn_blocking(move || {
        let app_config = config::load_app_config();
        startup_trace::mark_once("startup_app_config_loaded");
        let user_config = config::load_user_config_for_app_config(app_config);
        startup_trace::mark_once("startup_user_config_loaded");
        let rendering_environment_status =
            StartupRenderingEnvironmentStatus::for_loaded_config_with_runtime(
                user_config.rendering_gpu_preference,
                &startup_rendering_environment,
            );
        StartupEnvironment {
            home: dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")),
            system_language: crate::localization::detect_system_language(),
            user_config,
            state_database_path: config::default_state_database_path(),
            rendering_environment_status,
        }
    })
    .await
    .unwrap_or_else(|_| StartupEnvironment {
        home: PathBuf::from("/"),
        system_language: config::UiLanguage::English,
        user_config: config::ui_thread_startup_config(),
        state_database_path: PathBuf::new(),
        rendering_environment_status: StartupRenderingEnvironmentStatus::ready(
            fallback_rendering_environment,
        ),
    })
}

async fn delayed_thumbnail_refresh(directory: PathBuf) -> PathBuf {
    tokio::time::sleep(THUMBNAIL_REFRESH_DELAY).await;
    directory
}

async fn load_operation_store(
    path: PathBuf,
    startup_session_request: Option<StartupSessionPlanRequest>,
) -> Result<LoadedOperationStore, String> {
    if path.as_os_str().is_empty() {
        return Err("state database path is unavailable".to_owned());
    }

    let store_outcome = tokio::task::spawn_blocking(move || {
        let store = TaskQueueStore::new(path)?;
        let browser_session = match store.read_browser_session() {
            Ok(session) => session.and_then(crate::model::snapshot_from_stored),
            Err(StoreError::Json(_)) => None,
            Err(error) => return Err(error),
        };
        let column_width_overrides = store
            .read_column_widths()?
            .into_iter()
            .map(|(column_index, width)| {
                (column_index, config::normalize_column_width(width as f32))
            })
            .collect();
        let classified_startup_session = startup_session_request
            .map(|request| startup_paths::classify_startup_session(request, browser_session));
        Ok::<LoadedOperationStore, file_operation_store::StoreError>(LoadedOperationStore {
            task_queue_store: store,
            column_width_overrides,
            classified_startup_session,
        })
    })
    .await
    .map_err(|error| error.to_string())?;
    store_outcome.map_err(|error| error.to_string())
}

async fn persist_column_width_overrides(
    task_queue_store: TaskQueueStore,
    column_width_overrides: HashMap<usize, f32>,
) -> Result<(), String> {
    let stored_widths = column_width_overrides
        .into_iter()
        .map(|(column_index, width)| {
            (
                column_index,
                f64::from(config::normalize_column_width(width)),
            )
        })
        .collect();
    tokio::task::spawn_blocking(move || task_queue_store.replace_column_widths(stored_widths))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

async fn load_thumbnail_batch(
    cache_dir: PathBuf,
    works: Vec<ThumbnailWork>,
) -> Vec<ThumbnailLoadOutcome> {
    let mut handles = Vec::with_capacity(works.len());
    for work in works {
        let task_cache_dir = cache_dir.clone();
        let fallback_work = work.clone();
        let handle = tokio::spawn(async move {
            let outcome = match work.load_policy {
                ThumbnailLoadPolicy::LoadOrGenerate => {
                    match thumbnails::load_or_generate_thumbnail(
                        task_cache_dir,
                        work.request.clone(),
                    )
                    .await
                    {
                        Ok(thumbnail) => ThumbnailLoadResult::Ready(thumbnail),
                        Err(error) => ThumbnailLoadResult::Failed(error.to_string()),
                    }
                }
                ThumbnailLoadPolicy::CacheOnly => {
                    match thumbnails::load_cached_thumbnail(task_cache_dir, work.request.clone())
                        .await
                    {
                        Ok(Some(thumbnail)) => ThumbnailLoadResult::Ready(thumbnail),
                        Ok(None) => ThumbnailLoadResult::CacheMiss,
                        Err(error) => ThumbnailLoadResult::Failed(error.to_string()),
                    }
                }
            };
            ThumbnailLoadOutcome {
                work,
                result: outcome,
            }
        });
        handles.push((fallback_work, handle));
    }

    let mut outcomes = Vec::with_capacity(handles.len());
    for (fallback_work, handle) in handles {
        match handle.await {
            Ok(outcome) => outcomes.push(outcome),
            Err(error) => outcomes.push(ThumbnailLoadOutcome {
                work: fallback_work,
                result: ThumbnailLoadResult::Failed(format!("thumbnail task failed: {error}")),
            }),
        }
    }
    outcomes
}

async fn load_sidebar_locations(
    home: PathBuf,
    configured_favorites: Option<Vec<config::SidebarFavoriteConfig>>,
) -> Vec<SidebarLocation> {
    let fallback_home = home.clone();
    let locations = tokio::task::spawn_blocking(move || {
        sidebar_locations(&home, configured_favorites.as_deref())
    })
    .await
    .unwrap_or_else(|_| vec![home_sidebar_location(&fallback_home)]);
    startup_trace::mark_once("sidebar_locations_loaded");
    locations
}

async fn load_path_suggestions(input: String, current_dir: PathBuf) -> Vec<PathBuf> {
    let Some((directory, prefix)) = suggestion_directory_and_prefix(&input, &current_dir) else {
        return Vec::new();
    };
    let mut reader = match tokio::fs::read_dir(directory).await {
        Ok(reader) => reader,
        Err(_) => return Vec::new(),
    };

    let mut suggestions = Vec::new();
    while let Ok(Some(dir_entry)) = reader.next_entry().await {
        let file_name = dir_entry.file_name();
        if !file_name_starts_with(&file_name, &prefix) {
            continue;
        }

        let Ok(file_type) = dir_entry.file_type().await else {
            continue;
        };
        if file_type.is_dir() {
            suggestions.push(dir_entry.path());
        }
    }

    suggestions.sort_unstable();
    suggestions.truncate(PATH_SUGGESTION_LIMIT);
    suggestions
}

#[cfg(unix)]
fn file_name_starts_with(file_name: &std::ffi::OsStr, prefix: &str) -> bool {
    file_name.as_bytes().starts_with(prefix.as_bytes())
}

#[cfg(not(unix))]
fn file_name_starts_with(file_name: &std::ffi::OsStr, prefix: &str) -> bool {
    file_name.to_string_lossy().starts_with(prefix)
}

fn suggestion_directory_and_prefix(input: &str, current_dir: &Path) -> Option<(PathBuf, String)> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    let raw_path = PathBuf::from(trimmed);
    let path = if raw_path.is_absolute() {
        raw_path
    } else {
        current_dir.join(raw_path)
    };

    if trimmed.ends_with(std::path::MAIN_SEPARATOR) {
        return Some((path, String::new()));
    }

    let prefix = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let directory = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| current_dir.to_path_buf());

    Some((directory, prefix))
}

#[cfg(test)]
mod merge_expansion_tests {
    use super::*;

    /// read_dir 顺序不稳定,按集合比较子项传输。
    fn contains_transfer(transfers: &[QueuedTransfer], source: &Path, target: &Path) -> bool {
        transfers
            .iter()
            .any(|transfer| transfer.source == source && transfer.target == target)
    }

    #[tokio::test]
    async fn merge_expansion_transfers_children_without_new_conflicts() {
        let workspace = tempfile::tempdir().unwrap();
        let source = workspace.path().join("source");
        let target = workspace.path().join("target");
        std::fs::create_dir_all(source.join("nested")).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(source.join("report.txt"), b"report").unwrap();
        std::fs::write(source.join("nested").join("child.txt"), b"child").unwrap();

        let (transfers, conflicts) =
            expand_transfer_conflict_merges(vec![(source.clone(), target.clone())], Vec::new(), Vec::new())
                .await
                .unwrap();

        // 目标目录为空:子项全部照原名落位,不产生新冲突。
        assert!(conflicts.is_empty());
        assert_eq!(transfers.len(), 2);
        assert!(contains_transfer(
            &transfers,
            &source.join("report.txt"),
            &target.join("report.txt")
        ));
        assert!(contains_transfer(
            &transfers,
            &source.join("nested"),
            &target.join("nested")
        ));
    }

    #[tokio::test]
    async fn merge_expansion_reflags_same_named_children_as_new_conflicts() {
        let workspace = tempfile::tempdir().unwrap();
        let source = workspace.path().join("source");
        let target = workspace.path().join("target");
        std::fs::create_dir_all(source.join("nested")).unwrap();
        std::fs::create_dir_all(target.join("nested")).unwrap();
        std::fs::write(source.join("nested").join("inner.txt"), b"inner").unwrap();
        std::fs::write(source.join("dup.txt"), b"source content").unwrap();
        std::fs::write(target.join("dup.txt"), b"target content").unwrap();

        let (transfers, conflicts) =
            expand_transfer_conflict_merges(vec![(source.clone(), target.clone())], Vec::new(), Vec::new())
                .await
                .unwrap();

        assert_eq!(transfers.len(), 2);
        // 嵌套同名目录再次进冲突清单且仍可合并,逐层推进;
        // 同名文件不可合并,留在对话框里按替换/保留两者裁决。
        let nested_conflict = conflicts
            .iter()
            .find(|conflict| conflict.source == source.join("nested"))
            .unwrap();
        assert_eq!(nested_conflict.target, target.join("nested"));
        assert!(nested_conflict.can_merge());
        let file_conflict = conflicts
            .iter()
            .find(|conflict| conflict.source == source.join("dup.txt"))
            .unwrap();
        assert!(!file_conflict.can_merge());
        assert_eq!(
            file_conflict.source_metadata.len,
            "source content".len() as u64
        );
    }
}
