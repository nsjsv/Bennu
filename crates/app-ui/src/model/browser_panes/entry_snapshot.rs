//! 一个目录的条目：共享不复制，能按路径直接找到条目。

use std::fmt;
use std::hash::BuildHasher;
use std::ops::Deref;
use std::path::Path;
use std::sync::{Arc, LazyLock, OnceLock};

use file_core::DirectoryEntry;
use hashbrown::{DefaultHashBuilder, HashTable};

/// 条目（显示顺序）。克隆只增引用计数；改动走 `make_mut`，还有别的持有者时先复制一份。
///
/// 按路径找条目的索引第一次用到时才建，只存位置（8 万项约 0.5 MB）。`make_mut` 一律换掉它：增删、换序、
/// 原地改路径（重命名迁移）之后索引不会和条目对不上。
#[derive(Clone, Default)]
pub(crate) struct DirectoryEntrySnapshot {
    entries: Arc<Vec<DirectoryEntry>>,
    positions: Arc<OnceLock<HashTable<u32>>>,
}

impl DirectoryEntrySnapshot {
    /// 路径对应的条目。
    pub(crate) fn find(&self, path: &Path) -> Option<&DirectoryEntry> {
        let positions = self
            .positions
            .get_or_init(|| index_positions(&self.entries));
        positions
            .find(path_hash(path), |&position| {
                self.entries[position as usize].path() == path
            })
            .map(|&position| &self.entries[position as usize])
    }

    /// 改条目：还有别的持有者时先复制一份；按路径找的索引作废。
    pub(crate) fn make_mut(&mut self) -> &mut Vec<DirectoryEntry> {
        self.positions = Arc::default();
        Arc::make_mut(&mut self.entries)
    }

    /// 与 `other` 是不是同一份条目（没有复制过）。
    #[cfg(test)]
    pub(crate) fn shares_entries_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.entries, &other.entries)
    }
}

/// 只在本进程内用，种子随机即可，建索引和查找用同一个。
static PATH_HASHER: LazyLock<DefaultHashBuilder> = LazyLock::new(DefaultHashBuilder::default);

fn path_hash(path: &Path) -> u64 {
    PATH_HASHER.hash_one(path)
}

fn index_positions(entries: &[DirectoryEntry]) -> HashTable<u32> {
    let mut positions = HashTable::with_capacity(entries.len());
    for (position, entry) in entries.iter().enumerate() {
        positions.insert_unique(path_hash(entry.path()), position as u32, |&other| {
            path_hash(entries[other as usize].path())
        });
    }
    positions
}

impl Deref for DirectoryEntrySnapshot {
    type Target = Vec<DirectoryEntry>;

    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}

impl From<Vec<DirectoryEntry>> for DirectoryEntrySnapshot {
    fn from(entries: Vec<DirectoryEntry>) -> Self {
        Self {
            entries: Arc::new(entries),
            positions: Arc::default(),
        }
    }
}

impl FromIterator<DirectoryEntry> for DirectoryEntrySnapshot {
    fn from_iter<I: IntoIterator<Item = DirectoryEntry>>(entries: I) -> Self {
        entries.into_iter().collect::<Vec<_>>().into()
    }
}

impl<'a> IntoIterator for &'a DirectoryEntrySnapshot {
    type Item = &'a DirectoryEntry;
    type IntoIter = std::slice::Iter<'a, DirectoryEntry>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter()
    }
}

impl fmt::Debug for DirectoryEntrySnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.entries.fmt(formatter)
    }
}

#[cfg(test)]
#[path = "entry_snapshot_tests.rs"]
mod tests;
