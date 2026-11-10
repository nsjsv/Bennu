//! LocalSend 收藏设备：唯一的改动入口、设备列表划分、"按 IP 添加"表单。
//!
//! 收藏列表只存在 UserConfig 里。每次改动都经 `change_favorites`：改配置 → 推给常驻
//! 服务（接收时判信任、扫描时按记住的地址连）→ 持久化，所以服务手里的快照永远等于
//! 配置。只收 `is_certified()` 的设备：明文设备的指纹是自报的，收藏它等于让冒充者
//! 自动往这台电脑发文件、改掉收藏记住的地址。

use std::net::{Ipv4Addr, SocketAddr};

use iced::Task;
use local_send::{DeviceInfo, FindDeviceError};

use super::super::FileBrowser;
use super::TransferMessage;
use crate::config::FavoriteTransferDevice;
use crate::model::Message;

/// 收藏区的一行：收藏的设备，配上此刻在线的那台（同指纹、证书认过）。
pub(crate) struct FavoriteRow<'a> {
    pub(crate) favorite: &'a FavoriteTransferDevice,
    pub(crate) online: Option<&'a DeviceInfo>,
}

/// LocalSend 窗口里展开的"按 IP 添加"表单。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FavoriteAddressForm {
    pub(crate) name: String,
    pub(crate) ip: String,
    pub(crate) port: String,
    /// 正在连的地址：连接结果按它对号，取消后才回来的结果对不上就丢掉。
    pub(crate) connecting: Option<SocketAddr>,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) enum AddressFormMessage {
    Opened,
    NameChanged(String),
    IpChanged(String),
    PortChanged(String),
    Submitted,
    Canceled,
    Found {
        address: SocketAddr,
        outcome: Result<DeviceInfo, FindDeviceError>,
    },
}

impl FileBrowser {
    /// 发现的设备快照（证书指纹去重）；服务没在运行时为空。
    pub(crate) fn nearby_devices(&self) -> &[DeviceInfo] {
        self.transfer_runtime
            .as_ref()
            .map(|runtime| runtime.devices.as_slice())
            .unwrap_or(&[])
    }

    fn online_favorite(&self, fingerprint: &str) -> Option<&DeviceInfo> {
        online_among(self.nearby_devices(), fingerprint)
    }

    pub(crate) fn favorite_rows(&self) -> Vec<FavoriteRow<'_>> {
        favorite_rows_among(
            &self.user_config.transfer_favorite_devices,
            self.nearby_devices(),
        )
    }

    pub(crate) fn unclaimed_nearby_devices(&self) -> Vec<&DeviceInfo> {
        unclaimed_among(
            &self.user_config.transfer_favorite_devices,
            self.nearby_devices(),
        )
    }

    /// 所有改动的唯一出口：改了才推给服务并持久化。
    fn change_favorites(
        &mut self,
        change: impl FnOnce(&mut Vec<FavoriteTransferDevice>) -> bool,
    ) -> Task<Message> {
        if !change(&mut self.user_config.transfer_favorite_devices) {
            return Task::none();
        }
        if let Some(runtime) = &self.transfer_runtime {
            runtime.handle.set_favorites(service_favorites(
                &self.user_config.transfer_favorite_devices,
            ));
        }
        self.persist_user_preferences_command()
    }

    /// 加进收藏（☆、接收确认里的"加入收藏"、按 IP 添加）；已在收藏里就更新它。
    /// `custom_name` 是用户起的名字。
    pub(super) fn favor_device(
        &mut self,
        device: &DeviceInfo,
        custom_name: Option<String>,
    ) -> Task<Message> {
        self.change_favorites(|favorites| merge_favorite(favorites, device, custom_name))
    }

    /// ★ 与设置页"删除"：之后这台设备发来的文件又要确认。
    pub(super) fn unfavor_device(&mut self, fingerprint: &str) -> Task<Message> {
        self.favorite_alias_inputs.remove(fingerprint);
        self.change_favorites(|favorites| {
            let before = favorites.len();
            favorites.retain(|favorite| favorite.fingerprint != fingerprint);
            favorites.len() != before
        })
    }

    /// 发现了一台设备：是收藏就跟上它的地址（以及没改过的名字）。
    pub(super) fn refresh_favorite_from(&mut self, device: &DeviceInfo) -> Task<Message> {
        self.change_favorites(|favorites| {
            favorites
                .iter_mut()
                .find(|favorite| favorite.fingerprint == device.fingerprint)
                .is_some_and(|favorite| update_favorite(favorite, device, None))
        })
    }

    pub(super) fn edit_favorite_alias(&mut self, fingerprint: String, value: String) {
        self.favorite_alias_inputs.insert(fingerprint, value);
    }

    /// 设置页回车：有字 = 自己起的名字；清空 = 恢复成对方设备的名字（不在线就等下次
    /// 搜到再换）。
    pub(super) fn commit_favorite_alias(&mut self, fingerprint: &str) -> Task<Message> {
        let Some(value) = self.favorite_alias_inputs.remove(fingerprint) else {
            return Task::none();
        };
        let online_alias = self
            .online_favorite(fingerprint)
            .map(|device| device.alias.clone());
        let name = value.trim().to_owned();
        self.change_favorites(|favorites| {
            let Some(favorite) = favorites
                .iter_mut()
                .find(|favorite| favorite.fingerprint == fingerprint)
            else {
                return false;
            };
            let before = favorite.clone();
            if name.is_empty() {
                favorite.custom_alias = false;
                if let Some(alias) = online_alias {
                    favorite.alias = alias;
                }
            } else {
                favorite.alias = name;
                favorite.custom_alias = true;
            }
            *favorite != before
        })
    }

    pub(super) fn handle_address_form(&mut self, message: AddressFormMessage) -> Task<Message> {
        match message {
            AddressFormMessage::Opened => {
                let port = self.user_config.transfer_port.to_string();
                self.favorite_address_form
                    .get_or_insert_with(|| FavoriteAddressForm {
                        name: String::new(),
                        ip: String::new(),
                        port,
                        connecting: None,
                        error: None,
                    });
                Task::none()
            }
            AddressFormMessage::NameChanged(value) => {
                self.edit_address_form(|form| form.name = value);
                Task::none()
            }
            AddressFormMessage::IpChanged(value) => {
                self.edit_address_form(|form| form.ip = value);
                Task::none()
            }
            AddressFormMessage::PortChanged(value) => {
                self.edit_address_form(|form| form.port = value);
                Task::none()
            }
            AddressFormMessage::Submitted => self.submit_address_form(),
            AddressFormMessage::Canceled => {
                self.favorite_address_form = None;
                Task::none()
            }
            AddressFormMessage::Found { address, outcome } => {
                self.accept_found_device(address, outcome)
            }
        }
    }

    /// 连接中的表单不能改：结果要对上用户看到的那个地址。
    fn edit_address_form(&mut self, edit: impl FnOnce(&mut FavoriteAddressForm)) {
        if let Some(form) = self
            .favorite_address_form
            .as_mut()
            .filter(|form| form.connecting.is_none())
        {
            edit(form);
            form.error = None;
        }
    }

    fn submit_address_form(&mut self) -> Task<Message> {
        let Some(form) = self
            .favorite_address_form
            .as_mut()
            .filter(|form| form.connecting.is_none())
        else {
            return Task::none();
        };
        let Ok(ip) = form.ip.trim().parse::<Ipv4Addr>() else {
            form.error = Some(tr!("Enter an IPv4 address, like 192.168.1.20."));
            return Task::none();
        };
        let Some(port) = form
            .port
            .trim()
            .parse::<u16>()
            .ok()
            .filter(|port| *port != 0)
        else {
            form.error = Some(tr!("Port must be between %1 and %2.", 1, u16::MAX));
            return Task::none();
        };
        let Some(runtime) = &self.transfer_runtime else {
            form.error = Some(tr!("LocalSend is not running."));
            return Task::none();
        };
        let address = SocketAddr::from((ip, port));
        form.connecting = Some(address);
        form.error = None;
        Task::perform(runtime.handle.find_device(address), move |outcome| {
            Message::Transfer(TransferMessage::AddressForm(AddressFormMessage::Found {
                address,
                outcome,
            }))
        })
    }

    fn accept_found_device(
        &mut self,
        address: SocketAddr,
        outcome: Result<DeviceInfo, FindDeviceError>,
    ) -> Task<Message> {
        let Some(form) = self
            .favorite_address_form
            .as_mut()
            .filter(|form| form.connecting == Some(address))
        else {
            return Task::none();
        };
        match outcome {
            Ok(device) => {
                let name = form.name.trim();
                let custom_name = (!name.is_empty()).then(|| name.to_owned());
                self.favorite_address_form = None;
                self.favor_device(&device, custom_name)
            }
            Err(error) => {
                form.connecting = None;
                form.error = Some(find_device_error_message(&error));
                Task::none()
            }
        }
    }
}

/// 收藏只认领证书认过的同指纹设备。
fn online_among<'a>(devices: &'a [DeviceInfo], fingerprint: &str) -> Option<&'a DeviceInfo> {
    devices
        .iter()
        .find(|device| device.is_certified() && device.fingerprint == fingerprint)
}

fn favorite_rows_among<'a>(
    favorites: &'a [FavoriteTransferDevice],
    devices: &'a [DeviceInfo],
) -> Vec<FavoriteRow<'a>> {
    favorites
        .iter()
        .map(|favorite| FavoriteRow {
            favorite,
            online: online_among(devices, &favorite.fingerprint),
        })
        .collect()
}

/// "附近"区：没被收藏区认领的设备。明文设备即使自报的指纹和某个收藏相同也留在
/// 这里：认不了它是不是那台。
fn unclaimed_among<'a>(
    favorites: &[FavoriteTransferDevice],
    devices: &'a [DeviceInfo],
) -> Vec<&'a DeviceInfo> {
    devices
        .iter()
        .filter(|device| {
            !(device.is_certified()
                && favorites
                    .iter()
                    .any(|favorite| favorite.fingerprint == device.fingerprint))
        })
        .collect()
}

/// 加进收藏或更新已有的那条；明文设备一律不收（见模块说明）。
fn merge_favorite(
    favorites: &mut Vec<FavoriteTransferDevice>,
    device: &DeviceInfo,
    custom_name: Option<String>,
) -> bool {
    if !device.is_certified() {
        return false;
    }
    if let Some(existing) = favorites
        .iter_mut()
        .find(|favorite| favorite.fingerprint == device.fingerprint)
    {
        return update_favorite(existing, device, custom_name);
    }
    favorites.push(FavoriteTransferDevice {
        fingerprint: device.fingerprint.clone(),
        custom_alias: custom_name.is_some(),
        alias: custom_name.unwrap_or_else(|| device.alias.clone()),
        address: device_address(device),
        added_at: chrono::Utc::now().to_rfc3339(),
    });
    true
}

/// 跟上这一次见到的地址；给了名字就用它，没给时名字没改过就跟着对方的名字。
fn update_favorite(
    favorite: &mut FavoriteTransferDevice,
    device: &DeviceInfo,
    custom_name: Option<String>,
) -> bool {
    if !device.is_certified() {
        return false;
    }
    let before = favorite.clone();
    if let Some(address) = device_address(device) {
        favorite.address = Some(address);
    }
    match custom_name {
        Some(name) => {
            favorite.alias = name;
            favorite.custom_alias = true;
        }
        None if !favorite.custom_alias => favorite.alias = device.alias.clone(),
        None => {}
    }
    *favorite != before
}

/// 收藏记的是"IP + 它自己的端口"：扫描按这个地址直接连。
fn device_address(device: &DeviceInfo) -> Option<SocketAddr> {
    device.address.map(|ip| SocketAddr::new(ip, device.port))
}

pub(crate) fn service_favorites(favorites: &[FavoriteTransferDevice]) -> Vec<local_send::Favorite> {
    favorites
        .iter()
        .map(|favorite| local_send::Favorite {
            fingerprint: favorite.fingerprint.clone(),
            address: favorite.address,
        })
        .collect()
}

fn find_device_error_message(error: &FindDeviceError) -> String {
    match error {
        FindDeviceError::Unreachable(reason) => tr!("Could not connect: %1", reason),
        FindDeviceError::NotEncrypted => {
            tr!("This device has encryption turned off, so it cannot be a favorite.")
        }
        FindDeviceError::ThisDevice => tr!("This is this computer."),
    }
}

#[cfg(test)]
mod tests;
