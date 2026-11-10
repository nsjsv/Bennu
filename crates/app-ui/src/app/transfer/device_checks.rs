//! 设备行前的勾选框：勾了就能"给选中的 N 台发送"。
//!
//! 两个窗口（LocalSend、发送到手机）各勾各的，关哪个窗口清哪份：两边发的不是同一批
//! 文件（待发列表 / 打开窗口时那批）。按证书指纹记，设备暂时不在线也留着勾，回来
//! 照样算；N 只算此刻在线的。

use std::collections::HashSet;

use local_send::DeviceInfo;

use super::super::FileBrowser;
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
    pub(super) fn check_device(
        &mut self,
        owner: DeviceListOwner,
        fingerprint: String,
        checked: bool,
    ) {
        let set = self.checked_devices.of_mut(owner);
        if checked {
            set.insert(fingerprint);
        } else {
            set.remove(&fingerprint);
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
        let paths = match owner {
            DeviceListOwner::LocalSendWindow => self.staged_paths(),
            DeviceListOwner::TransferWindow => self
                .transfer_session
                .as_ref()
                .map(|session| session.paths.clone())
                .unwrap_or_default(),
        };
        self.begin_direct_sends(devices, paths)
    }
}
