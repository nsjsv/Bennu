//! 设备列表怎么找：有窗口在显示设备时服务才主动找（看列表的凭据由订阅拿着，见
//! `transfer_service`）。手动刷新时箭头一直转到这一轮找完，不然用户以为没点中会狂点；
//! 点一台设备又没东西可发时只连它这一台，比找一整轮快得多。
//!
//! 刷新、检测的结果按服务的代对号：服务重启后旧服务回来的结果作废。

use std::collections::HashSet;
use std::time::{Duration, Instant};

use iced::Task;
use local_send::DeviceInfo;

use super::super::FileBrowser;
use super::{LocalSendPage, TransferMessage};
use crate::model::Message;

/// 刷新箭头转一圈的时长。
const REFRESH_TURN: Duration = Duration::from_millis(800);

/// 跟着服务的那一代（`LocalSendRuntime`）走。
#[derive(Debug, Default)]
pub(crate) struct DeviceSearch {
    refresh: Option<RefreshSpin>,
    /// 正在单独检测的设备（证书指纹）。
    checking: HashSet<String>,
}

#[derive(Debug, Clone, Copy)]
struct RefreshSpin {
    started: Instant,
    /// 这一轮找完后转满当前这一圈、箭头回正的时刻；还在找时 None。
    stops_at: Option<Instant>,
}

impl DeviceSearch {
    /// 刷新箭头此刻转到一圈的几分之几；没在刷新时 None，按钮能点。
    pub(crate) fn refresh_turn(&self, now: Instant) -> Option<f32> {
        let spin = self.refresh?;
        Some(turns_since(spin.started, now).fract())
    }

    pub(crate) fn is_checking(&self, fingerprint: &str) -> bool {
        self.checking.contains(fingerprint)
    }

    fn finish_refresh(&mut self, now: Instant) {
        if let Some(spin) = &mut self.refresh {
            let turns = turns_since(spin.started, now).ceil().max(1.0);
            spin.stops_at = Some(spin.started + REFRESH_TURN.mul_f32(turns));
        }
    }

    fn settle_refresh(&mut self, now: Instant) {
        if self
            .refresh
            .and_then(|spin| spin.stops_at)
            .is_some_and(|stops_at| now >= stops_at)
        {
            self.refresh = None;
        }
    }
}

fn turns_since(started: Instant, now: Instant) -> f32 {
    now.saturating_duration_since(started).as_secs_f32() / REFRESH_TURN.as_secs_f32()
}

impl FileBrowser {
    /// 设备列表正显示在某个窗口里：只有这时服务才主动找设备。
    pub(crate) fn shows_device_list(&self) -> bool {
        self.transfer_window.is_some()
            || (self.local_send_window.is_some() && self.local_send_page == LocalSendPage::Devices)
    }

    /// 看不见箭头时不用逐帧重画：窗口关了，刷新照样等服务找完。
    pub(crate) fn device_refresh_is_spinning(&self) -> bool {
        self.shows_device_list()
            && self
                .transfer_runtime
                .as_ref()
                .is_some_and(|runtime| runtime.search.refresh.is_some())
    }

    pub(in crate::app) fn advance_device_refresh_spin(&mut self) {
        if let Some(runtime) = self.transfer_runtime.as_mut() {
            runtime.search.settle_refresh(Instant::now());
        }
    }

    /// 刷新按钮：清空列表，服务立刻找一轮整的；转圈期间按钮点不了。
    pub(super) fn refresh_devices(&mut self) -> Task<Message> {
        let Some(runtime) = self
            .transfer_runtime
            .as_mut()
            .filter(|runtime| runtime.search.refresh.is_none())
        else {
            return Task::none();
        };
        runtime.search.refresh = Some(RefreshSpin {
            started: Instant::now(),
            stops_at: None,
        });
        runtime.devices.clear();
        let generation = runtime.generation;
        Task::perform(runtime.handle.refresh_devices(), move |()| {
            Message::Transfer(TransferMessage::DevicesRefreshed(generation))
        })
    }

    pub(super) fn accept_devices_refreshed(&mut self, generation: u64) {
        if let Some(runtime) = self
            .transfer_runtime
            .as_mut()
            .filter(|runtime| runtime.generation == generation)
        {
            runtime.search.finish_refresh(Instant::now());
        }
    }

    /// 只连这一台：连上了它留在（或回到）列表里，连不上服务会把它拿掉（`DeviceLost`），
    /// 收藏就变成不在线。正在测的再点不重复连。
    pub(super) fn check_device(&mut self, device: DeviceInfo) -> Task<Message> {
        let Some(runtime) = self.transfer_runtime.as_mut() else {
            return Task::none();
        };
        if !runtime.search.checking.insert(device.fingerprint.clone()) {
            return Task::none();
        }
        let generation = runtime.generation;
        let fingerprint = device.fingerprint.clone();
        Task::perform(runtime.handle.check_device(&device), move |()| {
            Message::Transfer(TransferMessage::DeviceChecked {
                generation,
                fingerprint,
            })
        })
    }

    pub(super) fn accept_device_checked(&mut self, generation: u64, fingerprint: &str) {
        if let Some(runtime) = self
            .transfer_runtime
            .as_mut()
            .filter(|runtime| runtime.generation == generation)
        {
            runtime.search.checking.remove(fingerprint);
        }
    }

    pub(crate) fn is_checking_device(&self, fingerprint: &str) -> bool {
        self.transfer_runtime
            .as_ref()
            .is_some_and(|runtime| runtime.search.is_checking(fingerprint))
    }

    /// 刷新箭头的角度（圈的几分之几）；没在刷新时 None。
    pub(crate) fn device_refresh_turn(&self) -> Option<f32> {
        self.transfer_runtime
            .as_ref()
            .and_then(|runtime| runtime.search.refresh_turn(Instant::now()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spinning_since(started: Instant) -> DeviceSearch {
        DeviceSearch {
            refresh: Some(RefreshSpin {
                started,
                stops_at: None,
            }),
            checking: HashSet::new(),
        }
    }

    #[test]
    fn the_arrow_keeps_turning_until_the_round_ends_then_finishes_its_turn() {
        let started = Instant::now();
        let mut search = spinning_since(started);
        // 还在找：不管转了多久都不停。
        search.settle_refresh(started + REFRESH_TURN * 5);
        assert!(search.refresh_turn(started).is_some());

        // 转到第 2 圈中间时找完：转满第 2 圈才停，箭头回正。
        search.finish_refresh(started + REFRESH_TURN.mul_f32(1.5));
        search.settle_refresh(started + REFRESH_TURN.mul_f32(1.9));
        assert!(search.refresh_turn(started).is_some());
        search.settle_refresh(started + REFRESH_TURN * 2);
        assert!(search.refresh_turn(started).is_none());
    }

    #[test]
    fn a_round_that_ends_at_once_still_turns_the_arrow_once() {
        let started = Instant::now();
        let mut search = spinning_since(started);
        search.finish_refresh(started);
        search.settle_refresh(started + REFRESH_TURN / 2);
        assert!(search.refresh_turn(started).is_some());
        search.settle_refresh(started + REFRESH_TURN);
        assert!(search.refresh_turn(started).is_none());
    }
}
