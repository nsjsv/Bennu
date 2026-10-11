//! 复制后没能保留的元数据摘要:从元数据恢复一路带到传输检查点和任务完成提示。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 摘要要写进每条传输检查点 JSON。大目录(例如复制到 U 盘时每个文件的权限都
/// 设不上)不能让检查点无限增长,所以只保留受影响条目总数和前几条样本;
/// 界面最多也只展示这么多条。
pub const METADATA_LOSS_SAMPLE_LIMIT: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataKind {
    Permissions,
    ExtendedAttributes,
    AccessControlList,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetadataLossSample {
    #[serde(with = "super::recoverable_transfer::path_codec")]
    pub path: PathBuf,
    pub kinds: BTreeSet<MetadataKind>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetadataLossSummary {
    pub affected_entries: u64,
    pub samples: Vec<MetadataLossSample>,
}

impl MetadataLossSummary {
    /// 记录一个条目的恢复结果;`kinds` 为空表示该条目元数据全部保留,不计入。
    pub fn record(&mut self, path: &Path, kinds: BTreeSet<MetadataKind>) {
        if kinds.is_empty() {
            return;
        }
        self.affected_entries += 1;
        if self.samples.len() < METADATA_LOSS_SAMPLE_LIMIT {
            self.samples.push(MetadataLossSample {
                path: path.to_path_buf(),
                kinds,
            });
        }
    }

    pub fn absorb(&mut self, other: MetadataLossSummary) {
        self.affected_entries += other.affected_entries;
        let room = METADATA_LOSS_SAMPLE_LIMIT.saturating_sub(self.samples.len());
        self.samples.extend(other.samples.into_iter().take(room));
    }

    pub fn is_empty(&self) -> bool {
        self.affected_entries == 0
    }

    /// 暂存区里记录的样本路径换算到提交后的最终位置:提示要指向用户能找到的
    /// 文件,而不是已经改名消失的暂存路径。样本都来自以 `staged_root` 为根的
    /// 复制,不在其下的路径原样保留。
    pub(crate) fn rebased(mut self, staged_root: &Path, final_root: &Path) -> Self {
        for sample in &mut self.samples {
            if let Ok(relative) = sample.path.strip_prefix(staged_root) {
                sample.path = if relative.as_os_str().is_empty() {
                    final_root.to_path_buf()
                } else {
                    final_root.join(relative)
                };
            }
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(kinds: &[MetadataKind]) -> BTreeSet<MetadataKind> {
        kinds.iter().copied().collect()
    }

    #[test]
    fn fully_preserved_entries_are_not_recorded() {
        let mut summary = MetadataLossSummary::default();
        summary.record(Path::new("/a"), BTreeSet::new());
        assert!(summary.is_empty());
        assert!(summary.samples.is_empty());
    }

    #[test]
    fn samples_are_capped_while_every_affected_entry_is_counted() {
        let mut summary = MetadataLossSummary::default();
        for index in 0..8 {
            summary.record(
                &PathBuf::from(format!("/file-{index}")),
                kinds(&[MetadataKind::Permissions]),
            );
        }
        assert_eq!(summary.affected_entries, 8);
        assert_eq!(summary.samples.len(), METADATA_LOSS_SAMPLE_LIMIT);
        assert_eq!(summary.samples[0].path, PathBuf::from("/file-0"));
    }

    #[test]
    fn staged_sample_paths_are_rebased_to_the_final_target() {
        let mut summary = MetadataLossSummary::default();
        summary.record(
            Path::new("/dst/.transfer/payload"),
            kinds(&[MetadataKind::Permissions]),
        );
        summary.record(
            Path::new("/dst/.transfer/payload/inner/file.txt"),
            kinds(&[MetadataKind::ExtendedAttributes]),
        );

        let rebased = summary.rebased(
            Path::new("/dst/.transfer/payload"),
            Path::new("/dst/photos"),
        );

        assert_eq!(rebased.samples[0].path, PathBuf::from("/dst/photos"));
        assert_eq!(
            rebased.samples[1].path,
            PathBuf::from("/dst/photos/inner/file.txt")
        );
    }

    #[test]
    fn absorbing_keeps_total_and_fills_remaining_sample_slots() {
        let mut first = MetadataLossSummary::default();
        for index in 0..3 {
            first.record(
                &PathBuf::from(format!("/first-{index}")),
                kinds(&[MetadataKind::ExtendedAttributes]),
            );
        }
        let mut second = MetadataLossSummary::default();
        for index in 0..4 {
            second.record(
                &PathBuf::from(format!("/second-{index}")),
                kinds(&[MetadataKind::AccessControlList]),
            );
        }

        first.absorb(second);

        assert_eq!(first.affected_entries, 7);
        let paths = first
            .samples
            .iter()
            .map(|sample| sample.path.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/first-0"),
                PathBuf::from("/first-1"),
                PathBuf::from("/first-2"),
                PathBuf::from("/second-0"),
                PathBuf::from("/second-1"),
            ]
        );
    }
}
