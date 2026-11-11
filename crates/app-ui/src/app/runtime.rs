use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use desktop_linux::{
    DesktopActivationRuntime, WaylandDndController, WaylandDndEvent, WaylandDndWindowHandle,
    X11DndController, X11DndEvent, X11DndWindowHandle,
};
use file_core::watch_directory;
use iced::futures::SinkExt;
use iced::{Subscription, Task};

use super::FileBrowser;
use crate::command_line::ApplicationLaunchRequest;
use crate::model::{Message, X11DndMessage};
use crate::startup_rendering::StartupRenderingEnvironment;
use crate::startup_trace;
use bennu_theme::{read_matugen_colors_file, AppearanceMode};

const DIRECTORY_WATCH_DEBOUNCE: Duration = Duration::from_millis(250);
const DIRECTORY_WATCH_CHANNEL_SIZE: usize = 8;
const SIDEBAR_DEVICE_REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const SCROLLBAR_AUTO_HIDE_DURATION: Duration = Duration::from_millis(650);

pub(crate) fn run(
    application_launch_request: ApplicationLaunchRequest,
    file_manager_activation: Option<Arc<DesktopActivationRuntime>>,
    initial_desktop_activation: Option<desktop_linux::DesktopActivationEvent>,
    startup_rendering_environment: StartupRenderingEnvironment,
) -> iced::Result {
    startup_trace::mark("iced_run_entered");
    bennu_theme::typography::adopt_system_font_families();
    startup_trace::mark("system_fonts_adopted");
    iced::daemon(
        move || {
            FileBrowser::boot(
                application_launch_request.clone(),
                file_manager_activation.clone(),
                initial_desktop_activation.clone(),
                startup_rendering_environment.clone(),
            )
        },
        FileBrowser::update,
        FileBrowser::view,
    )
    .subscription(FileBrowser::subscription)
    .theme(FileBrowser::theme)
    .title(FileBrowser::title)
    .settings(iced::Settings {
        default_text_size: bennu_theme::typography::BODY_SIZE.into(),
        ..iced::Settings::default()
    })
    .run()
}

pub(super) fn desktop_activation_subscription(
    runtime: Arc<DesktopActivationRuntime>,
) -> Subscription<Message> {
    Subscription::run_with(
        DesktopActivationSubscriptionState { runtime },
        desktop_activation_stream,
    )
}

#[derive(Debug, Clone)]
struct DesktopActivationSubscriptionState {
    runtime: Arc<DesktopActivationRuntime>,
}

impl PartialEq for DesktopActivationSubscriptionState {
    fn eq(&self, other: &Self) -> bool {
        self.runtime.identity() == other.runtime.identity()
    }
}

impl Eq for DesktopActivationSubscriptionState {}

impl std::hash::Hash for DesktopActivationSubscriptionState {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.runtime.identity().hash(state);
    }
}

fn desktop_activation_stream(
    state: &DesktopActivationSubscriptionState,
) -> impl iced::futures::Stream<Item = Message> + 'static {
    let runtime = Arc::clone(&state.runtime);
    iced::stream::channel(16, async move |mut output| {
        let Some(mut receiver) = runtime.take_event_receiver() else {
            let _ = output
                .send(Message::DesktopActivationRuntimeFailed(
                    "desktop activation receiver was already taken".to_owned(),
                ))
                .await;
            return;
        };
        while let Some(event) = receiver.recv().await {
            if output
                .send(Message::DesktopActivationReceived(event))
                .await
                .is_err()
            {
                return;
            }
        }
        let _ = output
            .send(Message::DesktopActivationRuntimeFailed(
                "desktop activation channel closed".to_owned(),
            ))
            .await;
    })
}

pub(super) fn directory_watch_subscription(path: PathBuf) -> Subscription<Message> {
    Subscription::run_with(path, directory_watch_stream)
}

pub(super) fn matugen_colors_subscription(path: PathBuf) -> Subscription<Message> {
    Subscription::run_with(path, matugen_colors_stream)
}

/// 系统深浅色：启动时报一次，之后系统每切换一次报一次（“自动”模式跟着变）。
pub(super) fn system_mode_subscription() -> Subscription<Message> {
    Subscription::run(system_mode_stream)
}

fn system_mode_stream() -> impl iced::futures::Stream<Item = Message> {
    use iced::futures::StreamExt;
    desktop_linux::portal_prefers_dark_changes()
        .map(|prefers_dark| Message::SystemModeChanged(system_mode(prefers_dark)))
        // 连不上 portal 时上面只出一项就结束；流结束会被 iced 重新拉起，反复连 portal。
        .chain(iced::futures::stream::pending())
}

fn system_mode(prefers_dark: bool) -> AppearanceMode {
    if prefers_dark {
        AppearanceMode::Dark
    } else {
        AppearanceMode::Light
    }
}

pub(super) fn sidebar_device_refresh_subscription() -> Subscription<Message> {
    iced::time::every(SIDEBAR_DEVICE_REFRESH_INTERVAL)
        .map(|_| Message::SidebarDevicesRefreshRequested)
}

pub(super) fn wayland_file_dnd_subscription(
    window_handle: WaylandDndWindowHandle,
    controller: Arc<WaylandDndController>,
) -> Subscription<Message> {
    Subscription::run_with(
        WaylandDndSubscriptionState {
            window_handle,
            controller,
        },
        wayland_file_dnd_stream,
    )
}

pub(super) fn x11_file_dnd_subscription(
    window_handle: X11DndWindowHandle,
    controller: Arc<X11DndController>,
) -> Subscription<Message> {
    Subscription::run_with(
        X11DndSubscriptionState {
            window_handle,
            controller,
        },
        x11_file_dnd_stream,
    )
}

fn directory_watch_stream(path: &PathBuf) -> impl iced::futures::Stream<Item = Message> + 'static {
    let path = path.clone();
    iced::stream::channel(DIRECTORY_WATCH_CHANNEL_SIZE, async move |mut output| {
        match watch_directory(&path, DIRECTORY_WATCH_DEBOUNCE) {
            Ok(mut watcher) => {
                while let Some(change) = watcher.recv().await {
                    if output
                        .send(Message::ObservedDirectoryChanges(change))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
            // 连轮询都起不来说明目录本身读不到；目录加载会报出真实原因，这里只留记录。
            Err(error) => tracing::warn!(
                target: "app_ui::directory_watch",
                event = "directory_watch_unavailable",
                path = %path.display(),
                %error,
                "directory changes will not be followed"
            ),
        }

        iced::futures::future::pending().await
    })
}

fn matugen_colors_stream(path: &PathBuf) -> impl iced::futures::Stream<Item = Message> + 'static {
    let path = path.clone();
    let directory = path
        .parent()
        .expect("matugen theme path has a parent directory")
        .to_path_buf();
    iced::stream::channel(DIRECTORY_WATCH_CHANNEL_SIZE, async move |mut output| {
        if let Err(error) = tokio::fs::create_dir_all(&directory).await {
            let _ = output
                .send(Message::MatugenColorsUpdated(Err(format!(
                    "could not create matugen theme directory {}: {error}",
                    directory.display()
                ))))
                .await;
            iced::futures::future::pending().await
        }

        let watcher = watch_directory(directory, DIRECTORY_WATCH_DEBOUNCE);

        if output
            .send(Message::MatugenColorsUpdated(
                read_matugen_colors_file(&path).await,
            ))
            .await
            .is_err()
        {
            return;
        }

        let mut watcher = match watcher {
            Ok(watcher) => watcher,
            Err(error) => {
                let _ = output
                    .send(Message::MatugenColorsUpdated(Err(format!(
                        "could not watch matugen theme directory: {error}"
                    ))))
                    .await;
                iced::futures::future::pending().await
            }
        };

        while watcher.recv().await.is_some() {
            if output
                .send(Message::MatugenColorsUpdated(
                    read_matugen_colors_file(&path).await,
                ))
                .await
                .is_err()
            {
                return;
            }
        }

        iced::futures::future::pending().await
    })
}

#[derive(Debug, Clone)]
struct WaylandDndSubscriptionState {
    window_handle: WaylandDndWindowHandle,
    controller: Arc<WaylandDndController>,
}

impl PartialEq for WaylandDndSubscriptionState {
    fn eq(&self, other: &Self) -> bool {
        self.window_handle == other.window_handle && self.controller.id() == other.controller.id()
    }
}

impl Eq for WaylandDndSubscriptionState {}

impl std::hash::Hash for WaylandDndSubscriptionState {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.window_handle.hash(state);
        self.controller.id().hash(state);
    }
}

fn wayland_file_dnd_stream(
    state: &WaylandDndSubscriptionState,
) -> impl iced::futures::Stream<Item = Message> + 'static {
    let state = state.clone();
    iced::stream::channel(16, async move |mut output| {
        let (event_sender, mut event_receiver) = tokio::sync::mpsc::unbounded_channel();
        let (shutdown_sender, shutdown_receiver) = mpsc::channel();
        let _shutdown_guard = FileDndShutdown::new(shutdown_sender);

        if let Err(error) = desktop_linux::spawn_wayland_file_dnd(
            state.window_handle,
            state.controller,
            event_sender,
            shutdown_receiver,
        ) {
            let _ = output
                .send(Message::WaylandDndRuntimeFailed(error.to_string()))
                .await;
            return;
        }

        while let Some(event) = event_receiver.recv().await {
            let message = match event {
                WaylandDndEvent::FilesDropped(drop) => Message::WaylandFilesDropped(drop),
                WaylandDndEvent::FileDropFailed {
                    target_session_id,
                    details,
                } => Message::WaylandFileDropFailed(target_session_id, details),
                WaylandDndEvent::FileDragSource(event) => {
                    Message::WaylandFileDragSourceEvent(event)
                }
                WaylandDndEvent::FileDropTarget(event) => {
                    Message::WaylandFileDropTargetEvent(event)
                }
                WaylandDndEvent::RuntimeFailed(error) => Message::WaylandDndRuntimeFailed(error),
            };
            if output.send(message).await.is_err() {
                break;
            }
        }
    })
}

#[derive(Debug, Clone)]
struct X11DndSubscriptionState {
    window_handle: X11DndWindowHandle,
    controller: Arc<X11DndController>,
}

impl PartialEq for X11DndSubscriptionState {
    fn eq(&self, other: &Self) -> bool {
        self.window_handle == other.window_handle && self.controller.id() == other.controller.id()
    }
}

impl Eq for X11DndSubscriptionState {}

impl std::hash::Hash for X11DndSubscriptionState {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.window_handle.hash(state);
        self.controller.id().hash(state);
    }
}

fn x11_file_dnd_stream(
    state: &X11DndSubscriptionState,
) -> impl iced::futures::Stream<Item = Message> + 'static {
    let state = state.clone();
    iced::stream::channel(16, async move |mut output| {
        let runtime_id = state.controller.id();
        let (event_sender, mut event_receiver) = tokio::sync::mpsc::unbounded_channel();
        let (shutdown_sender, shutdown_receiver) = mpsc::channel();
        let _shutdown_guard = FileDndShutdown::new(shutdown_sender);

        if let Err(error) = desktop_linux::spawn_x11_file_dnd(
            state.window_handle,
            state.controller,
            event_sender,
            shutdown_receiver,
        ) {
            let _ = output
                .send(Message::X11Dnd(X11DndMessage::RuntimeEvent {
                    runtime_id,
                    event: X11DndEvent::RuntimeFailed(error.to_string()),
                }))
                .await;
            return;
        }

        while let Some(event) = event_receiver.recv().await {
            if output
                .send(Message::X11Dnd(X11DndMessage::RuntimeEvent {
                    runtime_id,
                    event,
                }))
                .await
                .is_err()
            {
                break;
            }
        }
    })
}

struct FileDndShutdown {
    sender: Option<mpsc::Sender<()>>,
}

impl FileDndShutdown {
    fn new(sender: mpsc::Sender<()>) -> Self {
        Self {
            sender: Some(sender),
        }
    }
}

impl Drop for FileDndShutdown {
    fn drop(&mut self) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(());
        }
    }
}

pub(super) fn scrollbar_auto_hide_command(generation: u64) -> Task<Message> {
    Task::perform(
        async move {
            tokio::time::sleep(SCROLLBAR_AUTO_HIDE_DURATION).await;
            generation
        },
        Message::ScrollbarAutoHideElapsed,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use bennu_theme::MatugenColors;
    use iced::futures::StreamExt;
    use std::fs;
    use tokio::time::timeout;

    const GENERATED: &str = include_str!("../../../bennu-theme/test-data/matugen.toml");

    async fn next_matugen_update(
        stream: &mut std::pin::Pin<Box<impl iced::futures::Stream<Item = Message>>>,
    ) -> Result<Option<MatugenColors>, String> {
        match timeout(Duration::from_secs(3), stream.next())
            .await
            .expect("Matugen watcher timed out")
            .expect("Matugen watcher ended")
        {
            Message::MatugenColorsUpdated(update) => update,
            message => panic!("unexpected watcher message: {message:?}"),
        }
    }

    #[tokio::test]
    async fn matugen_stream_reads_initial_atomic_updates_and_deletion() {
        let directory = tempfile::tempdir().expect("create temporary config directory");
        let path = directory.path().join("matugen.toml");
        fs::write(&path, GENERATED).expect("write initial Matugen colors");

        let mut stream = Box::pin(matugen_colors_stream(&path));
        let initial = next_matugen_update(&mut stream)
            .await
            .expect("initial Matugen colors must parse")
            .expect("initial Matugen colors must exist");

        let replacement = directory.path().join("matugen.toml.next");
        let regenerated = GENERATED.replace("#65558f", "#8f5565");
        fs::write(&replacement, &regenerated).expect("write replacement Matugen colors");
        fs::rename(&replacement, &path).expect("atomically replace Matugen colors");

        let updated = next_matugen_update(&mut stream)
            .await
            .expect("replacement Matugen colors must parse")
            .expect("replacement Matugen colors must exist");
        assert_ne!(updated, initial);

        fs::write(&replacement, "version = 1\nmode = \"dark\"\n")
            .expect("write an old-format Matugen file");
        fs::rename(&replacement, &path).expect("atomically replace with the old format");
        assert!(next_matugen_update(&mut stream).await.is_err());

        fs::remove_file(&path).expect("remove Matugen colors");
        assert!(next_matugen_update(&mut stream)
            .await
            .expect("deletion must be accepted")
            .is_none());
    }
}
