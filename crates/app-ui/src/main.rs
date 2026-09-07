// 当前 finish phase 只收口搜索移除遗留和验证门禁，不在这里为历史样式问题做大面积无关重构。
#![allow(
    clippy::clone_on_copy,
    clippy::cloned_ref_to_slice_refs,
    clippy::collapsible_if,
    clippy::derivable_impls,
    clippy::field_reassign_with_default,
    clippy::filter_map_bool_then,
    clippy::let_and_return,
    clippy::manual_clamp,
    clippy::manual_is_multiple_of,
    clippy::needless_option_as_deref,
    clippy::needless_return,
    clippy::nonminimal_bool,
    clippy::ptr_arg,
    clippy::redundant_pattern_matching,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::unnecessary_map_or,
    clippy::unnecessary_sort_by,
    clippy::unwrap_or_default,
    clippy::useless_conversion
)]

mod anchored_popup;
mod animated_image_preview;
mod animation;
mod app;
mod app_icon;
mod appearance;
mod audio_preview;
mod breadcrumb_drop_target_bounds;
mod column_entry_bounds;
mod command_line;
mod commands;
mod config;
mod directory_summary;
mod document_preview;
mod file_drag_hit_test_bounds;
mod file_drag_hit_test_marker;
mod file_entry_presentation;
mod file_entry_view;
mod floating_surface;
mod formatting;
mod icon_grid_geometry;
mod icon_grid_layout;
mod icon_grid_view;
mod icons;
mod input_blocking_space;
mod list_view;
mod localization;
mod matugen_theme;
mod measured_middle_ellipsized_text;
mod model;
mod network_connections;
mod open_with;
mod operation_history;
mod operation_progress;
mod operation_queue;
mod operation_queue_display;
mod operation_queue_view;
mod original_image_preview;
mod preview;
mod remote_preview_cache;
mod runtime_logging;
mod selection_marquee;
mod shortcuts;
mod sidebar;
mod sidebar_devices;
mod sqlite_preview;
mod startup_probe_cache;
mod startup_rendering;
mod startup_trace;
mod terminal_panel;
mod text_preview;
mod text_preview_loading;
mod text_preview_viewer;
mod three_column_view;
mod thumbnail_cache;
mod translated_surface;
mod typography;
mod ui_pacing;
mod video_preview;
mod view;
mod virtual_range;
mod visible_entries;
mod wayland_drag_icon;

fn main() -> std::process::ExitCode {
    // 限制 glibc 的线程 arena 数量，避免预览关闭后的分配页长期滞留。
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    let _ = unsafe { libc::mallopt(libc::M_ARENA_MAX, 1) };
    let action = match command_line::parse_process_arguments() {
        Ok(action) => action,
        Err(error) => {
            eprintln!("file-manager: {error}\nTry 'file-manager --help' for more information.");
            return std::process::ExitCode::from(2);
        }
    };

    let (application_launch_request, activation_service) = match action {
        command_line::CommandLineAction::Launch(request) => (Some(request), false),
        command_line::CommandLineAction::ActivationService => (None, true),
        command_line::CommandLineAction::RendererProbe => {
            return match startup_rendering::run_vulkan_renderer_probe() {
                Ok(()) => std::process::ExitCode::SUCCESS,
                Err(_) => std::process::ExitCode::FAILURE,
            };
        }
        command_line::CommandLineAction::PrintHelp => {
            print!("{}", command_line::HELP_TEXT);
            return std::process::ExitCode::SUCCESS;
        }
        command_line::CommandLineAction::PrintVersion => {
            print!("{}", command_line::VERSION_TEXT);
            return std::process::ExitCode::SUCCESS;
        }
    };
    startup_trace::begin_launch_from_env();

    let activation_paths = application_launch_request
        .as_ref()
        .map(command_line::ApplicationLaunchRequest::activation_paths)
        .unwrap_or_default();
    startup_trace::mark("desktop_activation_claim_started");
    let desktop_launch_window_policy = match config::stored_launch_window_policy() {
        config::LaunchWindowPolicy::MergeIntoExisting => {
            desktop_linux::DesktopLaunchWindowPolicy::MergeIntoExisting
        }
        config::LaunchWindowPolicy::OpenNewWindow => {
            desktop_linux::DesktopLaunchWindowPolicy::OpenNewWindow
        }
    };
    let activation_claim = desktop_linux::DesktopActivationRuntime::claim_or_forward(
        &activation_paths,
        desktop_launch_window_policy,
    );
    let activation_claim_outcome = match &activation_claim {
        Ok(desktop_linux::FileManagerActivationClaim::Primary(_)) => {
            startup_trace::DesktopActivationClaimOutcome::Primary
        }
        Ok(desktop_linux::FileManagerActivationClaim::Forwarded) => {
            startup_trace::DesktopActivationClaimOutcome::Forwarded
        }
        Ok(desktop_linux::FileManagerActivationClaim::Detached) => {
            startup_trace::DesktopActivationClaimOutcome::Detached
        }
        Err(_) => startup_trace::DesktopActivationClaimOutcome::Failed,
    };
    startup_trace::mark_desktop_activation_claim_finished(activation_claim_outcome);
    let activation_controller = match activation_claim {
        Ok(desktop_linux::FileManagerActivationClaim::Primary(controller)) => Some(controller),
        Ok(desktop_linux::FileManagerActivationClaim::Forwarded) => {
            return std::process::ExitCode::SUCCESS;
        }
        Ok(desktop_linux::FileManagerActivationClaim::Detached) => None,
        Err(error) => {
            eprintln!("file-manager: desktop activation failed: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let (application_launch_request, initial_desktop_activation) =
        match activation_controller.as_deref() {
            Some(controller) if activation_service => {
                let first_event = match controller.wait_for_initial_event() {
                    Ok(event) => event,
                    Err(error) => {
                        eprintln!("file-manager: desktop activation failed: {error}");
                        return std::process::ExitCode::FAILURE;
                    }
                };
                match first_event {
                    desktop_linux::DesktopActivationEvent::FocusMainWindow(_) => (
                        command_line::ApplicationLaunchRequest::ConfiguredStartup,
                        None,
                    ),
                    desktop_linux::DesktopActivationEvent::MergeWorkspace(workspace, _) => (
                        command_line::ApplicationLaunchRequest::ExplicitWorkspace(
                            command_line::ExplicitWorkspace::from_desktop_workspace(workspace),
                        ),
                        None,
                    ),
                    event @ desktop_linux::DesktopActivationEvent::OpenProperties(_, _) => (
                        command_line::ApplicationLaunchRequest::ConfiguredStartup,
                        Some(event),
                    ),
                }
            }
            _ if activation_service => (
                // Detached：带 --activation-service 启动，但主实例已占名且策略要求新窗口。
                command_line::ApplicationLaunchRequest::ConfiguredStartup,
                None,
            ),
            _ => (
                application_launch_request.expect("ordinary launch has a request"),
                None,
            ),
        };
    runtime_logging::init();
    startup_trace::mark_runtime_logging_ready();
    if let Some(controller) = activation_controller.as_deref() {
        match controller.standard_service_status() {
            desktop_linux::StandardFileManagerServiceStatus::Owned => tracing::info!(
                target: "app_ui::desktop_activation",
                standard_name = desktop_linux::FILE_MANAGER1_BUS_NAME,
                "standard file manager D-Bus interface is active"
            ),
            desktop_linux::StandardFileManagerServiceStatus::Occupied(reason) => tracing::warn!(
                target: "app_ui::desktop_activation",
                standard_name = desktop_linux::FILE_MANAGER1_BUS_NAME,
                reason,
                "standard file manager D-Bus name is unavailable; branded activation remains active"
            ),
        }
    }
    tracing::info!(
        target: "app_ui::runtime",
        event = "application_started",
        "File Manager application started"
    );
    let startup_rendering_environment = startup_rendering::apply_fast_startup_environment();
    match app::run(
        application_launch_request,
        activation_controller,
        initial_desktop_activation,
        startup_rendering_environment,
    ) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            // 渲染初始化失败时先尝试 GL 兜底重启(内部删除探针缓存,
            // 重启后重新探针自愈);兜底已用过或重启失败才报错退出。
            let _ = startup_probe_cache::gl_fallback_restart_after_renderer_failure();
            eprintln!("file-manager: application runtime failed: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
