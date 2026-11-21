//! 重读同一个条目时哪些慢项沿用上次的、哪些再读。

use super::SlowPropertyLookups;
use crate::model::{
    FilePropertiesDirectoryContentsState, FilePropertiesKindDetails, FilePropertiesLookup,
    FilePropertiesReread, FilePropertiesSnapshot,
};

/// 重读同一个条目时，上次读到的慢项先摆着，读到新的再换，不闪回“读取中”。依据没变的干脆
/// 不再读：打开方式只看类型，详细信息只看大小和修改时间，文件夹大小只看里面的东西；用户按了
/// 刷新（`All`）时都再读。种类变了（文件换成了同名文件夹）什么都不沿用。
pub(super) fn keep_previous_slow_properties(
    snapshot: &mut FilePropertiesSnapshot,
    lookups: &mut SlowPropertyLookups,
    previous: &FilePropertiesSnapshot,
    reread: FilePropertiesReread,
) {
    let content_unchanged =
        snapshot.size_bytes == previous.size_bytes && snapshot.modified == previous.modified;
    match (&mut snapshot.details, &previous.details) {
        (FilePropertiesKindDetails::File(file), FilePropertiesKindDetails::File(before)) => {
            if file.mime_type == before.mime_type
                && before.default_application != FilePropertiesLookup::Pending
            {
                file.default_application = before.default_application.clone();
                if reread != FilePropertiesReread::All {
                    lookups.default_application_for = None;
                }
            }
            if let (Some(details), Some(before_details)) =
                (&mut file.media_details, &before.media_details)
            {
                if *before_details != FilePropertiesLookup::Pending {
                    *details = before_details.clone();
                    if reread != FilePropertiesReread::All && content_unchanged {
                        lookups.media = None;
                    }
                }
            }
        }
        (
            FilePropertiesKindDetails::Directory(contents),
            FilePropertiesKindDetails::Directory(
                FilePropertiesDirectoryContentsState::Loaded(measured)
                | FilePropertiesDirectoryContentsState::Refreshing(measured),
            ),
        ) => {
            snapshot.size_bytes = measured.total_size_bytes;
            snapshot.disk_size_bytes = measured.total_disk_size_bytes;
            if reread == FilePropertiesReread::Item {
                *contents = FilePropertiesDirectoryContentsState::Loaded(measured.clone());
                lookups.directory_contents = false;
            } else {
                *contents = FilePropertiesDirectoryContentsState::Refreshing(measured.clone());
            }
        }
        _ => {}
    }
}
