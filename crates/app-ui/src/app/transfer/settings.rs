//! 设置页的传输项：接收开关、端口、接收目录、设备名（收藏设备在 `favorites.rs`）。
//!
//! 改动服务启动时就确定的配置（开关、端口、目录、设备名）都重启常驻服务并持久化。

use std::path::PathBuf;

use local_send::ReceiverFailure;

use super::super::FileBrowser;
use super::TransferMessage;
use crate::config::{default_transfer_device_alias, transfer_port_from};
use crate::model::Message;

impl FileBrowser {
    /// 设置页生效别名（配置值优先，缺省 hostname）。
    pub(crate) fn effective_transfer_device_alias(&self) -> String {
        self.user_config
            .transfer_device_alias
            .clone()
            .filter(|alias| !alias.trim().is_empty())
            .unwrap_or_else(default_transfer_device_alias)
    }

    /// 接收端没开起来或已停止的原因；服务重启换代后重新计算。
    pub(crate) fn transfer_receiver_failure(&self) -> Option<&ReceiverFailure> {
        self.transfer_runtime
            .as_ref()
            .and_then(|runtime| runtime.receiver_failure.as_ref())
    }

    pub(super) fn toggle_transfer_receive(&mut self) -> iced::Task<Message> {
        self.user_config.transfer_receive_enabled = !self.user_config.transfer_receive_enabled;
        self.start_or_restart_local_send_service();
        self.persist_user_preferences_command()
    }

    pub(super) fn edit_transfer_port(&mut self, value: String) {
        self.transfer_port_input = value;
        // 提示针对的是上一次提交的值；再改就不再适用。
        self.transfer_port_invalid = false;
    }

    /// 不合法的输入只提示、不改配置也不重启，原端口照常用。
    pub(super) fn commit_transfer_port(&mut self) -> iced::Task<Message> {
        let Some(port) = self
            .transfer_port_input
            .trim()
            .parse::<u16>()
            .ok()
            .and_then(transfer_port_from)
        else {
            self.transfer_port_invalid = true;
            return iced::Task::none();
        };
        self.transfer_port_invalid = false;
        self.transfer_port_input = port.to_string();
        if port == self.user_config.transfer_port {
            return iced::Task::none();
        }
        self.user_config.transfer_port = port;
        self.start_or_restart_local_send_service();
        self.persist_user_preferences_command()
    }

    pub(super) fn accept_transfer_download_dir(
        &mut self,
        outcome: Result<Option<PathBuf>, String>,
    ) -> iced::Task<Message> {
        match outcome {
            Ok(Some(dir)) => {
                self.user_config.transfer_download_dir = Some(dir);
                // 接收目录在服务启动时确定：改目录必须重启常驻服务。
                self.start_or_restart_local_send_service();
                self.persist_user_preferences_command()
            }
            Ok(None) => iced::Task::none(),
            Err(error) => {
                self.show_global_error(error);
                iced::Task::none()
            }
        }
    }

    pub(super) fn commit_transfer_device_alias(&mut self) -> iced::Task<Message> {
        let trimmed = self.transfer_device_alias_input.trim().to_owned();
        let configured = if trimmed.is_empty() {
            // 清空输入 = 回退 hostname 默认。
            None
        } else if Some(&trimmed) == self.user_config.transfer_device_alias.as_ref() {
            return iced::Task::none();
        } else {
            Some(trimmed)
        };
        self.user_config.transfer_device_alias = configured;
        self.start_or_restart_local_send_service();
        self.persist_user_preferences_command()
    }
}

/// 接收端失败的界面文案：弹出提示与设置页常驻原因共用。
pub(crate) fn receiver_failure_message(failure: &ReceiverFailure) -> String {
    match failure {
        ReceiverFailure::PortInUse { port } => tr!(
            "Could not start receiving: port %1 is already in use. Turn off receiving or choose another port.",
            port
        ),
        ReceiverFailure::BindFailed { port, reason } => {
            tr!("Could not start receiving on port %1: %2", port, reason)
        }
        ReceiverFailure::Stopped { port, reason } => {
            tr!("Receiving stopped (port %1): %2", port, reason)
        }
    }
}

/// 设置页接收目录选择（ashpd 目录选择器，复用搜索路径选择先例）。
pub(super) fn download_dir_chooser_command() -> iced::Task<Message> {
    iced::Task::perform(choose_download_directory(), |outcome| {
        Message::Transfer(TransferMessage::DownloadDirChosen(outcome))
    })
}

async fn choose_download_directory() -> Result<Option<PathBuf>, String> {
    let title = tr!("Choose receive directory");
    let accept_label = tr!("Select");
    let request = ashpd::desktop::file_chooser::SelectedFiles::open_file()
        .title(title.as_str())
        .accept_label(accept_label.as_str())
        .modal(true)
        .directory(true)
        .multiple(false)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let selected = match request.response() {
        Ok(selected) => selected,
        Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)) => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    selected
        .uris()
        .first()
        .map(|uri| {
            uri.to_file_path()
                .map_err(|_| tr!("the selected location is not a local directory"))
        })
        .transpose()
}
