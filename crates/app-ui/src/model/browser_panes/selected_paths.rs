use std::collections::HashSet;
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;

use super::next_content_revision;

/// 标签里选中的路径。每次可变借用都换一个新版本号：跟着选中走的面板按版本号判断选中变没变，
/// 不用逐个比较（选中几万项时每次更新都比一遍要毫秒级）。只借不改也会换号，多算一次而已。
#[derive(Debug, Clone, Default)]
pub(crate) struct SelectedPaths {
    paths: HashSet<PathBuf>,
    revision: u64,
}

impl SelectedPaths {
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }
}

impl From<HashSet<PathBuf>> for SelectedPaths {
    fn from(paths: HashSet<PathBuf>) -> Self {
        Self {
            paths,
            revision: next_content_revision(),
        }
    }
}

impl FromIterator<PathBuf> for SelectedPaths {
    fn from_iter<I: IntoIterator<Item = PathBuf>>(paths: I) -> Self {
        Self::from(paths.into_iter().collect::<HashSet<_>>())
    }
}

impl<'a> IntoIterator for &'a SelectedPaths {
    type Item = &'a PathBuf;
    type IntoIter = std::collections::hash_set::Iter<'a, PathBuf>;

    fn into_iter(self) -> Self::IntoIter {
        self.paths.iter()
    }
}

/// 只比路径：版本号是“哪次改的”，不是选中的内容。
impl PartialEq<HashSet<PathBuf>> for SelectedPaths {
    fn eq(&self, other: &HashSet<PathBuf>) -> bool {
        self.paths == *other
    }
}

impl Deref for SelectedPaths {
    type Target = HashSet<PathBuf>;

    fn deref(&self) -> &HashSet<PathBuf> {
        &self.paths
    }
}

impl DerefMut for SelectedPaths {
    fn deref_mut(&mut self) -> &mut HashSet<PathBuf> {
        self.revision = next_content_revision();
        &mut self.paths
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_change_gets_a_new_revision_and_reading_does_not() {
        let mut selected = SelectedPaths::default();
        let before = selected.revision();
        selected.insert(PathBuf::from("/a"));
        let after_insert = selected.revision();
        assert_ne!(after_insert, before);
        assert!(selected.contains(&PathBuf::from("/a")));
        assert_eq!(selected.revision(), after_insert);
        *selected = HashSet::new();
        assert_ne!(selected.revision(), after_insert);
    }
}
