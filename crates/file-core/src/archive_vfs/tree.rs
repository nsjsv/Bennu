//! 归档成员树：把扁平的成员列表合成可按目录逐级查询的树。
//!
//! 大量 zip 不写入显式目录条目，中间目录必须在此合成；同时这里
//! 承担 zip-slip 防护（拒绝绝对路径与 `..` 段成员）与「智能解压」
//! 所需的单根判定。树本身保持 UI 无关，只输出纯数据。

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::{archive_listing::ArchiveListingEntry, FileKind};

/// 树中一个成员节点的纯数据视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchiveMemberInfo {
    pub(crate) kind: FileKind,
    pub(crate) len: u64,
    pub(crate) modified: Option<SystemTime>,
}

#[derive(Debug, Default)]
struct ArchiveMemberNode {
    info: Option<ArchiveMemberInfo>,
    children: BTreeMap<OsString, ArchiveMemberNode>,
}

impl ArchiveMemberNode {
    fn is_directory(&self) -> bool {
        !self.children.is_empty()
            || self
                .info
                .as_ref()
                .is_some_and(|info| info.kind == FileKind::Directory)
    }
}

/// 成员路径 → 目录树的合成结果。
#[derive(Debug, Default)]
pub(crate) struct ArchiveMemberTree {
    root: ArchiveMemberNode,
}

impl ArchiveMemberTree {
    /// 由成员列表构建树。成员路径支持 `/` 与 `\` 分隔（7z/zip 历史
    /// 产物），`.` 段被忽略，绝对路径与含 `..` 段的成员被整体拒绝
    /// （zip-slip 防护）。
    pub(crate) fn build(members: impl IntoIterator<Item = ArchiveListingEntry>) -> Self {
        let mut tree = Self::default();
        for member in members {
            if let Some(segments) = normalized_member_segments(&member.path) {
                tree.insert_member(segments, member);
            }
        }
        tree
    }

    fn insert_member(&mut self, segments: Vec<OsString>, member: ArchiveListingEntry) {
        let mut node = &mut self.root;
        for segment in &segments {
            node = node.children.entry(segment.clone()).or_default();
        }
        // 同名冲突容忍：纯文件节点可被后续成员补充/覆盖；已是聚合
        // 目录的节点（先前作为中间段出现过）不接受文件信息。
        let is_aggregate_directory = !node.children.is_empty();
        if member.kind == FileKind::Directory || !is_aggregate_directory {
            node.info = Some(ArchiveMemberInfo {
                kind: member.kind,
                len: member.len,
                modified: member.modified,
            });
        }
    }

    /// 查询最内层归档内的相对路径节点；空路径即包根。
    fn lookup(&self, inner: &Path) -> Option<&ArchiveMemberNode> {
        let mut node = &self.root;
        for segment in inner.iter() {
            node = node.children.get(segment)?;
        }
        Some(node)
    }

    /// 列出目录节点的子项。隐藏过滤交给目录发现边界（与真实目录
    /// 共用 `is_hidden_name` 权威），树保持全量。
    pub(crate) fn children_of(&self, inner: &Path) -> Option<Vec<(OsString, ArchiveMemberInfo)>> {
        let node = self.lookup(inner)?;
        if !node.is_directory() {
            return None;
        }
        Some(
            node.children
                .iter()
                .map(|(name, child)| {
                    // 曾作为文件条目、后来又出现子项的节点按目录呈现：
                    // 聚合子项的事实优先于早先的文件条目声明。
                    let directory_fallback = ArchiveMemberInfo {
                        kind: FileKind::Directory,
                        len: 0,
                        modified: None,
                    };
                    let info = if child.children.is_empty() {
                        child.info.clone().unwrap_or(directory_fallback)
                    } else {
                        directory_fallback
                    };
                    (name.clone(), info)
                })
                .collect(),
        )
    }

    /// 智能解压的单根判定：根下恰有一个子项且为目录 → 返回其名称。
    /// 散文件、多顶层条目、空包都返回 `None`（多根语义）。
    pub(crate) fn single_root_name(&self) -> Option<&OsString> {
        if self.root.children.len() != 1 {
            return None;
        }
        let (name, node) = self.root.children.iter().next()?;
        node.is_directory().then_some(name)
    }

    /// 提取工作清单：文件成员是它自身，文件夹成员是子树内全部文件。
    /// 返回 (成员路径, 未压缩大小)；成员不存在返回 `None`。
    pub(crate) fn worklist_under(&self, inner_member: &Path) -> Option<Vec<(PathBuf, u64)>> {
        fn collect(node: &ArchiveMemberNode, prefix: &Path, out: &mut Vec<(PathBuf, u64)>) {
            if node.children.is_empty() {
                if let Some(info) = &node.info {
                    if info.kind != FileKind::Directory {
                        out.push((prefix.to_path_buf(), info.len));
                    }
                }
                return;
            }
            for (name, child) in &node.children {
                collect(child, &prefix.join(name), out);
            }
        }

        let node = self.lookup(inner_member)?;
        let mut worklist = Vec::new();
        if node.children.is_empty() && node.info.as_ref().is_some_and(|info| info.kind != FileKind::Directory) {
            worklist.push((
                inner_member.to_path_buf(),
                node.info.as_ref().map(|info| info.len).unwrap_or(0),
            ));
        } else {
            collect(node, inner_member, &mut worklist);
        }
        Some(worklist)
    }
}

/// 成员路径 → 规范化段序列。返回 `None` 表示该成员必须被拒绝。
fn normalized_member_segments(raw_path: &str) -> Option<Vec<OsString>> {
    let normalized = raw_path.replace('\\', "/");
    if normalized.starts_with('/') {
        return None;
    }
    let mut segments = Vec::new();
    for segment in normalized.split('/') {
        match segment {
            "" | "." => continue,
            ".." => return None,
            other => segments.push(OsString::from(other)),
        }
    }
    if segments.is_empty() {
        None
    } else {
        Some(segments)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive_listing::ArchiveListingEntry;

    fn member(path: &str, kind: FileKind, len: u64) -> ArchiveListingEntry {
        ArchiveListingEntry {
            path: path.to_owned(),
            kind,
            len,
            modified: None,
        }
    }

    fn child_names(tree: &ArchiveMemberTree, inner: &str) -> Vec<String> {
        tree.children_of(Path::new(inner))
            .unwrap()
            .into_iter()
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn implicit_directories_are_synthesized() {
        let tree = ArchiveMemberTree::build([member("a/b/c.txt", FileKind::File, 3)]);

        assert_eq!(child_names(&tree, ""), vec!["a"]);
        assert_eq!(child_names(&tree, "a"), vec!["b"]);
        assert_eq!(child_names(&tree, "a/b"), vec!["c.txt"]);
        assert!(tree.children_of(Path::new("a/b/c.txt")).is_none());
    }

    #[test]
    fn absolute_and_dotdot_members_are_rejected() {
        let tree = ArchiveMemberTree::build([
            member("/etc/passwd", FileKind::File, 10),
            member("ok/../evil.txt", FileKind::File, 10),
            member("safe.txt", FileKind::File, 1),
        ]);

        assert_eq!(child_names(&tree, ""), vec!["safe.txt"]);
    }

    #[test]
    fn windows_separators_and_dot_segments_normalize() {
        let tree = ArchiveMemberTree::build([member(
            ".\\docs\\readme.md",
            FileKind::File,
            5,
        )]);

        assert_eq!(child_names(&tree, ""), vec!["docs"]);
        assert_eq!(child_names(&tree, "docs"), vec!["readme.md"]);
    }

    #[test]
    fn single_root_is_detected_only_for_lone_directory() {
        let single = ArchiveMemberTree::build([
            member("root/", FileKind::Directory, 0),
            member("root/a.txt", FileKind::File, 1),
            member("root/sub/b.txt", FileKind::File, 1),
        ]);
        assert_eq!(
            single.single_root_name().map(OsString::as_os_str),
            Some(OsString::from("root").as_os_str())
        );

        let scattered = ArchiveMemberTree::build([
            member("a.txt", FileKind::File, 1),
            member("dir/", FileKind::Directory, 0),
            member("dir/b.txt", FileKind::File, 1),
        ]);
        assert_eq!(scattered.single_root_name(), None);

        let empty = ArchiveMemberTree::build([]);
        assert_eq!(empty.single_root_name(), None);
    }

    #[test]
    fn file_kind_reported_for_leaf_and_directory_for_inner_nodes() {
        let tree = ArchiveMemberTree::build([
            member("a/b/c.txt", FileKind::File, 7),
            member("a/top.md", FileKind::File, 2),
        ]);

        let children = tree.children_of(Path::new("a")).unwrap();
        let c_info = children
            .iter()
            .find(|(name, _)| name == "b")
            .map(|(_, info)| info.clone())
            .unwrap();
        assert_eq!(c_info.kind, FileKind::Directory);
        let top_info = children
            .iter()
            .find(|(name, _)| name == "top.md")
            .map(|(_, info)| info.clone())
            .unwrap();
        assert_eq!(top_info.kind, FileKind::File);
        assert_eq!(top_info.len, 2);
    }
}
