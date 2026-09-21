use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum NoReplaceRenameError {
    TargetExists,
    CrossDevice,
    Unsupported(io::Error),
    Failed(io::Error),
}

impl NoReplaceRenameError {
    pub fn into_io_error(self) -> io::Error {
        match self {
            Self::TargetExists => io::Error::new(io::ErrorKind::AlreadyExists, "target exists"),
            Self::CrossDevice => io::Error::new(
                io::ErrorKind::CrossesDevices,
                "source and target are on different filesystems",
            ),
            Self::Unsupported(source) | Self::Failed(source) => source,
        }
    }

    pub fn into_transfer_error(self, from: &Path, to: &Path) -> super::RecoverableTransferError {
        let source = self.into_io_error();
        super::RecoverableTransferError::SafeRename {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
            source,
        }
    }
}

#[cfg(target_os = "linux")]
pub fn rename_noreplace(from: &Path, to: &Path) -> Result<(), NoReplaceRenameError> {
    use rustix::fs::{renameat_with, RenameFlags, CWD};
    use rustix::io::Errno;

    renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE).map_err(|error| match error {
        Errno::EXIST => NoReplaceRenameError::TargetExists,
        Errno::XDEV => NoReplaceRenameError::CrossDevice,
        Errno::NOSYS | Errno::INVAL | Errno::OPNOTSUPP => {
            NoReplaceRenameError::Unsupported(error.into())
        }
        _ => NoReplaceRenameError::Failed(error.into()),
    })
}

#[cfg(not(target_os = "linux"))]
pub fn rename_noreplace(_from: &Path, _to: &Path) -> Result<(), NoReplaceRenameError> {
    Err(NoReplaceRenameError::Unsupported(io::Error::new(
        io::ErrorKind::Unsupported,
        "safe no-replace rename requires Linux",
    )))
}

pub fn recovered_name_candidate(path: &Path, sequence: u64) -> PathBuf {
    let mut name = path
        .file_name()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| "recovered".into());
    name.push(format!(".recovered{sequence}"));
    path.with_file_name(name)
}
