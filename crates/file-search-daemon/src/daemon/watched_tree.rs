//! 被监听目录的树。每个节点只存自己的名字和父节点，同一父目录下的路径前缀只存一次：
//! 7.9 万个目录时，按完整路径存三份约 38MB，这棵树只要几 MB。按路径查找沿路径组成部分
//! 在有序子节点里二分，按 watch 描述符查找走一张小表，撤一棵子树只走这棵子树。

use std::collections::HashMap;
use std::ffi::OsStr;
use std::num::NonZeroI32;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

const ROOT: u32 = 0;
/// 名字池里废弃的字节超过这么多、且多于在用的字节时，整体重排一次。
const MIN_COMPACTED_GARBAGE_BYTES: usize = 64 * 1024;

pub(super) type WatchDescriptor = NonZeroI32;

struct Node {
    parent: u32,
    name_start: u32,
    name_length: u16,
    watch: Option<WatchDescriptor>,
    /// 按名字排序。
    children: Vec<u32>,
}

pub(super) struct WatchedTree {
    nodes: Vec<Node>,
    free_nodes: Vec<u32>,
    names: Vec<u8>,
    garbage_name_bytes: usize,
    by_watch: HashMap<WatchDescriptor, u32>,
}

impl Default for WatchedTree {
    fn default() -> Self {
        Self {
            nodes: vec![Node {
                parent: ROOT,
                name_start: 0,
                name_length: 0,
                watch: None,
                children: Vec::new(),
            }],
            free_nodes: Vec::new(),
            names: Vec::new(),
            garbage_name_bytes: 0,
            by_watch: HashMap::new(),
        }
    }
}

impl WatchedTree {
    pub(super) fn watch_count(&self) -> usize {
        self.by_watch.len()
    }

    pub(super) fn is_watched(&self, directory: &Path) -> bool {
        self.lookup(directory)
            .is_some_and(|node| self.nodes[node as usize].watch.is_some())
    }

    /// 记下 `directory` 的 watch；路径不是由根开始的普通路径时不记，返回 false。
    /// 内核对同一个 inode 返回同一个描述符：目录被移走后在新位置再注册，描述符会从旧节点
    /// 转到新节点，之后撤旧路径不会连带撤掉新位置的监听。
    pub(super) fn insert(&mut self, directory: &Path, watch: WatchDescriptor) -> bool {
        let Some(node) = self.lookup_or_create(directory) else {
            return false;
        };
        if let Some(previous_node) = self.by_watch.insert(watch, node) {
            if previous_node != node {
                self.nodes[previous_node as usize].watch = None;
                self.prune_upward(previous_node);
            }
        }
        if let Some(replaced) = self.nodes[node as usize].watch.replace(watch) {
            if replaced != watch {
                self.by_watch.remove(&replaced);
            }
        }
        true
    }

    pub(super) fn path_of(&self, watch: WatchDescriptor) -> Option<PathBuf> {
        let mut node = *self.by_watch.get(&watch)?;
        let mut names = Vec::new();
        while node != ROOT {
            names.push(self.name(node));
            node = self.nodes[node as usize].parent;
        }
        let mut path = PathBuf::from("/");
        for name in names.into_iter().rev() {
            path.push(OsStr::from_bytes(name));
        }
        Some(path)
    }

    /// 撤掉 `scope` 本身和它下面的全部 watch，深的在前。
    pub(super) fn remove_scope(&mut self, scope: &Path) -> Vec<WatchDescriptor> {
        let Some(scope_node) = self.lookup(scope) else {
            return Vec::new();
        };
        let mut post_order = Vec::new();
        let mut pending = vec![(scope_node, false)];
        while let Some((node, children_done)) = pending.pop() {
            if children_done {
                post_order.push(node);
                continue;
            }
            pending.push((node, true));
            pending.extend(
                self.nodes[node as usize]
                    .children
                    .iter()
                    .map(|child| (*child, false)),
            );
        }
        let mut removed = Vec::new();
        for node in post_order {
            if let Some(watch) = self.nodes[node as usize].watch.take() {
                self.by_watch.remove(&watch);
                removed.push(watch);
            }
            if node != scope_node {
                self.release(node);
            }
        }
        self.nodes[scope_node as usize].children.clear();
        self.prune_upward(scope_node);
        self.compact_names_if_wasteful();
        removed
    }

    /// 内核撤掉了这个 watch（目录被删、所在文件系统卸载）。
    pub(super) fn forget(&mut self, watch: WatchDescriptor) {
        let Some(node) = self.by_watch.remove(&watch) else {
            return;
        };
        self.nodes[node as usize].watch = None;
        self.prune_upward(node);
        self.compact_names_if_wasteful();
    }

    /// 批量注册完一轮后调用：数组按倍数增长，7.9 万个目录时空着的容量约占一半。
    pub(super) fn shrink_to_fit(&mut self) {
        self.nodes.shrink_to_fit();
        for node in &mut self.nodes {
            node.children.shrink_to_fit();
        }
        self.names.shrink_to_fit();
        self.free_nodes.shrink_to_fit();
        self.by_watch.shrink_to_fit();
    }

    /// 估算占用的堆内存，供日志观察。
    pub(super) fn heap_bytes(&self) -> usize {
        let children = self
            .nodes
            .iter()
            .map(|node| node.children.capacity() * size_of::<u32>())
            .sum::<usize>();
        self.nodes.capacity() * size_of::<Node>()
            + self.names.capacity()
            + self.free_nodes.capacity() * size_of::<u32>()
            + self.by_watch.capacity() * (size_of::<WatchDescriptor>() + size_of::<u32>() + 1)
            + children
    }

    fn name(&self, node: u32) -> &[u8] {
        let node = &self.nodes[node as usize];
        let start = node.name_start as usize;
        &self.names[start..start + usize::from(node.name_length)]
    }

    fn child_position(&self, parent: u32, name: &[u8]) -> Result<usize, usize> {
        self.nodes[parent as usize]
            .children
            .binary_search_by(|child| self.name(*child).cmp(name))
    }

    fn lookup(&self, path: &Path) -> Option<u32> {
        let mut node = ROOT;
        for name in normal_components(path)? {
            let position = self.child_position(node, name).ok()?;
            node = self.nodes[node as usize].children[position];
        }
        Some(node)
    }

    fn lookup_or_create(&mut self, path: &Path) -> Option<u32> {
        let mut node = ROOT;
        for name in normal_components(path)? {
            node = match self.child_position(node, name) {
                Ok(position) => self.nodes[node as usize].children[position],
                Err(position) => {
                    let child = self.allocate(node, name)?;
                    self.nodes[node as usize].children.insert(position, child);
                    child
                }
            };
        }
        Some(node)
    }

    fn allocate(&mut self, parent: u32, name: &[u8]) -> Option<u32> {
        let name_length = u16::try_from(name.len()).ok()?;
        let name_start = u32::try_from(self.names.len()).ok()?;
        self.names.extend_from_slice(name);
        let node = Node {
            parent,
            name_start,
            name_length,
            watch: None,
            children: Vec::new(),
        };
        Some(match self.free_nodes.pop() {
            Some(free) => {
                self.nodes[free as usize] = node;
                free
            }
            None => {
                self.nodes.push(node);
                u32::try_from(self.nodes.len() - 1).ok()?
            }
        })
    }

    fn release(&mut self, node: u32) {
        self.garbage_name_bytes += usize::from(self.nodes[node as usize].name_length);
        let released = &mut self.nodes[node as usize];
        released.name_length = 0;
        released.children = Vec::new();
        self.free_nodes.push(node);
    }

    /// 没有 watch 也没有子节点的节点只是空壳，从它往上逐个回收。
    fn prune_upward(&mut self, mut node: u32) {
        while node != ROOT {
            let current = &self.nodes[node as usize];
            if current.watch.is_some() || !current.children.is_empty() {
                return;
            }
            let parent = current.parent;
            let siblings = &mut self.nodes[parent as usize].children;
            if let Some(position) = siblings.iter().position(|child| *child == node) {
                siblings.remove(position);
            }
            self.release(node);
            node = parent;
        }
    }

    fn compact_names_if_wasteful(&mut self) {
        let live_bytes = self.names.len() - self.garbage_name_bytes;
        if self.garbage_name_bytes < MIN_COMPACTED_GARBAGE_BYTES
            || self.garbage_name_bytes <= live_bytes
        {
            return;
        }
        let mut names = Vec::with_capacity(live_bytes);
        let free = self
            .free_nodes
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        for index in 1..self.nodes.len() {
            if free.contains(&(index as u32)) {
                continue;
            }
            let node = &mut self.nodes[index];
            let start = node.name_start as usize;
            let new_start = names.len() as u32;
            names.extend_from_slice(&self.names[start..start + usize::from(node.name_length)]);
            node.name_start = new_start;
        }
        self.names = names;
        self.garbage_name_bytes = 0;
    }
}

/// 只接受由根开始、只含普通组成部分的路径；`.`、`..` 和相对路径不是监听目录的身份。
fn normal_components(path: &Path) -> Option<Vec<&[u8]>> {
    let mut components = path.components();
    if components.next() != Some(Component::RootDir) {
        return None;
    }
    components
        .map(|component| match component {
            Component::Normal(name) => Some(name.as_bytes()),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
#[path = "watched_tree_tests.rs"]
mod tests;
