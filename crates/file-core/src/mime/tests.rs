use std::ffi::OsStr;
use std::path::PathBuf;

use super::cache::MimeCache;
use super::{MimeDatabase, NameType, TEXT_PLAIN};

/// 按规范拼一个 `mime.cache`：只放测试要用的表，字符串追加在末尾。
#[derive(Default)]
struct CacheBuilder {
    aliases: Vec<(&'static str, &'static str)>,
    parents: Vec<(&'static str, Vec<&'static str>)>,
    literals: Vec<(&'static str, &'static str, u32)>,
    suffixes: Vec<(&'static str, &'static str, u32)>,
    globs: Vec<(&'static str, &'static str, u32)>,
    magic: Vec<Magic>,
}

struct Magic {
    priority: u32,
    mime: &'static str,
    matchlets: Vec<Matchlet>,
}

#[derive(Clone)]
struct Matchlet {
    range_start: u32,
    range_length: u32,
    value: Vec<u8>,
    mask: Option<Vec<u8>>,
    children: Vec<Matchlet>,
}

fn matchlet(range_start: u32, value: &[u8]) -> Matchlet {
    Matchlet {
        range_start,
        range_length: 1,
        value: value.to_vec(),
        mask: None,
        children: Vec::new(),
    }
}

const CASE_SENSITIVE: u32 = 0x100;

struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    fn position(&self) -> u32 {
        self.bytes.len() as u32
    }

    fn reserve(&mut self, count: usize) -> u32 {
        let at = self.position();
        self.bytes.resize(self.bytes.len() + count * 4, 0);
        at
    }

    fn set(&mut self, at: u32, value: u32) {
        let at = at as usize;
        self.bytes[at..at + 4].copy_from_slice(&value.to_be_bytes());
    }

    fn string(&mut self, value: &str) -> u32 {
        let at = self.position();
        self.bytes.extend_from_slice(value.as_bytes());
        self.bytes.push(0);
        while !self.bytes.len().is_multiple_of(4) {
            self.bytes.push(0);
        }
        at
    }

    fn data(&mut self, value: &[u8]) -> u32 {
        let at = self.position();
        self.bytes.extend_from_slice(value);
        while !self.bytes.len().is_multiple_of(4) {
            self.bytes.push(0);
        }
        at
    }
}

/// 后缀树节点：字符 → 子节点；叶子（字符 0）挂类型。
#[derive(Default)]
struct SuffixNode {
    children: std::collections::BTreeMap<u32, SuffixNode>,
    leaves: Vec<(&'static str, u32)>,
}

impl CacheBuilder {
    fn build(mut self) -> MimeCache {
        let mut writer = Writer {
            bytes: vec![0u8; 40],
        };
        writer.bytes[..4].copy_from_slice(&[0, 1, 0, 2]);

        self.aliases.sort();
        let alias_list = writer.reserve(1 + self.aliases.len() * 2);
        writer.set(alias_list, self.aliases.len() as u32);
        for (index, (alias, mime)) in self.aliases.iter().enumerate() {
            let alias_at = writer.string(alias);
            let mime_at = writer.string(mime);
            writer.set(alias_list + 4 + index as u32 * 8, alias_at);
            writer.set(alias_list + 8 + index as u32 * 8, mime_at);
        }
        writer.set(4, alias_list);

        self.parents.sort();
        let parent_list = writer.reserve(1 + self.parents.len() * 2);
        writer.set(parent_list, self.parents.len() as u32);
        for (index, (mime, parents)) in self.parents.iter().enumerate() {
            let mime_at = writer.string(mime);
            let parents_at = writer.reserve(1 + parents.len());
            writer.set(parents_at, parents.len() as u32);
            for (parent_index, parent) in parents.iter().enumerate() {
                let parent_at = writer.string(parent);
                writer.set(parents_at + 4 + parent_index as u32 * 4, parent_at);
            }
            writer.set(parent_list + 4 + index as u32 * 8, mime_at);
            writer.set(parent_list + 8 + index as u32 * 8, parents_at);
        }
        writer.set(8, parent_list);

        self.literals.sort();
        let literal_list = write_name_table(&mut writer, &self.literals);
        writer.set(12, literal_list);

        let mut root = SuffixNode::default();
        for (suffix, mime, flags) in &self.suffixes {
            let mut node = &mut root;
            for character in suffix.chars().rev() {
                node = node.children.entry(u32::from(character)).or_default();
            }
            node.leaves.push((mime, *flags));
        }
        let tree = writer.reserve(2);
        let (count, first) = write_suffix_children(&mut writer, &root);
        writer.set(tree, count);
        writer.set(tree + 4, first);
        writer.set(16, tree);

        let glob_list = write_name_table(&mut writer, &self.globs);
        writer.set(20, glob_list);

        self.magic
            .sort_by_key(|magic| std::cmp::Reverse(magic.priority));
        let magic_list = writer.reserve(3);
        writer.set(magic_list, self.magic.len() as u32);
        writer.set(magic_list + 4, 4096);
        let entries = writer.reserve(self.magic.len() * 4);
        writer.set(magic_list + 8, entries);
        for (index, magic) in self.magic.iter().enumerate() {
            let entry = entries + index as u32 * 16;
            let mime_at = writer.string(magic.mime);
            let (count, first) = write_matchlets(&mut writer, &magic.matchlets);
            writer.set(entry, magic.priority);
            writer.set(entry + 4, mime_at);
            writer.set(entry + 8, count);
            writer.set(entry + 12, first);
        }
        writer.set(24, magic_list);

        let empty = writer.reserve(1);
        for field in [28, 32, 36] {
            writer.set(field, empty);
        }
        MimeCache::from_bytes(writer.bytes).expect("built cache is valid")
    }
}

fn write_name_table(writer: &mut Writer, entries: &[(&'static str, &'static str, u32)]) -> u32 {
    let list = writer.reserve(1 + entries.len() * 3);
    writer.set(list, entries.len() as u32);
    for (index, (name, mime, flags)) in entries.iter().enumerate() {
        let name_at = writer.string(name);
        let mime_at = writer.string(mime);
        let entry = list + 4 + index as u32 * 12;
        writer.set(entry, name_at);
        writer.set(entry + 4, mime_at);
        writer.set(entry + 8, *flags);
    }
    list
}

fn write_suffix_children(writer: &mut Writer, node: &SuffixNode) -> (u32, u32) {
    let count = node.leaves.len() + node.children.len();
    let first = writer.reserve(count * 3);
    for (index, (mime, flags)) in node.leaves.iter().enumerate() {
        let mime_at = writer.string(mime);
        let at = first + index as u32 * 12;
        writer.set(at, 0);
        writer.set(at + 4, mime_at);
        writer.set(at + 8, *flags);
    }
    for (index, (character, child)) in node.children.iter().enumerate() {
        let (child_count, child_first) = write_suffix_children(writer, child);
        let at = first + (node.leaves.len() + index) as u32 * 12;
        writer.set(at, *character);
        writer.set(at + 4, child_count);
        writer.set(at + 8, child_first);
    }
    (count as u32, first)
}

fn write_matchlets(writer: &mut Writer, matchlets: &[Matchlet]) -> (u32, u32) {
    let first = writer.reserve(matchlets.len() * 8);
    for (index, matchlet) in matchlets.iter().enumerate() {
        let at = first + index as u32 * 32;
        let value_at = writer.data(&matchlet.value);
        let mask_at = matchlet.mask.as_ref().map_or(0, |mask| writer.data(mask));
        let (children, first_child) = write_matchlets(writer, &matchlet.children);
        for (offset, value) in [
            matchlet.range_start,
            matchlet.range_length,
            1,
            matchlet.value.len() as u32,
            value_at,
            mask_at,
            children,
            first_child,
        ]
        .into_iter()
        .enumerate()
        {
            writer.set(at + offset as u32 * 4, value);
        }
    }
    (matchlets.len() as u32, first)
}

fn database() -> MimeDatabase {
    let cache = CacheBuilder {
        aliases: vec![
            ("application/x-gzip", "application/gzip"),
            ("text/x-markdown", "text/markdown"),
        ],
        parents: vec![
            ("application/x-compressed-tar", vec!["application/gzip"]),
            ("application/xml", vec![TEXT_PLAIN]),
            ("image/svg+xml", vec!["application/xml"]),
            ("application/json", vec!["application/javascript"]),
            ("application/javascript", vec![TEXT_PLAIN]),
        ],
        literals: vec![
            ("makefile", "text/x-makefile", 50),
            ("Doxyfile", "text/x-doxygen", 50 | CASE_SENSITIVE),
        ],
        suffixes: vec![
            (".txt", TEXT_PLAIN, 50),
            (".md", "text/markdown", 50),
            (".gz", "application/gzip", 50),
            (".tar.gz", "application/x-compressed-tar", 50),
            (".xml", "application/xml", 50),
            (".svg", "image/svg+xml", 50),
            (".json", "application/json", 50),
            (".json", "application/schema+json", 50),
            (".ts", "video/mp2t", 50),
            (".ts", "text/vnd.trolltech.linguist", 50),
            (".ts", "application/typescript", 40),
            (".py", "text/x-python", 60),
            (".py", "text/x-python3", 50),
            (".C", "text/x-c++src", 50 | CASE_SENSITIVE),
            (".c", "text/x-csrc", 50),
        ],
        globs: vec![("readme*", "text/x-readme", 10)],
        magic: vec![
            Magic {
                priority: 50,
                mime: "application/gzip",
                matchlets: vec![matchlet(0, b"\x1f\x8b")],
            },
            Magic {
                priority: 80,
                mime: "application/pdf",
                matchlets: vec![Matchlet {
                    range_start: 0,
                    range_length: 64,
                    ..matchlet(0, b"%PDF-")
                }],
            },
            Magic {
                priority: 50,
                mime: "video/mp2t",
                matchlets: vec![Matchlet {
                    children: vec![matchlet(188, b"\x47")],
                    ..matchlet(0, b"\x47")
                }],
            },
            Magic {
                priority: 40,
                mime: "image/png",
                matchlets: vec![Matchlet {
                    mask: Some(b"\xff\x00\xff".to_vec()),
                    ..matchlet(0, b"\x89\x00\x4e")
                }],
            },
        ],
    }
    .build();
    MimeDatabase::from_caches(vec![cache])
}

fn known(mime: &str) -> NameType {
    NameType::Known(mime.to_owned())
}

#[test]
fn names_resolve_by_literal_suffix_and_glob() {
    let database = database();
    let by_name = |name: &str| database.type_from_name(OsStr::new(name));
    assert_eq!(by_name("notes.txt"), known(TEXT_PLAIN));
    assert_eq!(by_name("NOTES.TXT"), known(TEXT_PLAIN));
    assert_eq!(by_name("Makefile"), known("text/x-makefile"));
    assert_eq!(by_name("README.zh"), known("text/x-readme"));
    assert_eq!(by_name("data.bin"), NameType::Unknown);
    assert_eq!(by_name("no-extension"), NameType::Unknown);
}

#[test]
fn the_longest_suffix_wins() {
    let database = database();
    assert_eq!(
        database.type_from_name(OsStr::new("backup.tar.gz")),
        known("application/x-compressed-tar")
    );
    assert_eq!(
        database.type_from_name(OsStr::new("backup.gz")),
        known("application/gzip")
    );
}

#[test]
fn case_sensitive_rules_only_match_the_exact_spelling() {
    let database = database();
    assert_eq!(
        database.type_from_name(OsStr::new("main.C")),
        known("text/x-c++src")
    );
    assert_eq!(
        database.type_from_name(OsStr::new("main.c")),
        known("text/x-csrc")
    );
    assert_eq!(
        database.type_from_name(OsStr::new("Doxyfile")),
        known("text/x-doxygen")
    );
    assert_eq!(
        database.type_from_name(OsStr::new("doxyfile")),
        NameType::Unknown
    );
}

#[test]
fn only_the_highest_weight_counts() {
    let database = database();
    assert_eq!(
        database.type_from_name(OsStr::new("tool.py")),
        known("text/x-python")
    );
    assert_eq!(
        database.type_from_name(OsStr::new("main.ts")),
        NameType::Candidates(vec![
            "video/mp2t".to_owned(),
            "text/vnd.trolltech.linguist".to_owned()
        ])
    );
}

#[test]
fn content_decides_between_name_candidates() {
    let database = database();
    let ts = database.type_from_name(OsStr::new("main.ts"));
    let mut transport = vec![0u8; 200];
    transport[0] = 0x47;
    transport[188] = 0x47;
    assert_eq!(
        database.type_from_content(&ts, &transport).as_deref(),
        Some("video/mp2t")
    );
    // 特征没命中：取第一个名字候选。
    assert_eq!(
        database
            .type_from_content(&ts, b"export const a = 1;\n")
            .as_deref(),
        Some("video/mp2t")
    );
}

#[test]
fn a_name_candidate_that_inherits_from_the_sniffed_type_wins() {
    let database = database();
    let candidates = NameType::Candidates(vec![
        "application/x-compressed-tar".to_owned(),
        "text/plain".to_owned(),
    ]);
    assert_eq!(
        database
            .type_from_content(&candidates, b"\x1f\x8b\x08\x00")
            .as_deref(),
        Some("application/x-compressed-tar")
    );
}

#[test]
fn unknown_names_are_sniffed_then_judged_as_text() {
    let database = database();
    let unknown = NameType::Unknown;
    assert_eq!(
        database
            .type_from_content(&unknown, b"junk %PDF-1.7\n")
            .as_deref(),
        Some("application/pdf")
    );
    assert_eq!(
        database
            .type_from_content(&unknown, b"#!/bin/sh\necho hi\n")
            .as_deref(),
        Some(TEXT_PLAIN)
    );
    assert_eq!(
        database.type_from_content(&unknown, b"\x00\x01\x02binary"),
        None
    );
    assert_eq!(database.type_from_content(&unknown, b""), None);
}

#[test]
fn masks_and_child_matchlets_are_honoured() {
    let database = database();
    assert_eq!(
        database
            .type_from_content(&NameType::Unknown, b"\x89\xffN rest")
            .as_deref(),
        Some("image/png")
    );
    // 父条件命中但子条件没命中（数据不够长）不算。
    assert_eq!(
        database.type_from_content(&NameType::Unknown, b"\x47\x00 short"),
        None
    );
}

#[test]
fn aliases_and_subclasses() {
    let database = database();
    assert_eq!(database.canonical("application/x-gzip"), "application/gzip");
    assert_eq!(database.canonical("text/plain"), "text/plain");
    assert!(database.is_subclass("image/svg+xml", "application/xml"));
    assert!(database.is_subclass("image/svg+xml", TEXT_PLAIN));
    assert!(database.is_subclass("text/x-markdown", TEXT_PLAIN));
    assert!(database.is_subclass("application/x-compressed-tar", "application/x-gzip"));
    assert!(!database.is_subclass("application/gzip", TEXT_PLAIN));
    assert!(database.is_subclass("application/gzip", "application/octet-stream"));
    assert!(!database.is_subclass("inode/directory", "application/octet-stream"));
}

#[test]
fn inheriting_types_lists_every_known_subclass() {
    let database = database();
    let mut text = database.inheriting_types(TEXT_PLAIN).to_vec();
    text.sort();
    for expected in [
        "application/javascript",
        "application/json",
        "application/xml",
        "image/svg+xml",
        TEXT_PLAIN,
        "text/markdown",
        "text/x-makefile",
        "text/x-python",
        "text/x-readme",
    ] {
        assert!(text.contains(&expected.to_owned()), "missing {expected}");
    }
    assert!(!text.contains(&"application/gzip".to_owned()));
    assert!(!text.contains(&"video/mp2t".to_owned()));
}

#[test]
fn a_missing_or_broken_database_still_tells_text_apart() {
    let database = MimeDatabase::load(vec![PathBuf::from("/nonexistent/mime.cache")]);
    assert_eq!(
        database.type_from_name(OsStr::new("notes.txt")),
        NameType::Unknown
    );
    assert_eq!(
        database
            .type_from_content(&NameType::Unknown, b"hello")
            .as_deref(),
        Some(TEXT_PLAIN)
    );
    assert!(MimeCache::from_bytes(vec![0u8; 8]).is_none());
    let mut truncated = vec![0u8; 40];
    truncated[..4].copy_from_slice(&[0, 1, 0, 2]);
    truncated[4..8].copy_from_slice(&1000u32.to_be_bytes());
    let broken = MimeDatabase::from_caches(vec![MimeCache::from_bytes(truncated).unwrap()]);
    assert_eq!(broken.canonical("text/plain"), "text/plain");
    assert_eq!(
        broken.type_from_name(OsStr::new("notes.txt")),
        NameType::Unknown
    );
}

/// 本机装了 shared-mime-info 时，与桌面的判断对拍几个常见名字。
#[test]
fn the_installed_database_agrees_with_the_desktop() {
    let path = PathBuf::from("/usr/share/mime/mime.cache");
    if !path.exists() {
        return;
    }
    let database = MimeDatabase::load(vec![path]);
    let by_name = |name: &str| database.type_from_name(OsStr::new(name)).without_content();
    assert_eq!(by_name("a.txt").as_deref(), Some(TEXT_PLAIN));
    assert_eq!(
        by_name("a.tar.gz").as_deref(),
        Some("application/x-compressed-tar")
    );
    assert_eq!(by_name("a.py").as_deref(), Some("text/x-python"));
    assert_eq!(by_name("Makefile").as_deref(), Some("text/x-makefile"));
    assert_eq!(by_name("a.csv").as_deref(), Some("text/csv"));
    for (name, expected) in [
        ("notes.md", "text/markdown"),
        ("photo.png", "image/png"),
        ("recording.flac", "audio/flac"),
        ("movie.mkv", "video/x-matroska"),
        ("bundle.7z", "application/x-7z-compressed"),
        ("report.doc", "application/msword"),
        (
            "report.docx",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        ),
        ("workbook.xls", "application/vnd.ms-excel"),
        ("document.odt", "application/vnd.oasis.opendocument.text"),
        ("report.pdf", "application/pdf"),
    ] {
        // 不同版本的库可能把旧名留作别名，按规范名比。
        assert_eq!(by_name(name), Some(database.canonical(expected)), "{name}");
    }
    assert!(database.is_subclass("image/svg+xml", TEXT_PLAIN));
    assert!(database.is_subclass("application/json", TEXT_PLAIN));
    assert!(database
        .inheriting_types(TEXT_PLAIN)
        .contains(&"text/x-python".to_owned()));
}
