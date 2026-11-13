//! 一个 shared-mime-info `mime.cache` 文件（规范 “The mime.cache files” 一节，1.x 版）。
//!
//! 文件按只读映射使用：`update-mime-database` 写新文件后改名替换，旧映射一直指向旧文件。
//! 所有偏移都来自文件本身，越界一律当作查不到，坏文件不会让进程崩溃。

use std::fs::File;
use std::path::Path;

use super::fnmatch::fnmatch;

const HEADER_BYTES: usize = 40;
const ALIAS_LIST: usize = 4;
const PARENT_LIST: usize = 8;
const LITERAL_LIST: usize = 12;
const SUFFIX_TREE: usize = 16;
const GLOB_LIST: usize = 20;
const MAGIC_LIST: usize = 24;
const CASE_SENSITIVE_FLAG: u32 = 0x100;
const WEIGHT_MASK: u32 = 0xff;

/// 按名字找到的一个类型和它的 glob 权重。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct NameMatch<'a> {
    pub(super) mime: &'a str,
    pub(super) weight: u32,
}

/// 名字比对的一遍：不分大小写的规则拿小写名字比，分大小写的拿原名比。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CasePass {
    Folded,
    Exact,
}

impl CasePass {
    fn accepts(self, weight_and_flags: u32) -> bool {
        let case_sensitive = weight_and_flags & CASE_SENSITIVE_FLAG != 0;
        match self {
            Self::Folded => !case_sensitive,
            Self::Exact => case_sensitive,
        }
    }
}

pub(super) struct MimeCache {
    bytes: CacheBytes,
}

impl MimeCache {
    pub(super) fn open(path: &Path) -> Option<Self> {
        let file = File::open(path).ok()?;
        let len = usize::try_from(file.metadata().ok()?.len()).ok()?;
        if len < HEADER_BYTES {
            return None;
        }
        let cache = Self {
            bytes: CacheBytes::map(&file, len)?,
        };
        cache.has_supported_version().then_some(cache)
    }

    #[cfg(test)]
    pub(super) fn from_bytes(bytes: Vec<u8>) -> Option<Self> {
        let cache = Self {
            bytes: CacheBytes::Owned(bytes),
        };
        (cache.bytes().len() >= HEADER_BYTES && cache.has_supported_version()).then_some(cache)
    }

    fn has_supported_version(&self) -> bool {
        self.bytes().get(..2) == Some(&[0, 1])
    }

    fn bytes(&self) -> &[u8] {
        self.bytes.as_slice()
    }

    fn card32(&self, offset: usize) -> Option<u32> {
        let bytes = self.bytes().get(offset..offset.checked_add(4)?)?;
        Some(u32::from_be_bytes(bytes.try_into().ok()?))
    }

    fn offset(&self, offset: usize) -> Option<usize> {
        usize::try_from(self.card32(offset)?).ok()
    }

    fn string(&self, offset: usize) -> Option<&str> {
        let tail = self.bytes().get(offset..)?;
        let end = tail.iter().position(|byte| *byte == 0)?;
        std::str::from_utf8(&tail[..end]).ok()
    }

    /// 定长条目的表：开头是条目数，条目从第 4 字节起。
    fn table(&self, header_field: usize) -> Option<(usize, usize)> {
        let list = self.offset(header_field)?;
        Some((self.offset(list)?, list + 4))
    }

    /// 按条目第一个字段（字符串偏移）二分查找；比较与 `strcmp` 一样按字节。
    fn binary_search(&self, header_field: usize, entry_bytes: usize, key: &str) -> Option<usize> {
        let (count, first) = self.table(header_field)?;
        let (mut low, mut high) = (0usize, count);
        while low < high {
            let middle = low + (high - low) / 2;
            let entry = first + middle * entry_bytes;
            let candidate = self.string(self.offset(entry)?)?;
            match candidate.as_bytes().cmp(key.as_bytes()) {
                std::cmp::Ordering::Less => low = middle + 1,
                std::cmp::Ordering::Greater => high = middle,
                std::cmp::Ordering::Equal => return Some(entry),
            }
        }
        None
    }

    pub(super) fn unalias(&self, mime: &str) -> Option<&str> {
        let entry = self.binary_search(ALIAS_LIST, 8, mime)?;
        self.string(self.offset(entry + 4)?)
    }

    pub(super) fn parents(&self, mime: &str) -> Vec<&str> {
        let Some(entry) = self.binary_search(PARENT_LIST, 8, mime) else {
            return Vec::new();
        };
        let Some(list) = self.offset(entry + 4) else {
            return Vec::new();
        };
        let count = self.offset(list).unwrap_or(0);
        (0..count)
            .filter_map(|index| self.string(self.offset(list + 4 + index * 4)?))
            .collect()
    }

    /// 整个名字等于某条规则（如 `makefile`）。
    pub(super) fn literal(&self, file_name: &str, pass: CasePass) -> Option<&str> {
        let entry = self.binary_search(LITERAL_LIST, 12, file_name)?;
        let flags = self.card32(entry + 8)?;
        pass.accepts(flags)
            .then(|| self.string(self.offset(entry + 4)?))
            .flatten()
    }

    /// 后缀树：从名字最后一个字往前走，取能走到的最长后缀上的类型。
    pub(super) fn suffix_matches<'a>(
        &'a self,
        file_name: &str,
        pass: CasePass,
        matches: &mut Vec<NameMatch<'a>>,
    ) {
        let Some((count, first)) = self
            .offset(SUFFIX_TREE)
            .and_then(|tree| Some((self.offset(tree)?, self.offset(tree + 4)?)))
        else {
            return;
        };
        let reversed = file_name.chars().rev().collect::<Vec<_>>();
        self.suffix_node_matches(count, first, &reversed, pass, matches);
    }

    fn suffix_node_matches<'a>(
        &'a self,
        count: usize,
        first: usize,
        remaining: &[char],
        pass: CasePass,
        matches: &mut Vec<NameMatch<'a>>,
    ) -> bool {
        let Some((&character, rest)) = remaining.split_first() else {
            return false;
        };
        let Some(node) = self.find_child(count, first, u32::from(character)) else {
            return false;
        };
        let (Some(children), Some(first_child)) = (self.offset(node + 4), self.offset(node + 8))
        else {
            return false;
        };
        // 更长的后缀优先；更深处没有类型时才用这一层的叶子。
        if self.suffix_node_matches(children, first_child, rest, pass, matches) {
            return true;
        }
        let found_before = matches.len();
        for index in 0..children {
            let child = first_child + index * 12;
            if self.card32(child) != Some(0) {
                break;
            }
            let (Some(mime), Some(flags)) = (
                self.offset(child + 4)
                    .and_then(|offset| self.string(offset)),
                self.card32(child + 8),
            ) else {
                continue;
            };
            if pass.accepts(flags) {
                matches.push(NameMatch {
                    mime,
                    weight: flags & WEIGHT_MASK,
                });
            }
        }
        matches.len() > found_before
    }

    /// 同一层的兄弟节点按字符升序排列。
    fn find_child(&self, count: usize, first: usize, character: u32) -> Option<usize> {
        let (mut low, mut high) = (0usize, count);
        while low < high {
            let middle = low + (high - low) / 2;
            let node = first + middle * 12;
            match self.card32(node)?.cmp(&character) {
                std::cmp::Ordering::Less => low = middle + 1,
                std::cmp::Ordering::Greater => high = middle,
                std::cmp::Ordering::Equal => return Some(node),
            }
        }
        None
    }

    /// 后缀树表达不了的通配规则（如 `README*`、`*.[1-9]`）。
    pub(super) fn glob_matches<'a>(
        &'a self,
        file_name: &str,
        pass: CasePass,
        matches: &mut Vec<NameMatch<'a>>,
    ) {
        let Some((count, first)) = self.table(GLOB_LIST) else {
            return;
        };
        for index in 0..count {
            let entry = first + index * 12;
            let (Some(pattern), Some(mime), Some(flags)) = (
                self.offset(entry).and_then(|offset| self.string(offset)),
                self.offset(entry + 4)
                    .and_then(|offset| self.string(offset)),
                self.card32(entry + 8),
            ) else {
                continue;
            };
            if pass.accepts(flags) && fnmatch(pattern, file_name) {
                matches.push(NameMatch {
                    mime,
                    weight: flags & WEIGHT_MASK,
                });
            }
        }
    }

    /// 判断类型最多要看开头多少字节。
    pub(super) fn magic_extent(&self) -> usize {
        self.offset(MAGIC_LIST)
            .and_then(|list| self.offset(list + 4))
            .unwrap_or(0)
    }

    /// 内容特征：条目已按优先级从高到低排好，第一个命中的就是结果。
    pub(super) fn magic_match(&self, data: &[u8]) -> Option<(&str, u32)> {
        let list = self.offset(MAGIC_LIST)?;
        let count = self.offset(list)?;
        let first = self.offset(list + 8)?;
        (0..count).find_map(|index| {
            let entry = first + index * 16;
            let priority = self.card32(entry)?;
            let matchlets = self.offset(entry + 8)?;
            let first_matchlet = self.offset(entry + 12)?;
            (0..matchlets)
                .any(|matchlet| self.matchlet_matches(first_matchlet + matchlet * 32, data))
                .then(|| Some((self.string(self.offset(entry + 4)?)?, priority)))
                .flatten()
        })
    }

    /// 自己命中且（有子条件时）任一子条件命中。
    fn matchlet_matches(&self, matchlet: usize, data: &[u8]) -> bool {
        if !self.matchlet_value_matches(matchlet, data) {
            return false;
        }
        let (Some(children), Some(first_child)) =
            (self.offset(matchlet + 24), self.offset(matchlet + 28))
        else {
            return false;
        };
        children == 0
            || (0..children).any(|child| self.matchlet_matches(first_child + child * 32, data))
    }

    /// 在 `[range_start, range_start + range_length)` 的每个起点比对值（有掩码先按位与）。
    /// 起点超出数据时不再往后找。没有掩码时整段交给子串查找：建索引时每个
    /// 名字认不出类型的文件都要比对一遍，逐个起点比太慢（一个文件要试三万多个位置）。
    fn matchlet_value_matches(&self, matchlet: usize, data: &[u8]) -> bool {
        let (Some(range_start), Some(range_length), Some(value_length), Some(value), Some(mask)) = (
            self.offset(matchlet),
            self.offset(matchlet + 4),
            self.offset(matchlet + 12),
            self.offset(matchlet + 16),
            self.offset(matchlet + 20),
        ) else {
            return false;
        };
        let Some(expected) = self.bytes().get(value..value + value_length) else {
            return false;
        };
        if range_length == 0 {
            return false;
        }
        if expected.is_empty() {
            return range_start <= data.len();
        }
        // 能放下整个值的最后一个起点之后的数据用不到。
        let window_end = range_start
            .saturating_add(range_length - 1)
            .saturating_add(value_length)
            .min(data.len());
        let Some(window) = data.get(range_start..window_end) else {
            return false;
        };
        match mask {
            0 => memchr::memmem::find(window, expected).is_some(),
            offset => {
                let Some(mask) = self.bytes().get(offset..offset + value_length) else {
                    return false;
                };
                window.windows(value_length).any(|candidate| {
                    candidate
                        .iter()
                        .zip(expected)
                        .zip(mask)
                        .all(|((byte, value), mask)| byte & mask == value & mask)
                })
            }
        }
    }

    /// 库里出现过的全部类型名（别名之外）：父类表、名字规则、内容特征里的类型。
    pub(super) fn known_types(&self, types: &mut Vec<String>) {
        if let Some((count, first)) = self.table(PARENT_LIST) {
            for index in 0..count {
                if let Some(mime) = self
                    .offset(first + index * 8)
                    .and_then(|offset| self.string(offset))
                {
                    types.push(mime.to_owned());
                    types.extend(self.parents(mime).into_iter().map(str::to_owned));
                }
            }
        }
        for header_field in [LITERAL_LIST, GLOB_LIST] {
            if let Some((count, first)) = self.table(header_field) {
                types.extend((0..count).filter_map(|index| {
                    Some(
                        self.string(self.offset(first + index * 12 + 4)?)?
                            .to_owned(),
                    )
                }));
            }
        }
        if let Some((count, first)) = self
            .offset(MAGIC_LIST)
            .and_then(|list| Some((self.offset(list)?, self.offset(list + 8)?)))
        {
            types.extend((0..count).filter_map(|index| {
                Some(
                    self.string(self.offset(first + index * 16 + 4)?)?
                        .to_owned(),
                )
            }));
        }
        if let Some((count, first)) = self
            .offset(SUFFIX_TREE)
            .and_then(|tree| Some((self.offset(tree)?, self.offset(tree + 4)?)))
        {
            self.suffix_tree_types(count, first, types);
        }
    }

    fn suffix_tree_types(&self, count: usize, first: usize, types: &mut Vec<String>) {
        for index in 0..count {
            let node = first + index * 12;
            if self.card32(node) == Some(0) {
                if let Some(mime) = self.offset(node + 4).and_then(|offset| self.string(offset)) {
                    types.push(mime.to_owned());
                }
            } else if let (Some(children), Some(first_child)) =
                (self.offset(node + 4), self.offset(node + 8))
            {
                self.suffix_tree_types(children, first_child, types);
            }
        }
    }
}

/// 映射进来的文件内容（测试里也可以是内存里的字节）。
enum CacheBytes {
    Mapped {
        address: *mut std::ffi::c_void,
        len: usize,
    },
    #[cfg(test)]
    Owned(Vec<u8>),
}

// 映射是只读、私有的，映射期间不会被改写，跨线程共享只读切片是安全的。
unsafe impl Send for CacheBytes {}
unsafe impl Sync for CacheBytes {}

impl CacheBytes {
    fn map(file: &File, len: usize) -> Option<Self> {
        // SAFETY：只读私有映射；`update-mime-database` 用改名替换文件，不会就地截断，
        // 映射在 Drop 里解除，长度与文件长度一致。
        let address = unsafe {
            rustix::mm::mmap(
                std::ptr::null_mut(),
                len,
                rustix::mm::ProtFlags::READ,
                rustix::mm::MapFlags::PRIVATE,
                file,
                0,
            )
        }
        .ok()?;
        Some(Self::Mapped { address, len })
    }

    fn as_slice(&self) -> &[u8] {
        match self {
            // SAFETY：`address..address + len` 是 `map` 建立的只读映射，活到 Drop。
            Self::Mapped { address, len } => unsafe {
                std::slice::from_raw_parts(address.cast::<u8>(), *len)
            },
            #[cfg(test)]
            Self::Owned(bytes) => bytes,
        }
    }
}

impl Drop for CacheBytes {
    fn drop(&mut self) {
        match self {
            // SAFETY：解除的正是 `map` 建立的那段映射，之后不再访问。
            Self::Mapped { address, len } => {
                let _ = unsafe { rustix::mm::munmap(*address, *len) };
            }
            #[cfg(test)]
            Self::Owned(_) => {}
        }
    }
}
