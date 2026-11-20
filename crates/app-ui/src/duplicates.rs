//! 找重复的文件：内容相同的文件每组留修改时间最早的一份，其余的交回去（调用方选中它们，用户看过再
//! 删）。先按大小分组，只有同样大小的才读内容算 BLAKE3。硬链接到同一份的只算一个：删了它不省地方。
//! 会读盘，只在后台线程调用。

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::time::SystemTime;

struct Copy {
    path: PathBuf,
    modified: SystemTime,
}

/// 多出来的副本，按路径排好。文件夹、符号链接、空文件、读不了的跳过。
pub(crate) fn redundant_copies(files: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut by_size: HashMap<u64, HashMap<(u64, u64), Copy>> = HashMap::new();
    for path in files {
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.is_file() || metadata.len() == 0 {
            continue;
        }
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        by_size
            .entry(metadata.len())
            .or_default()
            .entry((metadata.dev(), metadata.ino()))
            .or_insert(Copy { path, modified });
    }
    let mut redundant = Vec::new();
    for copies in by_size.into_values().filter(|copies| copies.len() > 1) {
        let mut by_content: HashMap<blake3::Hash, Vec<Copy>> = HashMap::new();
        for copy in copies.into_values() {
            if let Some(hash) = content_hash(&copy.path) {
                by_content.entry(hash).or_default().push(copy);
            }
        }
        for mut same in by_content.into_values().filter(|same| same.len() > 1) {
            same.sort_by(|left, right| {
                (left.modified, &left.path).cmp(&(right.modified, &right.path))
            });
            redundant.extend(same.into_iter().skip(1).map(|copy| copy.path));
        }
    }
    redundant.sort();
    redundant
}

fn content_hash(path: &std::path::Path) -> Option<blake3::Hash> {
    let file = std::fs::File::open(path).ok()?;
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(file).ok()?;
    Some(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &std::path::Path, content: &[u8], seconds_ago: u64) {
        std::fs::write(path, content).unwrap();
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() - std::time::Duration::from_secs(seconds_ago))
            .unwrap();
    }

    #[test]
    fn copies_of_the_same_content_but_the_oldest_are_redundant() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(&root.join("原件.jpg"), b"same", 300);
        write(&root.join("副本.jpg"), b"same", 100);
        write(&root.join("又一份.jpg"), b"same", 200);
        write(&root.join("同大小.jpg"), b"diff", 400);
        write(&root.join("别的.jpg"), b"longer", 500);
        write(&root.join("空的"), b"", 500);
        write(&root.join("也空.txt"), b"", 500);
        std::fs::hard_link(root.join("原件.jpg"), root.join("硬链接.jpg")).unwrap();
        std::os::unix::fs::symlink(root.join("原件.jpg"), root.join("链接.jpg")).unwrap();
        std::fs::create_dir(root.join("文件夹")).unwrap();

        let files = std::fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        let redundant = redundant_copies(files);

        assert_eq!(redundant, [root.join("副本.jpg"), root.join("又一份.jpg")]);
    }
}
