//! 两个窗口（LocalSend、发送到手机）的设备格子：点一下发这个窗口要发的东西，按住 Ctrl
//! 点是勾上，勾了就能"给选中的 N 台发送"。
//!
//! 两个窗口各发各的、各勾各的，关哪个窗口清哪份：两边发的不是同一批（待发列表或文字 /
//! 打开窗口时那批文件）。勾选按证书指纹记，设备暂时不在线也留着勾，回来照样算；N 只算
//! 此刻在线的。

use std::collections::HashSet;

use local_send::DeviceInfo;

use super::super::FileBrowser;
use super::Outgoing;
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
    /// 点设备就发（没东西可发、正在发给它时什么都不做）；按住 Ctrl 点是勾上 / 取消，
    /// 勾好几台再一起发。
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

    /// 勾着且此刻在线的设备（顶栏的 N）。
    pub(crate) fn checked_online_devices(&self, owner: DeviceListOwner) -> Vec<&DeviceInfo> {
        let checked = self.checked_devices.of(owner);
        self.nearby_devices()
            .iter()
            .filter(|device| checked.contains(&device.fingerprint))
            .collect()
    }

    /// 发给勾着的在线设备；勾选保留，换一批文件接着发给同一组人不用重勾。
    pub(super) fn send_to_checked_devices(
        &mut self,
        owner: DeviceListOwner,
    ) -> iced::Task<Message> {
        let devices = self
            .checked_online_devices(owner)
            .into_iter()
            .cloned()
            .collect();
        let outgoing = self.outgoing_of(owner);
        self.begin_direct_sends(devices, outgoing)
    }
}
