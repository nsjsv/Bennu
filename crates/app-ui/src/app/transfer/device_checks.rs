//! 两个窗口（LocalSend、发送到手机）的设备格子：点一下发这个窗口要发的东西（发送本身就是
//! 检测，连不上就在这台设备上显示失败），没东西可发时只检测这一台；按住 Ctrl 点是勾上，
//! 勾了就能"给选中的 N 台发送"。
//!
//! 两个窗口各发各的、各勾各的，关哪个窗口清哪份：两边发的不是同一批（待发列表或文字 /
//! 打开窗口时那批文件）。勾选按证书指纹记，设备暂时不见了也留着勾，回来照样算；N 只算
//! 此刻连得到的（在线的，和记着地址的不在线收藏）。

use std::collections::HashSet;

use local_send::DeviceInfo;

use super::super::FileBrowser;
use super::{FavoriteRow, Outgoing};
use crate::model::Message;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeviceListOwner {
    LocalSendWindow,
    TransferWindow,
}

#[derive(Debug, Default)]
pub(crate) struct CheckedDevices {
    local_send: HashSet<String>,
    transfer: HashSet<String>,
}

impl CheckedDevices {
    pub(crate) fn of(&self, owner: DeviceListOwner) -> &HashSet<String> {
        match owner {
            DeviceListOwner::LocalSendWindow => &self.local_send,
            DeviceListOwner::TransferWindow => &self.transfer,
        }
    }

    pub(in crate::app) fn clear(&mut self, owner: DeviceListOwner) {
        self.of_mut(owner).clear();
    }

    fn of_mut(&mut self, owner: DeviceListOwner) -> &mut HashSet<String> {
        match owner {
            DeviceListOwner::LocalSendWindow => &mut self.local_send,
            DeviceListOwner::TransferWindow => &mut self.transfer,
        }
    }
}

impl FileBrowser {
    /// 点设备就发（正在发给它时不重复发）；没东西可发时只检测这一台。按住 Ctrl 点是
    /// 勾上 / 取消，勾好几台再一起发。
    pub(super) fn press_device(
        &mut self,
        owner: DeviceListOwner,
        device: DeviceInfo,
    ) -> iced::Task<Message> {
        if self.keyboard_modifiers.control() {
            let set = self.checked_devices.of_mut(owner);
            if !set.remove(&device.fingerprint) {
                set.insert(device.fingerprint);
            }
            return iced::Task::none();
        }
        if !self.has_outgoing(owner) {
            return self.check_device(device);
        }
        let outgoing = self.outgoing_of(owner);
        self.begin_direct_sends(vec![device], outgoing)
    }

    /// 每个窗口发什么只在这里分：LocalSend 看待发卡片，发送到手机是打开窗口时那批文件。
    pub(super) fn outgoing_of(&self, owner: DeviceListOwner) -> Outgoing {
        match owner {
            DeviceListOwner::LocalSendWindow => self.local_send_outgoing(),
            DeviceListOwner::TransferWindow => Outgoing::Files(
                self.transfer_session
                    .as_ref()
                    .map(|session| session.paths.clone())
                    .unwrap_or_default(),
            ),
        }
    }

    /// 有没有东西可发：设备区的提示和勾选顶栏的“发送”据此变。
    pub(crate) fn has_outgoing(&self, owner: DeviceListOwner) -> bool {
        match owner {
            DeviceListOwner::LocalSendWindow => self.local_send_has_outgoing(),
            DeviceListOwner::TransferWindow => self
                .transfer_session
                .as_ref()
                .is_some_and(|session| !session.paths.is_empty()),
        }
    }

    /// 勾着且此刻连得到的设备（顶栏的 N）。
    pub(crate) fn checked_targets(&self, owner: DeviceListOwner) -> Vec<DeviceInfo> {
        let checked = self.checked_devices.of(owner);
        self.favorite_rows()
            .iter()
            .filter_map(FavoriteRow::target)
            .chain(self.unclaimed_nearby_devices().into_iter().cloned())
            .filter(|device| checked.contains(&device.fingerprint))
            .collect()
    }

    /// 发给勾着的设备；勾选保留，换一批文件接着发给同一组人不用重勾。
    pub(super) fn send_to_checked_devices(
        &mut self,
        owner: DeviceListOwner,
    ) -> iced::Task<Message> {
        let devices = self.checked_targets(owner);
        let outgoing = self.outgoing_of(owner);
        self.begin_direct_sends(devices, outgoing)
    }
}
