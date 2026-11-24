//! 文件夹树：从根开始只列文件夹，展开到哪列到哪。只记列过的那几层，列表在后台读；跟着活动
//! 窗格的当前目录展开它的上级。根照 Dolphin：当前目录在主目录里时是主目录，出了主目录是 `/`
//! （「只限主目录」关掉后总是 `/`）。根自己不占一行，第一层是根下面的文件夹。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use file_core::{DirectoryEntryChanges, FileKind, ResolvedEntryChange};

/// 一行的高度：只画看得见的行，按它算哪几行在视口里。
pub(crate) const FOLDER_ROW_HEIGHT: f32 = 28.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FolderNode {
    pub(crate) path: PathBuf,
    pub(crate) name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Listing {
    Loading {
        request: u64,
    },
    Loaded(Vec<FolderNode>),
    /// 读不了（没权限、已经没了）：当作没有子文件夹。
    Unreadable,
}

/// 要在后台读的一个文件夹；读完带着 `request` 回来，对不上（期间又读了一次）就丢掉。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ListingRequest {
    pub(crate) directory: PathBuf,
    pub(crate) request: u64,
    pub(crate) include_hidden: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Expander {
    /// 还没列过：可能有子文件夹，箭头先画着（列一层下面的每个文件夹太费）。
    Collapsed,
    Expanded,
    Loading,
    /// 列过了，没有子文件夹。
    Empty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FolderRow<'a> {
    pub(crate) path: &'a Path,
    pub(crate) name: &'a str,
    pub(crate) depth: usize,
    pub(crate) expander: Expander,
}

#[derive(Debug, Clone)]
pub(crate) struct FolderTree {
    root: PathBuf,
    include_hidden: bool,
    listings: HashMap<PathBuf, Listing>,
    /// 展开着的文件夹（不含根，根总是列着）。
    expanded: HashSet<PathBuf>,
    /// 活动窗格的当前目录：高亮它。
    current: Option<PathBuf>,
    /// 跟随之后还没把当前目录滚出来：等它那一行出现（上级都列完）再滚。
    reveal_pending: bool,
    next_request: u64,
    /// 视口：滚动位置和高度，画哪几行、要不要滚都按它。
    pub(crate) scroll_offset: f32,
    pub(crate) viewport_height: f32,
}

/// 还没量到视口时当它这么高：先画这么多行，量到了再按真的来。
const UNMEASURED_VIEWPORT_HEIGHT: f32 = 1200.0;

/// 树的根：当前目录在主目录里（且只限主目录）时是主目录，否则是 `/`。
pub(crate) fn tree_root(directory: &Path, home: &Path, limit_to_home: bool) -> PathBuf {
    if limit_to_home && directory.starts_with(home) {
        home.to_path_buf()
    } else {
        PathBuf::from("/")
    }
}

impl FolderTree {
    pub(crate) fn new(root: PathBuf, include_hidden: bool) -> Self {
        Self {
            root,
            include_hidden,
            listings: HashMap::new(),
            expanded: HashSet::new(),
            current: None,
            reveal_pending: false,
            next_request: 0,
            scroll_offset: 0.0,
            viewport_height: UNMEASURED_VIEWPORT_HEIGHT,
        }
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn include_hidden(&self) -> bool {
        self.include_hidden
    }

    pub(crate) fn current(&self) -> Option<&Path> {
        self.current.as_deref()
    }

    /// 根还没列过时列它（刚建好的树）。
    pub(crate) fn list_root(&mut self) -> Option<ListingRequest> {
        let root = self.root.clone();
        (!self.listings.contains_key(&root)).then(|| self.start_listing(root))
    }

    /// 当前目录换了：展开根和它之间的每一级，高亮它，等它那一行出来后滚过去。
    pub(crate) fn follow(&mut self, directory: &Path) -> Vec<ListingRequest> {
        self.current = Some(directory.to_path_buf());
        self.reveal_pending = true;
        let mut requests = Vec::from_iter(self.list_root());
        let ancestors = directory
            .ancestors()
            .skip(1)
            .take_while(|ancestor| *ancestor != self.root && ancestor.starts_with(&self.root))
            .map(Path::to_path_buf)
            .collect::<Vec<_>>();
        for ancestor in ancestors.into_iter().rev() {
            self.expanded.insert(ancestor.clone());
            if !self.listings.contains_key(&ancestor) {
                requests.push(self.start_listing(ancestor));
            }
        }
        requests
    }

    /// 点了箭头：展开着就收起（连同下面展开的，不再看着它们），收着就展开（没列过先列）。
    pub(crate) fn toggle(&mut self, directory: &Path) -> Option<ListingRequest> {
        if self.expanded.contains(directory) {
            self.forget_subtree(directory);
            return None;
        }
        self.expanded.insert(directory.to_path_buf());
        (!self.listings.contains_key(directory))
            .then(|| self.start_listing(directory.to_path_buf()))
    }

    /// 后台读完了一个文件夹。不是最近一次请求的结果（期间又读了一次、已经收起）丢掉。
    pub(crate) fn accept_listing(
        &mut self,
        directory: &Path,
        request: u64,
        folders: Option<Vec<FolderNode>>,
    ) {
        let Some(listing) = self.listings.get_mut(directory) else {
            return;
        };
        if *listing != (Listing::Loading { request }) {
            return;
        }
        *listing = match folders {
            Some(folders) => Listing::Loaded(folders),
            None => Listing::Unreadable,
        };
    }

    /// 「显示隐藏文件」换了：列过的都重列。
    pub(crate) fn relist_all(&mut self, include_hidden: bool) -> Vec<ListingRequest> {
        self.include_hidden = include_hidden;
        let listed = self.listings.keys().cloned().collect::<Vec<_>>();
        listed
            .into_iter()
            .map(|directory| self.start_listing(directory))
            .collect()
    }

    /// 要看着的文件夹：根和展开着的（它们的子文件夹列在树里）。
    pub(crate) fn watched_directories(&self) -> impl Iterator<Item = &Path> {
        std::iter::once(self.root.as_path()).chain(self.expanded.iter().map(PathBuf::as_path))
    }

    /// 看着的文件夹里有了变化。只关心文件夹：有文件夹进来、出去、改名，或者变化太多要重扫时
    /// 重列这一层（排序和隐藏规则都在读的那边，不在这里另写一套）；删掉的文件夹下面展开的
    /// 一并忘掉。
    pub(crate) fn accept_changes(
        &mut self,
        changes: &DirectoryEntryChanges,
    ) -> Option<ListingRequest> {
        let directory = &changes.directory;
        let listing = self.listings.get(directory)?;
        // 还在读时不知道哪些已经读进去了：凡是可能涉及文件夹的变化都重读。
        let is_child = |path: &Path| match listing {
            Listing::Loaded(folders) => folders.iter().any(|folder| folder.path == path),
            Listing::Loading { .. } => true,
            Listing::Unreadable => false,
        };
        let may_be_folder =
            |kind: FileKind| matches!(kind, FileKind::Directory | FileKind::Symlink);
        let mut relist = false;
        let mut gone = Vec::new();
        for change in &changes.changes {
            match change {
                ResolvedEntryChange::Added(entry) => relist |= may_be_folder(entry.kind()),
                ResolvedEntryChange::Removed { path } if is_child(path) => {
                    gone.push(path.clone());
                    relist = true;
                }
                ResolvedEntryChange::Removed { .. } | ResolvedEntryChange::ContentChanged(_) => {}
                ResolvedEntryChange::Renamed { from, to } => {
                    if is_child(from) {
                        gone.push(from.clone());
                        relist = true;
                    }
                    relist |= may_be_folder(to.kind());
                }
                ResolvedEntryChange::RescanRequired => relist = true,
            }
        }
        for path in gone {
            self.forget_subtree(&path);
        }
        relist.then(|| self.start_listing(directory.clone()))
    }

    /// 从上往下的所有行（只有展开的才往下走）。
    pub(crate) fn rows(&self) -> Vec<FolderRow<'_>> {
        let mut rows = Vec::new();
        self.push_rows(&self.root, 0, &mut rows);
        rows
    }

    fn push_rows<'a>(&'a self, directory: &Path, depth: usize, rows: &mut Vec<FolderRow<'a>>) {
        let Some(Listing::Loaded(folders)) = self.listings.get(directory) else {
            return;
        };
        for folder in folders {
            let expanded = self.expanded.contains(&folder.path);
            let expander = match self.listings.get(&folder.path) {
                Some(Listing::Loaded(children)) if children.is_empty() => Expander::Empty,
                Some(Listing::Unreadable) => Expander::Empty,
                Some(Listing::Loading { .. }) if expanded => Expander::Loading,
                Some(_) if expanded => Expander::Expanded,
                _ => Expander::Collapsed,
            };
            rows.push(FolderRow {
                path: &folder.path,
                name: &folder.name,
                depth,
                expander,
            });
            if expanded {
                self.push_rows(&folder.path, depth + 1, rows);
            }
        }
    }

    /// 跟随后当前目录那一行出来了：返回它是第几行（只返回一次）。上级有读不了的、当前目录
    /// 就是根时不再等。
    pub(crate) fn take_reveal(&mut self) -> Option<usize> {
        if !self.reveal_pending {
            return None;
        }
        let current = self.current.as_deref()?;
        if let Some(index) = self.rows().iter().position(|row| row.path == current) {
            self.reveal_pending = false;
            return Some(index);
        }
        let still_listing = current
            .ancestors()
            .skip(1)
            .take_while(|ancestor| ancestor.starts_with(&self.root))
            .any(|ancestor| matches!(self.listings.get(ancestor), Some(Listing::Loading { .. })));
        if !still_listing {
            self.reveal_pending = false;
        }
        None
    }

    /// 把第 `index` 行滚进视口要滚到哪（已经在里面时 None）：滚到视口中间。
    pub(crate) fn scroll_target_for_row(&self, index: usize) -> Option<f32> {
        let top = index as f32 * FOLDER_ROW_HEIGHT;
        let visible = self.scroll_offset..self.scroll_offset + self.viewport_height;
        if visible.contains(&top) && visible.contains(&(top + FOLDER_ROW_HEIGHT)) {
            return None;
        }
        Some((top - (self.viewport_height - FOLDER_ROW_HEIGHT) / 2.0).max(0.0))
    }

    fn start_listing(&mut self, directory: PathBuf) -> ListingRequest {
        self.next_request += 1;
        let request = self.next_request;
        self.listings
            .insert(directory.clone(), Listing::Loading { request });
        ListingRequest {
            directory,
            request,
            include_hidden: self.include_hidden,
        }
    }

    /// 收起或删掉一个文件夹：它和下面展开过的都不再展开、不再记列表（也就不再看着）。
    fn forget_subtree(&mut self, directory: &Path) {
        self.expanded.retain(|path| !path.starts_with(directory));
        self.listings.retain(|path, _| !path.starts_with(directory));
    }
}

#[cfg(test)]
#[path = "folder_tree_tests.rs"]
mod tests;
