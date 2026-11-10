//! 接收确认里的清单：别人发来的文件夹合成一行（名字 · 几个文件 · 多大），单独的文件
//! 一行一个。fileName 带相对路径时首段就是文件夹名（同官方发文件夹的写法）。

use std::collections::BTreeMap;

use local_send::FileMetadata;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IncomingEntry {
    Folder {
        name: String,
        files: usize,
        bytes: u64,
    },
    File {
        name: String,
        bytes: u64,
    },
}

/// 按名字排好（请求里的文件是无序的）。
pub(crate) fn incoming_entries(files: &[FileMetadata]) -> Vec<IncomingEntry> {
    let mut folders: BTreeMap<&str, (usize, u64)> = BTreeMap::new();
    let mut singles = Vec::new();
    for file in files {
        let name = file.file_name.trim_start_matches(['/', '\\']);
        match name.split_once(['/', '\\']) {
            Some((folder, _)) => {
                let entry = folders.entry(folder).or_default();
                entry.0 += 1;
                entry.1 = entry.1.saturating_add(file.size);
            }
            None => singles.push(IncomingEntry::File {
                name: name.to_owned(),
                bytes: file.size,
            }),
        }
    }
    singles.sort_by(|a, b| entry_name(a).cmp(entry_name(b)));
    folders
        .into_iter()
        .map(|(name, (files, bytes))| IncomingEntry::Folder {
            name: name.to_owned(),
            files,
            bytes,
        })
        .chain(singles)
        .collect()
}

fn entry_name(entry: &IncomingEntry) -> &str {
    match entry {
        IncomingEntry::Folder { name, .. } | IncomingEntry::File { name, .. } => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, size: u64) -> FileMetadata {
        FileMetadata {
            id: name.to_owned(),
            file_name: name.to_owned(),
            size,
            file_type: None,
            sha256: None,
            preview: None,
        }
    }

    #[test]
    fn folders_collapse_into_one_line_and_single_files_stay_apart() {
        let entries = incoming_entries(&[
            file("说明.txt", 3),
            file("照片/2024/a.jpg", 10),
            file("照片/b.jpg", 20),
            file("a.txt", 1),
        ]);

        assert_eq!(
            entries,
            [
                IncomingEntry::Folder {
                    name: "照片".to_owned(),
                    files: 2,
                    bytes: 30
                },
                IncomingEntry::File {
                    name: "a.txt".to_owned(),
                    bytes: 1
                },
                IncomingEntry::File {
                    name: "说明.txt".to_owned(),
                    bytes: 3
                },
            ]
        );
    }
}
