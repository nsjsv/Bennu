use super::*;
use crate::ops::transfer_object::inspect_transfer_source;
use tempfile::tempdir;

/// 按 errno 制造失败的假适配层;`None` 表示该调用成功。
#[derive(Default)]
struct FakeMetadataSystem {
    source_attributes: Vec<(OsString, Vec<u8>)>,
    attribute_list_errno: Option<i32>,
    attribute_write_errno: Option<i32>,
    permissions_errno: Option<i32>,
    source_acl: Option<Vec<exacl::AclEntry>>,
    acl_read_errno: Option<i32>,
    acl_write_errno: Option<i32>,
    times_errno: Option<i32>,
    written_attributes: Vec<OsString>,
    acl_written: bool,
}

fn outcome(errno: Option<i32>) -> io::Result<()> {
    errno.map_or(Ok(()), |code| Err(io::Error::from_raw_os_error(code)))
}

impl MetadataSystem for FakeMetadataSystem {
    fn extended_attribute_names(&mut self, _path: &Path) -> io::Result<Vec<OsString>> {
        outcome(self.attribute_list_errno)?;
        Ok(self
            .source_attributes
            .iter()
            .map(|(name, _)| name.clone())
            .collect())
    }

    fn extended_attribute(&mut self, _path: &Path, name: &OsStr) -> io::Result<Option<Vec<u8>>> {
        Ok(self
            .source_attributes
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.clone()))
    }

    fn set_extended_attribute(
        &mut self,
        _path: &Path,
        name: &OsStr,
        _value: &[u8],
    ) -> io::Result<()> {
        outcome(self.attribute_write_errno)?;
        self.written_attributes.push(name.to_os_string());
        Ok(())
    }

    fn set_permissions(
        &mut self,
        _path: &Path,
        _permissions: std::fs::Permissions,
    ) -> io::Result<()> {
        outcome(self.permissions_errno)
    }

    fn access_control_list(&mut self, _path: &Path) -> io::Result<AccessControlList> {
        outcome(self.acl_read_errno)?;
        Ok(AccessControlList(
            self.source_acl
                .clone()
                .unwrap_or_else(|| exacl::from_mode(0o644)),
        ))
    }

    fn set_access_control_list(
        &mut self,
        _path: &Path,
        _acl: &AccessControlList,
    ) -> io::Result<()> {
        outcome(self.acl_write_errno)?;
        self.acl_written = true;
        Ok(())
    }

    fn set_times(
        &mut self,
        _path: &Path,
        _accessed: FileTime,
        _modified: FileTime,
    ) -> io::Result<()> {
        outcome(self.times_errno)
    }

    fn set_symbolic_link_times(
        &mut self,
        _path: &Path,
        _accessed: FileTime,
        _modified: FileTime,
    ) -> io::Result<()> {
        outcome(self.times_errno)
    }
}

fn extended_acl() -> Vec<exacl::AclEntry> {
    let mut entries = exacl::from_mode(0o640);
    entries.push(exacl::AclEntry::allow_user("0", exacl::Perm::READ, None));
    entries.push(exacl::AclEntry::allow_mask(exacl::Perm::READ, None));
    entries
}

/// 真实文件提供 `std::fs::Metadata`;元数据写入全部经假适配层,不触碰它。
struct SourceFile {
    _dir: tempfile::TempDir,
    path: std::path::PathBuf,
    target: std::path::PathBuf,
    object: TransferSourceObject,
}

async fn source_file() -> SourceFile {
    let dir = tempdir().unwrap();
    let path = dir.path().join("source.txt");
    let target = dir.path().join("target.txt");
    std::fs::write(&path, b"payload").unwrap();
    let object = inspect_transfer_source(&path).await.unwrap();
    SourceFile {
        _dir: dir,
        path,
        target,
        object,
    }
}

fn preserve(
    system: &mut FakeMetadataSystem,
    file: &SourceFile,
) -> Result<BTreeSet<MetadataKind>, FileError> {
    preserve_metadata(system, &file.path, &file.target, &file.object)
}

#[tokio::test]
async fn time_restore_failure_is_reported_with_the_target_path() {
    let file = source_file().await;
    let mut system = FakeMetadataSystem {
        times_errno: Some(libc::EPERM),
        ..Default::default()
    };

    let error = preserve(&mut system, &file).unwrap_err();

    match error {
        FileError::PreserveTimes { path, source } => {
            assert_eq!(path, file.target);
            assert_eq!(source.raw_os_error(), Some(libc::EPERM));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn symbolic_link_time_restore_failure_is_reported() {
    let dir = tempdir().unwrap();
    let link = dir.path().join("link");
    std::os::unix::fs::symlink("missing", &link).unwrap();
    let object = inspect_transfer_source(&link).await.unwrap();
    let mut system = FakeMetadataSystem {
        times_errno: Some(libc::EPERM),
        ..Default::default()
    };

    let error =
        preserve_metadata(&mut system, &link, &dir.path().join("copy"), &object).unwrap_err();

    assert!(matches!(error, FileError::PreserveTimes { .. }));
}

#[tokio::test]
async fn only_user_namespace_attributes_are_copied() {
    let file = source_file().await;
    let mut system = FakeMetadataSystem {
        source_attributes: vec![
            (OsString::from("user.xdg.origin.url"), b"https://a".to_vec()),
            (OsString::from("security.selinux"), b"label".to_vec()),
            (OsString::from("security.capability"), b"cap".to_vec()),
            (OsString::from("trusted.overlay"), b"x".to_vec()),
            (OsString::from("system.posix_acl_access"), b"acl".to_vec()),
        ],
        ..Default::default()
    };

    let lost = preserve(&mut system, &file).unwrap();

    assert!(lost.is_empty());
    assert_eq!(
        system.written_attributes,
        vec![OsString::from("user.xdg.origin.url")]
    );
}

#[tokio::test]
async fn unsupported_source_attributes_and_acl_are_not_losses() {
    let file = source_file().await;
    let mut system = FakeMetadataSystem {
        attribute_list_errno: Some(libc::EOPNOTSUPP),
        acl_read_errno: Some(libc::ENOTSUP),
        ..Default::default()
    };

    let lost = preserve(&mut system, &file).unwrap();

    assert!(lost.is_empty());
    assert!(!system.acl_written);
}

#[tokio::test]
async fn minimal_acl_is_not_written_because_mode_already_carries_it() {
    let file = source_file().await;
    let mut system = FakeMetadataSystem {
        // 目标不支持 ACL;最小 ACL 不应触发写入,因此也不应报告丢失。
        acl_write_errno: Some(libc::EOPNOTSUPP),
        ..Default::default()
    };

    let lost = preserve(&mut system, &file).unwrap();

    assert!(lost.is_empty());
    assert!(!system.acl_written);
}

#[tokio::test]
async fn extended_acl_is_written() {
    let file = source_file().await;
    let mut system = FakeMetadataSystem {
        source_acl: Some(extended_acl()),
        ..Default::default()
    };

    let lost = preserve(&mut system, &file).unwrap();

    assert!(lost.is_empty());
    assert!(system.acl_written);
}

#[tokio::test]
async fn failed_writes_are_reported_by_kind() {
    let file = source_file().await;
    let mut system = FakeMetadataSystem {
        source_attributes: vec![(OsString::from("user.tag"), b"v".to_vec())],
        attribute_write_errno: Some(libc::EOPNOTSUPP),
        permissions_errno: Some(libc::EPERM),
        source_acl: Some(extended_acl()),
        acl_write_errno: Some(libc::EOPNOTSUPP),
        ..Default::default()
    };

    let lost = preserve(&mut system, &file).unwrap();

    assert_eq!(
        lost,
        BTreeSet::from([
            MetadataKind::Permissions,
            MetadataKind::ExtendedAttributes,
            MetadataKind::AccessControlList,
        ])
    );
}

#[tokio::test]
async fn unreadable_source_attributes_are_losses() {
    let file = source_file().await;
    let mut system = FakeMetadataSystem {
        attribute_list_errno: Some(libc::EACCES),
        acl_read_errno: Some(libc::EACCES),
        ..Default::default()
    };

    let lost = preserve(&mut system, &file).unwrap();

    assert_eq!(
        lost,
        BTreeSet::from([
            MetadataKind::ExtendedAttributes,
            MetadataKind::AccessControlList,
        ])
    );
}

#[tokio::test]
async fn vanished_source_reports_unreadable_kinds_and_keeps_content() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source.txt");
    let target = directory.path().join("target.txt");
    std::fs::write(&source, b"payload").unwrap();
    std::fs::write(&target, b"payload").unwrap();
    let source_object = inspect_transfer_source(&source).await.unwrap();
    std::fs::remove_file(&source).unwrap();

    let lost = apply_transfer_metadata(&source, &target, &source_object)
        .await
        .unwrap();

    assert_eq!(
        lost,
        BTreeSet::from([
            MetadataKind::ExtendedAttributes,
            MetadataKind::AccessControlList,
        ])
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"payload");
}
