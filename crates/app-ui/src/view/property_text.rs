//! 属性的文字写法：属性窗口和信息面板共用，同一个文件在两处写得一样。

use std::path::Path;
use std::time::SystemTime;

use bennu_preview::formatting::file_kind_label;

use crate::formatting::{format_file_size, format_system_time};
use crate::model::{FilePropertiesKindDetails, FilePropertiesPermissions, FilePropertiesSnapshot};

/// 普通文件「PNG 图像（image/png）」；类型库没有类型名的只写类型；其余是种类名。
pub(super) fn type_text(details: &FilePropertiesKindDetails) -> String {
    match details {
        FilePropertiesKindDetails::File(file) => match &file.description {
            Some(description) => tr!("%1 (%2)", description, file.mime_type),
            None => file.mime_type.clone(),
        },
        details => file_kind_label(details.kind()),
    }
}

/// 文件系统不记创建时间（老式 ext4、网络和共享目录）时不显示这一行，不写「不可用」：
/// 这是文件系统的事，不是这个文件出了问题。状态改变时间（ctime）放最后。
pub(super) fn time_rows(snapshot: &FilePropertiesSnapshot) -> Vec<(String, String)> {
    let created = snapshot
        .created
        .map(|created| (tr!("Created"), format_system_time(created)));
    created
        .into_iter()
        .chain([
            (tr!("Modified"), display_optional_time(snapshot.modified)),
            (tr!("Accessed"), display_optional_time(snapshot.accessed)),
            (
                tr!("Status Changed"),
                display_optional_time(snapshot.changed),
            ),
        ])
        .collect()
}

pub(super) fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

pub(super) fn display_optional_time(time: Option<SystemTime>) -> String {
    time.map(format_system_time)
        .unwrap_or_else(|| tr!("Unavailable"))
}

pub(super) fn display_size(bytes: u64) -> String {
    tr!("%1 (%2 bytes)", format_file_size(bytes), bytes)
}

pub(super) fn display_permissions(permissions: FilePropertiesPermissions) -> String {
    format!(
        "{} ({})",
        permissions.symbolic_string(),
        permissions.octal_string()
    )
}
