//! LocalSend 窗口：待发列表与窗口生命周期。
//!
//! Bennu 的拖放只在主窗口上收（Wayland、X11 两套底层都是单窗口），所以文件经
//! 侧栏"LocalSend"落点或窗口里的选择器进待发列表，再点设备发送。待发列表、
//! "按 IP 添加"表单与设备勾选随窗口：关窗即清空，进行中的直推不受影响。

use std::path::{Path, PathBuf};

use iced::{window, Size, Task};
use local_send::DeviceInfo;
use tokio_util::sync::CancellationToken;

use super::super::FileBrowser;
use super::TransferMessage;
use crate::directory_summary::{
    read_directory_recursive_total_size, DirectorySummaryError, RecursiveTotalSize,
};
use crate::model::{Message, WINDOW_TOP_BAR_HEIGHT};

const LOCAL_SEND_WINDOW_APP_ID: &str = "bennu-localsend";
const DEFAULT_WIDTH: f32 = 440.0;
const DEFAULT_HEIGHT: f32 = 560.0;
const MIN_WIDTH: f32 = 360.0;
const MIN_HEIGHT: f32 = 420.0;

/// 待发列表里的一项：路径 + 大小（文件夹递归算，后台进行）。
pub(crate) struct StagedItem {
    pub(crate) path: PathBuf,
    pub(crate) size: StagedSize,
    measuring: CancellationToken,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StagedSize {
    Measuring,
    Measured(RecursiveTotalSize),
    Unreadable(String),
}

/// 门户选择器选什么：文件与文件夹是两种选择对话框，不能混选。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChooserTarget {
    Files,
    Folders,
}

impl FileBrowser {
    /// 侧栏入口：打开或聚焦窗口。
    pub(super) fn open_local_send_window(&mut self) -> Task<Message> {
        Task::batch([
            self.prepare_auxiliary_window_open(),
            self.ensure_local_send_window(),
        ])
    }

    fn ensure_local_send_window(&mut self) -> Task<Message> {
        if let Some(window) = self.local_send_window {
            self.focused_window = window;
            return window::gain_focus(window);
        }
        let (window, command) = window::open(local_send_window_settings());
        self.local_send_window = Some(window);
        self.focused_window = window;
        command.discard()
    }

    pub(in crate::app) fn close_local_send_window(&mut self) -> Task<Message> {
        for item in self.local_send_staged.drain(..) {
            item.measuring.cancel();
        }
        // 还在连的结果回来时对不上表单，会被丢掉。
        self.favorite_address_form = None;
        self.checked_devices
            .clear(super::DeviceListOwner::LocalSendWindow);
        let Some(window) = self.local_send_window.take() else {
            return Task::none();
        };
        self.clear_closed_window_focus(window);
        window::close(window)
    }

    /// 进待发列表的唯一入口（侧栏落点、选择器）：压缩包里的虚拟路径发不了，跳过并
    /// 提示一次；已在列表里的不重复加；其余追加并后台测大小，然后打开窗口。
    pub(in crate::app) fn stage_local_send_paths(&mut self, paths: Vec<PathBuf>) -> Task<Message> {
        let (sendable, inside_archive): (Vec<_>, Vec<_>) = paths.into_iter().partition(|path| {
            file_core::archive_path_identity(path) != file_core::ArchivePathIdentity::InsideArchive
        });
        if !inside_archive.is_empty() {
            self.show_global_error(tr!(
                "Files inside an archive cannot be sent directly. Extract them first."
            ));
        }
        let mut measurements = Vec::new();
        for path in sendable {
            if self.local_send_staged.iter().any(|item| item.path == path) {
                continue;
            }
            let measuring = CancellationToken::new();
            measurements.push(measure_staged_size(path.clone(), measuring.clone()));
            self.local_send_staged.push(StagedItem {
                path,
                size: StagedSize::Measuring,
                measuring,
            });
        }
        measurements.push(self.open_local_send_window());
        Task::batch(measurements)
    }

    /// 按路径回填；已被移除的项找不到就丢弃。同一路径移除后又加入时，旧测量的
    /// 结果量的也是这个路径，照样可用。
    pub(super) fn accept_staged_size(&mut self, path: &Path, size: StagedSize) {
        if let Some(item) = self
            .local_send_staged
            .iter_mut()
            .find(|item| item.path == path)
        {
            item.size = size;
        }
    }

    pub(super) fn remove_staged_item(&mut self, path: &Path) {
        self.local_send_staged.retain(|item| {
            let keep = item.path != path;
            if !keep {
                item.measuring.cancel();
            }
            keep
        });
    }

    pub(super) fn clear_staged_items(&mut self) {
        for item in self.local_send_staged.drain(..) {
            item.measuring.cancel();
        }
    }

    /// 待发列表里的路径；发完列表保留，可以接着发给别的设备。
    pub(super) fn staged_paths(&self) -> Vec<PathBuf> {
        self.local_send_staged
            .iter()
            .map(|item| item.path.clone())
            .collect()
    }

    pub(super) fn send_staged_to_device(&mut self, device: DeviceInfo) -> Task<Message> {
        let paths = self.staged_paths();
        self.begin_direct_sends(vec![device], paths)
    }
}

fn local_send_window_settings() -> window::Settings {
    let mut settings = window::Settings {
        size: Size::new(DEFAULT_WIDTH, DEFAULT_HEIGHT + WINDOW_TOP_BAR_HEIGHT),
        min_size: Some(Size::new(MIN_WIDTH, MIN_HEIGHT + WINDOW_TOP_BAR_HEIGHT)),
        decorations: false,
        // 关闭请求交给状态机：关窗要清待发列表、停掉还在测的大小。
        exit_on_close_request: false,
        ..window::Settings::default()
    };
    settings.platform_specific.application_id = LOCAL_SEND_WINDOW_APP_ID.to_owned();
    settings.icon = crate::app_icon::startup_window_icon();
    settings
}

/// 文件取元数据长度；文件夹与列表视图同一规则递归求和，读不全时标"≥"。
fn measure_staged_size(path: PathBuf, measuring: CancellationToken) -> Task<Message> {
    let measured_path = path.clone();
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || staged_size(&measured_path, measuring))
                .await
                .unwrap_or_else(|error| StagedSize::Unreadable(error.to_string()))
        },
        move |size| Message::Transfer(TransferMessage::StagedSizeMeasured { path, size }),
    )
}

fn staged_size(path: &Path, measuring: CancellationToken) -> StagedSize {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => return StagedSize::Unreadable(error.to_string()),
    };
    if !metadata.is_dir() {
        return StagedSize::Measured(RecursiveTotalSize {
            bytes: metadata.len(),
            complete: true,
        });
    }
    match read_directory_recursive_total_size(path, measuring) {
        Ok(size) => StagedSize::Measured(size),
        // 被取消的结果到达时该项已不在列表里，回填时会被丢弃。
        Err(DirectorySummaryError::Cancelled) => StagedSize::Measuring,
        Err(DirectorySummaryError::Io(error)) => StagedSize::Unreadable(error.to_string()),
        Err(DirectorySummaryError::Overflow(reason)) => StagedSize::Unreadable(reason.to_owned()),
    }
}

/// 门户选择器（同接收目录选择的 ashpd 用法），可多选；取消返回空。
pub(super) fn chooser_command(target: ChooserTarget) -> Task<Message> {
    Task::perform(choose_paths(target), |outcome| {
        Message::Transfer(TransferMessage::LocalSendPathsChosen(outcome))
    })
}

async fn choose_paths(target: ChooserTarget) -> Result<Vec<PathBuf>, String> {
    let title = match target {
        ChooserTarget::Files => tr!("Choose files to send"),
        ChooserTarget::Folders => tr!("Choose folders to send"),
    };
    let accept_label = tr!("Select");
    let request = ashpd::desktop::file_chooser::SelectedFiles::open_file()
        .title(title.as_str())
        .accept_label(accept_label.as_str())
        .modal(true)
        .directory(target == ChooserTarget::Folders)
        .multiple(true)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let selected = match request.response() {
        Ok(selected) => selected,
        Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)) => {
            return Ok(Vec::new())
        }
        Err(error) => return Err(error.to_string()),
    };
    selected
        .uris()
        .iter()
        .map(|uri| {
            uri.to_file_path()
                .map_err(|_| tr!("the selected item is not on this computer"))
        })
        .collect()
}

#[cfg(test)]
mod tests;
