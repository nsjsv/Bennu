use std::path::Path;

#[cfg(unix)]
use std::ffi::OsStr;

use super::transfer_object::{TransferSourceKind, TransferSourceObject};

pub(super) async fn apply_transfer_metadata_best_effort(
    source: &Path,
    target: &Path,
    source_object: &TransferSourceObject,
) {
    let source = source.to_path_buf();
    let target = target.to_path_buf();
    let source_object = source_object.clone();
    let completion = tokio::task::spawn_blocking(move || match source_object.kind {
        TransferSourceKind::RegularFile | TransferSourceKind::Directory => {
            preserve_file_or_directory_metadata(&source, &target, &source_object.metadata);
        }
        TransferSourceKind::SymbolicLink { .. } => {
            preserve_symbolic_link_metadata(&source, &target, &source_object.metadata);
        }
    })
    .await;

    if let Err(join_error) = completion {
        if join_error.is_panic() {
            std::panic::resume_unwind(join_error.into_panic());
        }
    }
}

fn preserve_file_or_directory_metadata(
    source: &Path,
    target: &Path,
    source_metadata: &std::fs::Metadata,
) {
    copy_extended_attributes(source, target);
    let _ = std::fs::set_permissions(target, source_metadata.permissions());
    copy_posix_acl(source, target);
    let access_time = filetime::FileTime::from_last_access_time(source_metadata);
    let modification_time = filetime::FileTime::from_last_modification_time(source_metadata);
    let _ = filetime::set_file_times(target, access_time, modification_time);
}

fn preserve_symbolic_link_metadata(
    source: &Path,
    target: &Path,
    source_metadata: &std::fs::Metadata,
) {
    copy_extended_attributes(source, target);
    let access_time = filetime::FileTime::from_last_access_time(source_metadata);
    let modification_time = filetime::FileTime::from_last_modification_time(source_metadata);
    let _ = filetime::set_symlink_file_times(target, access_time, modification_time);
}

#[cfg(unix)]
fn copy_extended_attributes(source: &Path, target: &Path) {
    let Ok(attribute_names) = xattr::list(source) else {
        return;
    };

    for attribute_name in attribute_names {
        if is_posix_acl_attribute(&attribute_name) {
            continue;
        }
        let Ok(Some(value)) = xattr::get(source, &attribute_name) else {
            continue;
        };
        let _ = xattr::set(target, &attribute_name, &value);
    }
}

#[cfg(not(unix))]
fn copy_extended_attributes(_source: &Path, _target: &Path) {}

#[cfg(unix)]
fn is_posix_acl_attribute(attribute_name: &OsStr) -> bool {
    attribute_name == OsStr::new("system.posix_acl_access")
        || attribute_name == OsStr::new("system.posix_acl_default")
}

#[cfg(target_os = "linux")]
fn copy_posix_acl(source: &Path, target: &Path) {
    let Ok(entries) = exacl::getfacl(source, None) else {
        return;
    };
    let _ = exacl::setfacl(&[target], &entries, None);
}

#[cfg(not(target_os = "linux"))]
fn copy_posix_acl(_source: &Path, _target: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::transfer_object::inspect_transfer_source;
    use tempfile::tempdir;

    #[tokio::test]
    async fn unavailable_metadata_source_is_best_effort() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source.txt");
        let target = directory.path().join("target.txt");
        std::fs::write(&source, b"payload").unwrap();
        std::fs::write(&target, b"payload").unwrap();
        let source_object = inspect_transfer_source(&source).await.unwrap();
        std::fs::remove_file(&source).unwrap();

        apply_transfer_metadata_best_effort(&source, &target, &source_object).await;

        assert_eq!(std::fs::read(&target).unwrap(), b"payload");
    }
}
