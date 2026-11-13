//! 文件类型：读系统的 freedesktop shared-mime-info 库（`mime.cache`），按规范建议的检查顺序
//! 判断——先看名字，名字定不下来再看开头内容，最后看像不像文字。桌面和“打开方式”
//! 用的是同一个库，类型名、别名、父子关系都以它为准。
//!
//! 系统里没有这个库时一切照常运行：名字认不出任何类型，内容只分文字和其他。

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod cache;
mod fnmatch;

use cache::{CasePass, MimeCache, NameMatch};

pub const TEXT_PLAIN: &str = "text/plain";
const OCTET_STREAM: &str = "application/octet-stream";
/// 判断“像不像文字”只看开头这么多字节（shared-mime-info 规范的建议）。
const TEXT_SNIFF_BYTES: usize = 128;
/// 隔多久检查一次库文件有没有被 `update-mime-database` 换掉。
const RECHECK_INTERVAL: Duration = Duration::from_secs(5);

/// 只看名字得到的结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameType {
    /// 名字就能确定类型，不用读文件。
    Known(String),
    /// 名字对应好几种类型（同权重），要看内容挑。
    Candidates(Vec<String>),
    /// 名字认不出，只能看内容。
    Unknown,
}

impl NameType {
    /// 不读文件时的结论：有候选就取第一个。
    pub fn without_content(self) -> Option<String> {
        match self {
            Self::Known(mime) => Some(mime),
            Self::Candidates(candidates) => candidates.into_iter().next(),
            Self::Unknown => None,
        }
    }

    fn candidates(&self) -> &[String] {
        match self {
            Self::Known(mime) => std::slice::from_ref(mime),
            Self::Candidates(candidates) => candidates,
            Self::Unknown => &[],
        }
    }
}

pub struct MimeDatabase {
    caches: Vec<MimeCache>,
    sources: Vec<SourceStamp>,
    /// 每页查询都要展开类型筛选（如“文本及其子类”，三百多个类型），展开一次就记住。
    inheriting: Mutex<HashMap<String, Arc<[String]>>>,
}

/// 库文件的身份：换文件（改名替换）或改内容都会变。
#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceStamp {
    path: PathBuf,
    identity: Option<(u64, u64, i64, i64)>,
}

impl SourceStamp {
    fn read(path: PathBuf) -> Self {
        let identity = std::fs::metadata(&path).ok().map(|metadata| {
            (
                metadata.dev(),
                metadata.ino(),
                metadata.mtime(),
                metadata.mtime_nsec(),
            )
        });
        Self { path, identity }
    }
}

struct SystemDatabase {
    database: Arc<MimeDatabase>,
    checked_at: Instant,
}

static SYSTEM: Mutex<Option<SystemDatabase>> = Mutex::new(None);

impl MimeDatabase {
    /// 进程里共享的系统库；库文件被换掉后（最多晚 5 秒）换成新库。
    pub fn system() -> Arc<MimeDatabase> {
        let mut system = SYSTEM
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = Instant::now();
        match system.as_mut() {
            Some(current) if now.duration_since(current.checked_at) < RECHECK_INTERVAL => {
                Arc::clone(&current.database)
            }
            Some(current) => {
                current.checked_at = now;
                if current.database.sources_changed() {
                    current.database = Arc::new(Self::load(system_cache_paths()));
                }
                Arc::clone(&current.database)
            }
            None => {
                let database = Arc::new(Self::load(system_cache_paths()));
                *system = Some(SystemDatabase {
                    database: Arc::clone(&database),
                    checked_at: now,
                });
                database
            }
        }
    }

    /// 按优先顺序加载这些 `mime.cache`（先用户自己的，再系统的）。
    pub fn load(paths: Vec<PathBuf>) -> Self {
        let sources = paths.into_iter().map(SourceStamp::read).collect::<Vec<_>>();
        let caches = sources
            .iter()
            .filter(|source| source.identity.is_some())
            .filter_map(|source| MimeCache::open(&source.path))
            .collect::<Vec<_>>();
        if caches.is_empty() {
            tracing::warn!(
                target: "file_search::mime",
                event = "mime_database_unavailable",
                "no shared-mime-info cache was found; file types fall back to text detection"
            );
        }
        Self {
            caches,
            sources,
            inheriting: Mutex::default(),
        }
    }

    #[cfg(test)]
    fn from_caches(caches: Vec<MimeCache>) -> Self {
        Self {
            caches,
            sources: Vec::new(),
            inheriting: Mutex::default(),
        }
    }

    fn sources_changed(&self) -> bool {
        self.sources
            .iter()
            .any(|source| SourceStamp::read(source.path.clone()) != *source)
    }

    /// 只看名字：先整名，再后缀树，最后通配规则；每一步先按原样比分大小写的规则，没有再拿
    /// 小写名字比不分大小写的（规范要求分大小写的优先）。只留权重最高的。
    pub fn type_from_name(&self, file_name: &OsStr) -> NameType {
        let file_name = file_name.to_string_lossy();
        let folded = file_name.to_ascii_lowercase();
        let passes = [
            (&*file_name, CasePass::Exact),
            (folded.as_str(), CasePass::Folded),
        ];
        if let Some(mime) = passes.iter().find_map(|(name, pass)| {
            self.caches
                .iter()
                .find_map(|cache| cache.literal(name, *pass))
        }) {
            return NameType::Known(mime.to_owned());
        }
        let mut matches = Vec::new();
        for (name, pass) in passes {
            for cache in &self.caches {
                cache.suffix_matches(name, pass, &mut matches);
            }
            if !matches.is_empty() {
                break;
            }
        }
        // 后缀已经给出唯一答案时不必再跑通配规则。
        if matches.len() < 2 {
            for (name, pass) in passes {
                for cache in &self.caches {
                    cache.glob_matches(name, pass, &mut matches);
                }
            }
        }
        name_type_from_matches(matches)
    }

    /// 名字定不下来时按开头内容定类型（`head` 至少给 `magic_extent()` 字节，文件更短就给全部）。
    /// 特征命中时，若某个名字候选是它的子类，用那个候选（`.tar.gz` 的特征是 gzip）；
    /// 特征没命中就取第一个名字候选；都没有时开头像文字算 text/plain，否则不知道（None）。
    pub fn type_from_content(&self, from_name: &NameType, head: &[u8]) -> Option<String> {
        if let NameType::Known(mime) = from_name {
            return Some(mime.clone());
        }
        let candidates = from_name.candidates();
        let magic = self
            .caches
            .iter()
            .filter_map(|cache| cache.magic_match(head))
            .max_by_key(|(_, priority)| *priority);
        let mime = match magic {
            Some((sniffed, _)) => candidates
                .iter()
                .find(|candidate| self.is_subclass(candidate, sniffed))
                .cloned()
                .unwrap_or_else(|| sniffed.to_owned()),
            None => match candidates.first() {
                Some(candidate) => candidate.clone(),
                None if looks_like_text(head) => TEXT_PLAIN.to_owned(),
                None => return None,
            },
        };
        (mime != OCTET_STREAM).then_some(mime)
    }

    /// 看内容时要读开头多少字节。
    pub fn magic_extent(&self) -> usize {
        self.caches
            .iter()
            .map(MimeCache::magic_extent)
            .max()
            .unwrap_or(0)
            .max(TEXT_SNIFF_BYTES)
    }

    /// 去掉别名后的规范类型名。
    pub fn canonical(&self, mime: &str) -> String {
        self.caches
            .iter()
            .find_map(|cache| cache.unalias(mime))
            .unwrap_or(mime)
            .to_owned()
    }

    /// `mime` 是不是 `base` 或它的子类（沿父类表往上找）。所有 `text/*` 都是 text/plain 的
    /// 子类，所有非 `inode/*` 类型都是 application/octet-stream 的子类（规范约定）。
    pub fn is_subclass(&self, mime: &str, base: &str) -> bool {
        let base = self.canonical(base);
        let mut pending = vec![self.canonical(mime)];
        let mut seen = HashSet::new();
        while let Some(current) = pending.pop() {
            if current == base
                || (base == TEXT_PLAIN && current.starts_with("text/"))
                || (base == OCTET_STREAM && !current.starts_with("inode/"))
            {
                return true;
            }
            if !seen.insert(current.clone()) {
                continue;
            }
            for cache in &self.caches {
                pending.extend(
                    cache
                        .parents(&current)
                        .into_iter()
                        .map(|parent| self.canonical(parent)),
                );
            }
        }
        false
    }

    /// `base` 和库里所有属于它的子类的类型（规范名，去重）。
    pub fn inheriting_types(&self, base: &str) -> Arc<[String]> {
        let mut inheriting = self
            .inheriting
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Arc::clone(
            inheriting
                .entry(base.to_owned())
                .or_insert_with(|| self.collect_inheriting_types(base).into()),
        )
    }

    fn collect_inheriting_types(&self, base: &str) -> Vec<String> {
        let mut known = vec![base.to_owned()];
        for cache in &self.caches {
            cache.known_types(&mut known);
        }
        let mut seen = HashSet::new();
        known
            .into_iter()
            .map(|mime| self.canonical(&mime))
            .filter(|mime| seen.insert(mime.clone()))
            .filter(|mime| self.is_subclass(mime, base))
            .collect()
    }
}

/// 只留权重最高的结果，去重；只剩一个就是名字确定的类型。
fn name_type_from_matches(mut matches: Vec<NameMatch<'_>>) -> NameType {
    let Some(highest) = matches.iter().map(|found| found.weight).max() else {
        return NameType::Unknown;
    };
    matches.retain(|found| found.weight == highest);
    let mut candidates = Vec::<String>::new();
    for found in matches {
        if !candidates.iter().any(|existing| existing == found.mime) {
            candidates.push(found.mime.to_owned());
        }
    }
    match candidates.len() {
        1 => NameType::Known(candidates.remove(0)),
        _ => NameType::Candidates(candidates),
    }
}

/// 开头没有除制表、换行、回车、换页以外的控制字符。
fn looks_like_text(head: &[u8]) -> bool {
    !head.is_empty()
        && !head
            .iter()
            .take(TEXT_SNIFF_BYTES)
            .any(|byte| byte.is_ascii_control() && !matches!(byte, b'\t' | b'\n' | b'\r' | 0x0c))
}

/// 按 XDG 顺序：`$XDG_DATA_HOME/mime`，再 `$XDG_DATA_DIRS` 里各目录的 `mime`。
fn system_cache_paths() -> Vec<PathBuf> {
    let data_dirs = std::env::var_os("XDG_DATA_DIRS")
        .filter(|value| !value.is_empty())
        .map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
        .unwrap_or_else(|| {
            vec![
                PathBuf::from("/usr/local/share"),
                PathBuf::from("/usr/share"),
            ]
        });
    std::iter::once(file_core::app_dirs::user_data_dir())
        .chain(data_dirs)
        .map(|directory| directory.join("mime").join("mime.cache"))
        .collect()
}

/// 只看名字的类型，给不读文件的场合（目录回退扫描）。
pub fn type_for_name_only(database: &MimeDatabase, path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|name| database.type_from_name(name).without_content())
}

#[cfg(test)]
#[path = "mime/tests.rs"]
mod tests;
