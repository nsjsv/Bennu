use std::path::Path;

use super::RecoverableTransferError;

pub fn sync_tree_blocking(root: &Path) -> Result<(), RecoverableTransferError> {
    // syncfs 一次刷整棵树所在文件系统:逐文件 open+fsync+close 在 2 万文件
    // 的树上要串行数万次系统调用(btrfs 上每次 0.2–1ms,是并行 staging 的
    // 主要瓶颈);syncfs 的持久化保证只强不弱(连带刷掉同盘其它脏页),
    // 成本是一次全量回写等待。
    let file = std::fs::File::open(root).map_err(|source| {
        RecoverableTransferError::file_system("open staged tree for sync", root, source)
    })?;
    let result = unsafe { libc::syncfs(std::os::unix::io::AsRawFd::as_raw_fd(&file)) };
    if result != 0 {
        let source = std::io::Error::last_os_error();
        return Err(RecoverableTransferError::file_system(
            "sync staged tree",
            root,
            source,
        ));
    }
    Ok(())
}

pub fn sync_parent_blocking(path: &Path) -> Result<(), RecoverableTransferError> {
    let parent = path
        .parent()
        .ok_or_else(|| RecoverableTransferError::ArtifactOwnership {
            path: path.to_path_buf(),
            reason: "path has no parent directory to sync".to_owned(),
        })?;
    sync_open_path(parent, "sync parent directory")
}

fn sync_open_path(path: &Path, action: &'static str) -> Result<(), RecoverableTransferError> {
    std::fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|source| RecoverableTransferError::file_system(action, path, source))
}
