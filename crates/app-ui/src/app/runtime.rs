use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use desktop_linux::{
    DesktopActivationRuntime, WaylandDndController, WaylandDndEvent, WaylandDndWindowHandle,
    X11DndController, X11DndEvent, X11DndWindowHandle,
};
use file_core::{watch_directory, ResolvedEntryChange, ScanOptions};
use file_operations::ForeignTaskWait;
use iced::futures::SinkExt;
use iced::Subscription;

use super::FileBrowser;
use crate::command_line::ApplicationLaunchRequest;
use crate::model::{Message, X11DndMessage};
use crate::startup_rendering::StartupRenderingEnvironment;
use crate::startup_trace;
use bennu_theme::{read_matugen_colors_file, AppearanceMode};

const DIRECTORY_WATCH_DEBOUNCE: Duration = Duration::from_millis(250);
const DIRECTORY_WATCH_CHANNEL_SIZE: usize = 8;
/// 一次提交会连着改库文件和 WAL；合并成一次重新看。
const OPERATION_STORE_WATCH_DEBOUNCE: Duration = Duration::from_secs(1);

pub(crate) fn run(
    application_launch_request: ApplicationLaunchRequest,
    file_manager_activation: Option<Arc<DesktopActivationRuntime>>,
    initial_desktop_activation: Option<desktop_linux::DesktopActivationEvent>,
    startup_rendering_environment: StartupRenderingEnvironment,
) -> iced::Result {
    startup_trace::mark("iced_run_entered");
    // 字体要在第一次排版前定下，等不到完整的设置加载；读不了就跟随系统，出错由那次加载提示。
    let preferences = crate::config::read_user_preferences_without_repair(
        &crate::config::default_state_database_path(),
    )
    .ok();
    bennu_theme::font_families::adopt_font_families(
        preferences
            .as_ref()
            .map_or("", |preferences| &preferences.interface_font),
        preferences
            .as_ref()
            .map_or("", |preferences| &preferences.monospace_font),
    );
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
    .scale_factor(|browser, _window| browser.interface_scale())
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

/// 监视一个显示中的目录。放不放行隐藏条目的变化进订阅身份：看不见的变化在这里就丢掉，
/// 不发消息、不叫醒界面；切换「显示隐藏文件」或目录里列着点名的隐藏条目时订阅随之换新。
/// 放行后哪一个隐藏条目真在列表里，由各标签应用变化时按自己的规则判断。
pub(super) fn directory_watch_subscription(
    path: PathBuf,
    passes_hidden: bool,
) -> Subscription<Message> {
    Subscription::run_with((path, passes_hidden), directory_watch_stream)
}

/// 状态库（含 WAL、回滚日志）有写入时报一次；开始监视时先报一次，补上监视建立之前
/// 别的进程已经写下的任务与设置。
pub(super) fn state_database_subscription(database_path: PathBuf) -> Subscription<Message> {
    Subscription::run_with(database_path, state_database_stream)
}

fn state_database_stream(
    database_path: &PathBuf,
) -> impl iced::futures::Stream<Item = Message> + 'static {
    let database_path = database_path.clone();
    iced::stream::channel(1, async move |mut output| {
        if output.send(Message::StateDatabaseChanged).await.is_err() {
            return;
        }
        let (Some(directory), Some(database_name)) =
            (database_path.parent(), database_path.file_name())
        else {
            return iced::futures::future::pending().await;
        };
        match watch_directory(directory, OPERATION_STORE_WATCH_DEBOUNCE) {
            Ok(mut watcher) => {
                while let Some(observed) = watcher.recv().await {
                    let touches_database = observed
                        .changes
                        .iter()
                        .any(|change| change_touches_database(change, database_name));
                    if touches_database && output.send(Message::StateDatabaseChanged).await.is_err()
                    {
                        break;
                    }
                }
            }
            Err(error) => tracing::warn!(
                target: "app_ui::operation_supervision",
                event = "operation_store_watch_unavailable",
                path = %directory.display(),
                %error,
                "tasks left by other Bennu processes will be adopted at the next start"
            ),
        }
        iced::futures::future::pending().await
    })
}

/// 同目录里还有本进程自己建的任务锁文件，那些变化与别的进程的任务无关。
fn change_touches_database(change: &ResolvedEntryChange, database_name: &OsStr) -> bool {
    let is_database_file = |path: &Path| {
        path.file_name().is_some_and(|name| {
            name == database_name
                || ["-wal", "-journal"].iter().any(|suffix| {
                    let mut journal = database_name.to_owned();
                    journal.push(suffix);
                    name == journal
                })
        })
    };
    match change {
        ResolvedEntryChange::Added(entry) | ResolvedEntryChange::ContentChanged(entry) => {
            is_database_file(entry.path())
        }
        ResolvedEntryChange::Removed { path } => is_database_file(path),
        ResolvedEntryChange::Renamed { from, to } => {
            is_database_file(from) || is_database_file(to.path())
        }
        ResolvedEntryChange::RescanRequired => true,
    }
}

/// 阻塞等一条外来任务的锁被放手。等待放在独立线程里，而不是 tokio 的阻塞线程池：
/// 持锁进程可能一直不退出，阻塞线程池会在运行时关闭时等它，拖住 Bennu 退出。
pub(super) fn foreign_task_wait_subscription(wait: ForeignTaskWait) -> Subscription<Message> {
    Subscription::run_with(wait, foreign_task_wait_stream)
}

fn foreign_task_wait_stream(
    wait: &ForeignTaskWait,
) -> impl iced::futures::Stream<Item = Message> + 'static {
    let wait = wait.clone();
    iced::stream::channel(1, async move |mut output| {
        let (released, release) = tokio::sync::oneshot::channel();
        let waiter = wait.clone();
        let spawned = std::thread::Builder::new()
            .name("foreign-task-wait".to_owned())
            .spawn(move || {
                let _ = released.send(waiter.wait_for_release());
            });
        let outcome = match spawned {
            Ok(_) => release
                .await
                .unwrap_or_else(|_| Err("foreign task wait stopped".to_owned())),
            Err(error) => Err(format!(
                "could not start waiting for a foreign task: {error}"
            )),
        };
        let _ = output
            .send(Message::ForeignTaskReleased(
                wait.stored_task_id,
                wait.generation,
                outcome,
            ))
            .await;
        iced::futures::future::pending().await
    })
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

/// 设备、挂载、网络位置有变化时报一次；没有变化时不醒、不发消息。
pub(super) fn storage_change_subscription() -> Subscription<Message> {
    Subscription::run(storage_change_stream)
}

fn storage_change_stream() -> impl iced::futures::Stream<Item = Message> {
    use iced::futures::StreamExt;
    desktop_linux::storage_changes()
        .map(|()| Message::StorageChanged)
        // 两路信号源都连不上时流会结束；结束会被 iced 重新拉起反复重连，所以挂住。
        .chain(iced::futures::stream::pending())
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

fn directory_watch_stream(
    (path, passes_hidden): &(PathBuf, bool),
) -> impl iced::futures::Stream<Item = Message> + 'static {
    let path = path.clone();
    let listing = ScanOptions {
        include_hidden: *passes_hidden,
        ..ScanOptions::default()
    };
    iced::stream::channel(DIRECTORY_WATCH_CHANNEL_SIZE, async move |mut output| {
        match watch_directory(&path, DIRECTORY_WATCH_DEBOUNCE) {
            Ok(mut watcher) => {
                while let Some(mut change) = watcher.recv().await {
                    change
                        .changes
                        .retain(|change| change.affects_listing(&listing));
                    if change.changes.is_empty() {
                        continue;
                    }
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
        // 句柄活到流结束：订阅撤掉时流被丢弃，句柄随之 drop，拖放线程退出。
        let _runtime = match desktop_linux::spawn_wayland_file_dnd(
            state.window_handle,
            state.controller,
            event_sender,
        ) {
            Ok(runtime) => runtime,
            Err(error) => {
                let _ = output
                    .send(Message::WaylandDndRuntimeFailed(error.to_string()))
                    .await;
                return;
            }
        };

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
        // 同 Wayland：句柄随流 drop，拖放线程退出。
        let _runtime = match desktop_linux::spawn_x11_file_dnd(
            state.window_handle,
            state.controller,
            event_sender,
        ) {
            Ok(runtime) => runtime,
            Err(error) => {
                let _ = output
                    .send(Message::X11Dnd(X11DndMessage::RuntimeEvent {
                        runtime_id,
                        event: X11DndEvent::RuntimeFailed(error.to_string()),
                    }))
                    .await;
                return;
            }
        };

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
