use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

use super::super::FileOperationControls;
use super::RecoverableTransferError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ObjectFingerprint(pub [u8; 32]);

pub async fn fingerprint_object(
    root: &Path,
) -> Result<ObjectFingerprint, RecoverableTransferError> {
    fingerprint_object_with_checkpoint(root, || Ok(())).await
}

pub async fn fingerprint_object_with_controls(
    root: &Path,
    controls: &FileOperationControls,
) -> Result<ObjectFingerprint, RecoverableTransferError> {
    let controls = controls.clone();
    fingerprint_object_with_checkpoint(root, move || controls.checkpoint_now().map_err(Into::into))
        .await
}

async fn fingerprint_object_with_checkpoint<C>(
    root: &Path,
    checkpoint: C,
) -> Result<ObjectFingerprint, RecoverableTransferError>
where
    C: FnMut() -> Result<(), RecoverableTransferError> + Send + 'static,
{
    let root = root.to_path_buf();
    let error_path = root.clone();
    tokio::task::spawn_blocking(move || fingerprint_object_blocking(&root, checkpoint))
        .await
        .map_err(|join_error| {
            RecoverableTransferError::file_system(
                "join object fingerprint task for",
                &error_path,
                std::io::Error::other(join_error),
            )
        })?
}

fn fingerprint_object_blocking<C>(
    root: &Path,
    mut checkpoint: C,
) -> Result<ObjectFingerprint, RecoverableTransferError>
where
    C: FnMut() -> Result<(), RecoverableTransferError>,
{
    // 两阶段指纹:与旧实现逐字节同序、同值。
    // 阶段一:与旧算法完全相同的 DFS(子项按名排序)顺序收集条目元数据,
    //   不读内容;
    // 阶段二:哈希流严格按阶段一的顺序消费,文件内容由 worker 池并行预读
    //   (有界窗口),大文件由消费者直接顺序读。并行只影响读的时机,
    //   不影响喂入哈希器的字节序列。

    struct FingerprintEntry {
        relative_path: PathBuf,
        path: PathBuf,
        kind: FingerprintEntryKind,
    }

    enum FingerprintEntryKind {
        File { length: u64 },
        SymbolicLink { target: PathBuf },
        Directory,
    }

    let mut entries: Vec<FingerprintEntry> = Vec::new();
    let mut pending = vec![(PathBuf::new(), root.to_path_buf())];

    while let Some((relative_path, path)) = pending.pop() {
        checkpoint()?;
        let metadata = std::fs::symlink_metadata(&path).map_err(|source| {
            RecoverableTransferError::file_system("read fingerprint metadata for", &path, source)
        })?;
        let file_type = metadata.file_type();
        if file_type.is_file() {
            entries.push(FingerprintEntry {
                relative_path,
                path,
                kind: FingerprintEntryKind::File {
                    length: metadata.len(),
                },
            });
            continue;
        }
        if file_type.is_symlink() {
            let target = std::fs::read_link(&path).map_err(|source| {
                RecoverableTransferError::file_system(
                    "read fingerprint symbolic link",
                    &path,
                    source,
                )
            })?;
            entries.push(FingerprintEntry {
                relative_path,
                path,
                kind: FingerprintEntryKind::SymbolicLink { target },
            });
            continue;
        }
        if !file_type.is_dir() {
            return Err(RecoverableTransferError::UnsupportedObject { path });
        }
        entries.push(FingerprintEntry {
            relative_path: relative_path.clone(),
            path: path.clone(),
            kind: FingerprintEntryKind::Directory,
        });
        let dir_entries = std::fs::read_dir(&path).map_err(|source| {
            RecoverableTransferError::file_system("read fingerprint directory", &path, source)
        })?;
        let mut children = Vec::new();
        for entry in dir_entries {
            checkpoint()?;
            let entry = entry.map_err(|source| {
                RecoverableTransferError::file_system(
                    "read fingerprint directory entry",
                    &path,
                    source,
                )
            })?;
            children.push((entry.file_name(), entry.path()));
        }
        children.sort_by(|(left, _), (right, _)| left.cmp(right));
        for (name, child_path) in children.into_iter().rev() {
            pending.push((relative_path.join(name), child_path));
        }
    }

    // 阶段二:有序消费 + 并行预读。
    const PREFETCH_WORKERS: usize = 4;
    const PREFETCH_WINDOW: usize = 32;
    // 超过该大小的文件不预读(避免窗口内存爆炸),由消费者顺序直读。
    const PREFETCH_MAX_BYTES: u64 = 32 * 1024 * 1024;

    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0; 1024 * 1024];

    let entries = Arc::new(entries);
    let prefetched_indices: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            matches!(entry.kind, FingerprintEntryKind::File { length } if length <= PREFETCH_MAX_BYTES)
        })
        .map(|(index, _)| index)
        .collect();
    let prefetched_indices = Arc::new(prefetched_indices);
    let prefetch_count = prefetched_indices.len();
    let is_prefetched: std::collections::HashSet<usize> =
        prefetched_indices.iter().copied().collect();

    if prefetch_count > 0 {
        enum PrefetchResult {
            Content(usize, Vec<u8>),
            Failed(usize, std::io::Error),
        }

        let next_claim = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let consumed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let consumer_done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (result_tx, result_rx) = std::sync::mpsc::channel::<PrefetchResult>();
        let result_rx = Arc::new(std::sync::Mutex::new(result_rx));
        let mut workers = Vec::with_capacity(PREFETCH_WORKERS);
        for _ in 0..PREFETCH_WORKERS {
            let next_claim = next_claim.clone();
            let consumed = consumed.clone();
            let consumer_done = consumer_done.clone();
            let result_tx = result_tx.clone();
            let entries = entries.clone();
            let prefetched_indices = prefetched_indices.clone();
            workers.push(std::thread::spawn(move || loop {
                let claim = next_claim.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if claim >= prefetch_count {
                    return;
                }
                // 有界窗口:领先消费者太多时等待,防止内存无界增长。
                loop {
                    if consumer_done.load(std::sync::atomic::Ordering::SeqCst) {
                        return;
                    }
                    let consumed_index = consumed.load(std::sync::atomic::Ordering::SeqCst);
                    if claim < consumed_index + PREFETCH_WINDOW {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_micros(200));
                }
                let entry_index = prefetched_indices[claim];
                let FingerprintEntryKind::File { .. } = entries[entry_index].kind else {
                    continue;
                };
                let read = std::fs::read(&entries[entry_index].path);
                let outcome = match read {
                    Ok(content) => PrefetchResult::Content(entry_index, content),
                    Err(error) => PrefetchResult::Failed(entry_index, error),
                };
                if result_tx.send(outcome).is_err() {
                    return;
                }
            }));
        }
        drop(result_tx);

        let mut pending_contents: std::collections::BTreeMap<usize, Vec<u8>> =
            std::collections::BTreeMap::new();
        let mut last_error: Option<std::io::Error> = None;

        for (index, entry) in entries.iter().enumerate() {
            // 提前退出必须先叫停 worker,否则取消/出错后它们会继续把剩余
            // 队列全部预读完毕。
            if let Err(error) = checkpoint() {
                consumer_done.store(true, std::sync::atomic::Ordering::SeqCst);
                return Err(error);
            }
            update_component(&mut hasher, entry.relative_path.as_os_str());
            match &entry.kind {
                FingerprintEntryKind::File { length } => {
                    hasher.update(b"file\0");
                    hasher.update(&length.to_le_bytes());
                    // 只有预读集内的小文件等 worker 结果;超过
                    // PREFETCH_MAX_BYTES 的大文件 worker 不认领,必须由
                    // 消费者顺序直读。缺了直读分支,消费到大文件时 recv
                    // 要么永远等不到(worker 被窗口门锁死,整体死锁),
                    // 要么在通道排空关闭后被误报成 join 失败。
                    let mut read_total = 0u64;
                    if is_prefetched.contains(&index) {
                        let content = loop {
                            if let Some(content) = pending_contents.remove(&index) {
                                break content;
                            }
                            if last_error.is_some() {
                                consumer_done.store(true, std::sync::atomic::Ordering::SeqCst);
                                return Err(last_error.map(|source| {
                                    RecoverableTransferError::file_system(
                                        "read fingerprint file",
                                        &entry.path,
                                        source,
                                    )
                                })
                                .unwrap());
                            }
                            let outcome = {
                                let receiver = result_rx.lock().unwrap();
                                receiver.recv().map_err(|join| {
                                    RecoverableTransferError::file_system(
                                        "join fingerprint prefetch worker",
                                        &entry.path,
                                        std::io::Error::other(join),
                                    )
                                })?
                            };
                            match outcome {
                                PrefetchResult::Content(entry_index, content) => {
                                    pending_contents.insert(entry_index, content);
                                }
                                PrefetchResult::Failed(entry_index, error) => {
                                    pending_contents.insert(entry_index, Vec::new());
                                    last_error = Some(error);
                                }
                            }
                        };
                        hasher.update(&content);
                        read_total = content.len() as u64;
                    } else {
                        // blake3 流式 update 与整块 update 同值:直读不改变指纹。
                        let mut file = std::fs::File::open(&entry.path).map_err(|source| {
                            consumer_done.store(true, std::sync::atomic::Ordering::SeqCst);
                            RecoverableTransferError::file_system(
                                "open fingerprint file",
                                &entry.path,
                                source,
                            )
                        })?;
                        loop {
                            checkpoint()?;
                            let read = file.read(&mut buffer).map_err(|source| {
                                consumer_done.store(true, std::sync::atomic::Ordering::SeqCst);
                                RecoverableTransferError::file_system(
                                    "read fingerprint file",
                                    &entry.path,
                                    source,
                                )
                            })?;
                            if read == 0 {
                                break;
                            }
                            read_total += read as u64;
                            hasher.update(&buffer[..read]);
                        }
                    }
                    if read_total != *length {
                        consumer_done.store(true, std::sync::atomic::Ordering::SeqCst);
                        return Err(RecoverableTransferError::file_system(
                            "read fingerprint file",
                            &entry.path,
                            std::io::Error::new(
                                std::io::ErrorKind::UnexpectedEof,
                                "file size changed during fingerprint",
                            ),
                        ));
                    }
                    consumed.store(index + 1, std::sync::atomic::Ordering::SeqCst);
                }
                FingerprintEntryKind::SymbolicLink { target } => {
                    hasher.update(b"symlink\0");
                    update_component(&mut hasher, target.as_os_str());
                    consumed.store(index + 1, std::sync::atomic::Ordering::SeqCst);
                }
                FingerprintEntryKind::Directory => {
                    hasher.update(b"directory\0");
                    consumed.store(index + 1, std::sync::atomic::Ordering::SeqCst);
                }
            }
        }
        consumer_done.store(true, std::sync::atomic::Ordering::SeqCst);
        for worker in workers {
            let _ = worker.join();
        }
        if let Some(source) = last_error {
            return Err(RecoverableTransferError::file_system(
                "read fingerprint file",
                root,
                source,
            ));
        }
    } else {
        for entry in entries.iter() {
            checkpoint()?;
            update_component(&mut hasher, entry.relative_path.as_os_str());
            match &entry.kind {
                FingerprintEntryKind::File { length } => {
                    hasher.update(b"file\0");
                    hasher.update(&length.to_le_bytes());
                    let mut file = std::fs::File::open(&entry.path).map_err(|source| {
                        RecoverableTransferError::file_system(
                            "open fingerprint file",
                            &entry.path,
                            source,
                        )
                    })?;
                    loop {
                        checkpoint()?;
                        let read = file.read(&mut buffer).map_err(|source| {
                            RecoverableTransferError::file_system(
                                "read fingerprint file",
                                &entry.path,
                                source,
                            )
                        })?;
                        if read == 0 {
                            break;
                        }
                        hasher.update(&buffer[..read]);
                    }
                }
                FingerprintEntryKind::SymbolicLink { target } => {
                    hasher.update(b"symlink\0");
                    update_component(&mut hasher, target.as_os_str());
                }
                FingerprintEntryKind::Directory => {
                    hasher.update(b"directory\0");
                }
            }
        }
    }

    Ok(ObjectFingerprint(*hasher.finalize().as_bytes()))
}

fn update_component(hasher: &mut blake3::Hasher, value: &OsStr) {
    #[cfg(unix)]
    {
        let bytes = value.as_bytes();
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    }
    #[cfg(not(unix))]
    {
        let encoded = value.to_string_lossy();
        let bytes = encoded.as_bytes();
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::FileOperationRunState;
    use tokio::sync::watch;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn controlled_fingerprint_stops_before_reading_content() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("content.bin");
        std::fs::write(&path, vec![7_u8; 1024 * 1024]).unwrap();
        let (_sender, receiver) = watch::channel(FileOperationRunState::ApplicationStopping);
        let controls = FileOperationControls::new(CancellationToken::new(), receiver);

        let error = fingerprint_object_with_controls(&path, &controls)
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            RecoverableTransferError::FileOperation(crate::FileError::ApplicationStopping)
        ));
    }

    // ===== 大文件直读分支的不变量测试 =====
    // 参考实现:与阶段一相同的排序单线程顺序遍历,锁定"并行只影响读的
    // 时机,不影响哈希值"。
    fn reference_fingerprint(root: &Path) -> ObjectFingerprint {
        enum Kind {
            File { length: u64 },
            Symlink { target: PathBuf },
            Directory,
        }
        struct Entry {
            rel: PathBuf,
            path: PathBuf,
            kind: Kind,
        }
        let mut entries: Vec<Entry> = Vec::new();
        let mut pending = vec![(PathBuf::new(), root.to_path_buf())];
        while let Some((rel, path)) = pending.pop() {
            let metadata = std::fs::symlink_metadata(&path).unwrap();
            let file_type = metadata.file_type();
            if file_type.is_file() {
                entries.push(Entry { rel, path, kind: Kind::File { length: metadata.len() } });
                continue;
            }
            if file_type.is_symlink() {
                let target = std::fs::read_link(&path).unwrap();
                entries.push(Entry { rel, path, kind: Kind::Symlink { target } });
                continue;
            }
            entries.push(Entry { rel: rel.clone(), path: path.clone(), kind: Kind::Directory });
            let mut children = Vec::new();
            for entry in std::fs::read_dir(&path).unwrap() {
                let entry = entry.unwrap();
                children.push((entry.file_name(), entry.path()));
            }
            children.sort_by(|(left, _), (right, _)| left.cmp(right));
            for (name, child_path) in children.into_iter().rev() {
                pending.push((rel.join(name), child_path));
            }
        }
        let mut hasher = blake3::Hasher::new();
        for entry in &entries {
            update_component(&mut hasher, entry.rel.as_os_str());
            match &entry.kind {
                Kind::File { length } => {
                    hasher.update(b"file\0");
                    hasher.update(&length.to_le_bytes());
                    hasher.update(&std::fs::read(&entry.path).unwrap());
                }
                Kind::Symlink { target } => {
                    hasher.update(b"symlink\0");
                    update_component(&mut hasher, target.as_os_str());
                }
                Kind::Directory => {
                    hasher.update(b"directory\0");
                }
            }
        }
        ObjectFingerprint(*hasher.finalize().as_bytes())
    }

    /// mixed:大文件 + 一批小文件 + 子目录。`large_first` 决定大文件在
    /// 排序遍历中的位置——居首是旧实现的死锁形态(≥33 个小文件顶满
    /// 预读窗口,消费者与 worker 互相等待),居末是旧的误报形态
    /// (worker 排空通道退出,消费者 recv 报 join 失败)。
    fn write_mixed_tree(root: &Path, large_first: bool) {
        const LARGE: usize = 32 * 1024 * 1024 + 7;
        let large = vec![0xA5_u8; LARGE];
        std::fs::create_dir_all(root.join("subdir")).unwrap();
        if large_first {
            std::fs::write(root.join("aaa-large.bin"), &large).unwrap();
        } else {
            std::fs::write(root.join("aaa-small.txt"), b"small").unwrap();
        }
        for index in 0..40 {
            std::fs::write(root.join(format!("n{index:02}.txt")), format!("content-{index}"))
                .unwrap();
        }
        std::fs::write(root.join("subdir").join("nested-large.bin"), &large).unwrap();
        if large_first {
            std::fs::write(root.join("z-small.txt"), b"small").unwrap();
        } else {
            std::fs::write(root.join("zzz-large.bin"), &large).unwrap();
        }
    }

    async fn fingerprint_with_deadline(root: &Path) -> ObjectFingerprint {
        tokio::time::timeout(std::time::Duration::from_secs(60), fingerprint_object(root))
            .await
            .expect("fingerprint deadlocked on a tree containing large files")
            .unwrap()
    }

    #[tokio::test]
    async fn large_file_first_does_not_deadlock_the_prefetch_window() {
        let directory = tempfile::tempdir().unwrap();
        write_mixed_tree(directory.path(), true);
        let fingerprint = fingerprint_with_deadline(directory.path()).await;
        assert_eq!(fingerprint, reference_fingerprint(directory.path()));
    }

    #[tokio::test]
    async fn large_file_last_is_not_misreported_as_worker_join_failure() {
        let directory = tempfile::tempdir().unwrap();
        write_mixed_tree(directory.path(), false);
        let fingerprint = fingerprint_with_deadline(directory.path()).await;
        assert_eq!(fingerprint, reference_fingerprint(directory.path()));
    }
}
