use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::Path;

use filetime::FileTime;

use super::metadata_loss::MetadataKind;
use super::transfer_object::{TransferSourceKind, TransferSourceObject};
use crate::FileError;

/// 把源对象的元数据恢复到目标上,返回没能保留的种类(空集合表示全部保留)。
///
/// 时间单独作为错误返回:可恢复传输按 mtime 快照校验(identity.rs 的
/// `matches_staging_snapshot`),时间没恢复必然在后续校验里被误报成"源已变化",
/// 所以必须在这里报出真实原因。
pub(super) async fn apply_transfer_metadata(
    source: &Path,
    target: &Path,
    source_object: &TransferSourceObject,
) -> Result<BTreeSet<MetadataKind>, FileError> {
    let source = source.to_path_buf();
    let target = target.to_path_buf();
    let source_object = source_object.clone();
    let restoration = tokio::task::spawn_blocking(move || {
        preserve_metadata(&mut HostMetadataSystem, &source, &target, &source_object)
    })
    .await;

    match restoration {
        Ok(result) => result,
        Err(join_error) if join_error.is_panic() => {
            std::panic::resume_unwind(join_error.into_panic())
        }
        // 阻塞任务只会在运行时关闭时被取消。
        Err(_) => Err(FileError::ApplicationStopping),
    }
}

/// 摘要只记录"哪类没保留"(用户确定的提示格式),具体系统错误不进入摘要。
fn preserve_metadata(
    system: &mut impl MetadataSystem,
    source: &Path,
    target: &Path,
    source_object: &TransferSourceObject,
) -> Result<BTreeSet<MetadataKind>, FileError> {
    let mut lost = BTreeSet::new();
    let accessed = FileTime::from_last_access_time(&source_object.metadata);
    let modified = FileTime::from_last_modification_time(&source_object.metadata);

    // 顺序遵循文件传输保真规范:xattr 先于 mode(只读 mode 过早落到目标会挡住
    // user.* 写入),ACL 在 mode 之后以保持源 ACL 的 mask 语义,时间最后写。
    if copy_user_extended_attributes(system, source, target).is_err() {
        lost.insert(MetadataKind::ExtendedAttributes);
    }
    let times = match source_object.kind {
        TransferSourceKind::RegularFile | TransferSourceKind::Directory => {
            if system
                .set_permissions(target, source_object.metadata.permissions())
                .is_err()
            {
                lost.insert(MetadataKind::Permissions);
            }
            if copy_extended_access_control_list(system, source, target).is_err() {
                lost.insert(MetadataKind::AccessControlList);
            }
            system.set_times(target, accessed, modified)
        }
        // Linux 不支持符号链接的 mode 和 ACL,只恢复 xattr 和链接自身的时间。
        TransferSourceKind::SymbolicLink { .. } => {
            system.set_symbolic_link_times(target, accessed, modified)
        }
    };
    times.map_err(|source| FileError::PreserveTimes {
        path: target.to_path_buf(),
        source,
    })?;

    Ok(lost)
}

/// 只复制 `user.*`:`security.selinux` 应由目标位置的策略决定标签,
/// `security.capability`、`trusted.*` 普通用户本来就写不进去;
/// `system.posix_acl_*` 由 ACL 路径单独处理。
fn copy_user_extended_attributes(
    system: &mut impl MetadataSystem,
    source: &Path,
    target: &Path,
) -> io::Result<()> {
    let names = match system.extended_attribute_names(source) {
        Ok(names) => names,
        // 源文件系统不支持 xattr:源上本来就没有,无可保留。
        Err(error) if is_unsupported(&error) => return Ok(()),
        Err(error) => return Err(error),
    };

    let mut first_failure = None;
    for name in names.iter().filter(|name| is_user_namespace(name)) {
        let copied = system
            .extended_attribute(source, name)
            .and_then(|value| match value {
                Some(value) => system.set_extended_attribute(target, name, &value),
                // 列出之后又被删掉的属性:源上已经没有,无可保留。
                None => Ok(()),
            });
        if let Err(error) = copied {
            first_failure.get_or_insert(error);
        }
    }
    first_failure.map_or(Ok(()), Err)
}

fn copy_extended_access_control_list(
    system: &mut impl MetadataSystem,
    source: &Path,
    target: &Path,
) -> io::Result<()> {
    let acl = match system.access_control_list(source) {
        Ok(acl) => acl,
        // 源文件系统不支持 ACL:源上本来就没有,无可保留。
        Err(error) if is_unsupported(&error) => return Ok(()),
        Err(error) => return Err(error),
    };
    // 只有属主/属组/其他三条的最小 ACL 与权限位等价,权限位已经单独恢复;
    // 再写一遍只会在不支持 ACL 的目标(例如 exFAT U 盘)上制造误报。
    if !acl.is_extended() {
        return Ok(());
    }
    system.set_access_control_list(target, &acl)
}

fn is_user_namespace(name: &OsStr) -> bool {
    name.as_encoded_bytes().starts_with(b"user.")
}

fn is_unsupported(error: &io::Error) -> bool {
    #[cfg(unix)]
    if matches!(error.raw_os_error(), Some(code) if code == libc::ENOTSUP || code == libc::EOPNOTSUPP)
    {
        return true;
    }
    error.kind() == io::ErrorKind::Unsupported
}

/// 元数据读写的外部依赖适配层(xattr / exacl / filetime / std::fs)。单独抽出,
/// 是为了让测试能确定性地制造这些系统调用的失败;正式与测试共用
/// `preserve_metadata` 的全部分类逻辑。
trait MetadataSystem {
    fn extended_attribute_names(&mut self, path: &Path) -> io::Result<Vec<OsString>>;
    fn extended_attribute(&mut self, path: &Path, name: &OsStr) -> io::Result<Option<Vec<u8>>>;
    fn set_extended_attribute(&mut self, path: &Path, name: &OsStr, value: &[u8])
        -> io::Result<()>;
    fn set_permissions(&mut self, path: &Path, permissions: std::fs::Permissions)
        -> io::Result<()>;
    fn access_control_list(&mut self, path: &Path) -> io::Result<AccessControlList>;
    fn set_access_control_list(&mut self, path: &Path, acl: &AccessControlList) -> io::Result<()>;
    fn set_times(&mut self, path: &Path, accessed: FileTime, modified: FileTime) -> io::Result<()>;
    fn set_symbolic_link_times(
        &mut self,
        path: &Path,
        accessed: FileTime,
        modified: FileTime,
    ) -> io::Result<()>;
}

#[cfg(target_os = "linux")]
struct AccessControlList(Vec<exacl::AclEntry>);

#[cfg(target_os = "linux")]
impl AccessControlList {
    /// 有具名用户/组条目或目录默认 ACL 时,ACL 才携带权限位之外的信息。
    fn is_extended(&self) -> bool {
        self.0
            .iter()
            .any(|entry| !entry.name.is_empty() || entry.flags.contains(exacl::Flag::DEFAULT))
    }
}

#[cfg(not(target_os = "linux"))]
struct AccessControlList;

#[cfg(not(target_os = "linux"))]
impl AccessControlList {
    fn is_extended(&self) -> bool {
        false
    }
}

struct HostMetadataSystem;

impl MetadataSystem for HostMetadataSystem {
    #[cfg(unix)]
    fn extended_attribute_names(&mut self, path: &Path) -> io::Result<Vec<OsString>> {
        Ok(xattr::list(path)?.collect())
    }

    #[cfg(not(unix))]
    fn extended_attribute_names(&mut self, _path: &Path) -> io::Result<Vec<OsString>> {
        Err(io::ErrorKind::Unsupported.into())
    }

    #[cfg(unix)]
    fn extended_attribute(&mut self, path: &Path, name: &OsStr) -> io::Result<Option<Vec<u8>>> {
        xattr::get(path, name)
    }

    #[cfg(not(unix))]
    fn extended_attribute(&mut self, _path: &Path, _name: &OsStr) -> io::Result<Option<Vec<u8>>> {
        Err(io::ErrorKind::Unsupported.into())
    }

    #[cfg(unix)]
    fn set_extended_attribute(
        &mut self,
        path: &Path,
        name: &OsStr,
        value: &[u8],
    ) -> io::Result<()> {
        xattr::set(path, name, value)
    }

    #[cfg(not(unix))]
    fn set_extended_attribute(
        &mut self,
        _path: &Path,
        _name: &OsStr,
        _value: &[u8],
    ) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }

    fn set_permissions(
        &mut self,
        path: &Path,
        permissions: std::fs::Permissions,
    ) -> io::Result<()> {
        std::fs::set_permissions(path, permissions)
    }

    #[cfg(target_os = "linux")]
    fn access_control_list(&mut self, path: &Path) -> io::Result<AccessControlList> {
        exacl::getfacl(path, None).map(AccessControlList)
    }

    #[cfg(not(target_os = "linux"))]
    fn access_control_list(&mut self, _path: &Path) -> io::Result<AccessControlList> {
        Err(io::ErrorKind::Unsupported.into())
    }

    #[cfg(target_os = "linux")]
    fn set_access_control_list(&mut self, path: &Path, acl: &AccessControlList) -> io::Result<()> {
        exacl::setfacl(&[path], &acl.0, None)
    }

    #[cfg(not(target_os = "linux"))]
    fn set_access_control_list(
        &mut self,
        _path: &Path,
        _acl: &AccessControlList,
    ) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }

    fn set_times(&mut self, path: &Path, accessed: FileTime, modified: FileTime) -> io::Result<()> {
        filetime::set_file_times(path, accessed, modified)
    }

    fn set_symbolic_link_times(
        &mut self,
        path: &Path,
        accessed: FileTime,
        modified: FileTime,
    ) -> io::Result<()> {
        filetime::set_symlink_file_times(path, accessed, modified)
    }
}

// 假适配层直接构造 exacl 条目,exacl 只在 Linux 上可用。
#[cfg(all(test, target_os = "linux"))]
#[path = "transfer_metadata_tests.rs"]
mod tests;
