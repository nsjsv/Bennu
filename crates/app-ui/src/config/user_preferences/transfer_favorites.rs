//! 收藏设备的存储形态 ↔ 配置：读配置是收藏设备进程序的唯一入口，校验只在这里做。

use super::super::unrecognized_settings::UnrecognizedSettings;
use super::super::{FavoriteTransferDevice, StoredFavoriteTransferDevice};

pub(super) fn stored_favorite_transfer_devices(
    devices: &[FavoriteTransferDevice],
) -> Vec<StoredFavoriteTransferDevice> {
    devices
        .iter()
        .map(|device| StoredFavoriteTransferDevice {
            fingerprint: device.fingerprint.clone(),
            alias: device.alias.clone(),
            custom_alias: device.custom_alias,
            address: device.address.map(|address| address.to_string()),
            added_at: device.added_at.clone(),
        })
        .collect()
}

/// 收藏设备的唯一入口校验：空 fingerprint 丢弃（不能让它匹配上任何设备）；fingerprint
/// 转大写（证书指纹的格式；升级来的旧信任设备存的是对方自报的，大小写不定）；地址
/// 读不懂的只丢地址，设备照留，搜到后会补上。
pub(super) fn favorite_transfer_devices_from_stored(
    stored: &[StoredFavoriteTransferDevice],
    unrecognized: &mut UnrecognizedSettings,
) -> Vec<FavoriteTransferDevice> {
    let mut unreadable_address = false;
    let devices = stored
        .iter()
        .filter(|device| !device.fingerprint.is_empty())
        .map(|device| FavoriteTransferDevice {
            fingerprint: device.fingerprint.to_ascii_uppercase(),
            alias: if device.alias.is_empty() {
                device.fingerprint.clone()
            } else {
                device.alias.clone()
            },
            custom_alias: device.custom_alias,
            address: device.address.as_deref().and_then(|address| {
                let parsed = address.parse().ok();
                unreadable_address |= parsed.is_none();
                parsed
            }),
            added_at: device.added_at.clone(),
        })
        .collect::<Vec<_>>();
    unrecognized.note_dropped("transfer_trusted_devices", stored.len(), devices.len());
    if unreadable_address {
        unrecognized.note("transfer_trusted_devices");
    }
    devices
}
