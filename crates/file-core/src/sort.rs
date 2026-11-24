use std::cmp::Ordering;
use std::ffi::OsStr;

use crate::{
    DirectoryEntry, DirectoryMetadataState, DiscoveredDirectoryEntry, FileKind, ScanOptions,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SortField {
    Name,
    Size,
    Kind,
    Modified,
    /// 状态改变时间（ctime）。
    Changed,
}

impl SortField {
    /// 按它排要先读每个条目的文件信息（stat）；名字和类型在列目录时就有。
    pub fn needs_filesystem_metadata(self) -> bool {
        match self {
            Self::Name | Self::Kind => false,
            Self::Size | Self::Modified | Self::Changed => true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SortDirection {
    Ascending,
    Descending,
}

pub fn sort_entries(entries: &mut [DirectoryEntry], options: &ScanOptions) {
    let names = NameSortKeys::new(entries.iter().map(DirectoryEntry::name));
    let mut order = (0..entries.len()).collect::<Vec<_>>();
    order.sort_unstable_by(|&left, &right| {
        compare_entries_with(&entries[left], &entries[right], options, || {
            names.compare(left, right)
        })
    });
    let sorted = order
        .into_iter()
        .map(|index| entries[index].clone())
        .collect::<Vec<_>>();
    entries.clone_from_slice(&sorted);
}

pub fn discovered_sort_is_ready(
    entries: &[DiscoveredDirectoryEntry],
    options: &ScanOptions,
) -> bool {
    !options.sort_field.needs_filesystem_metadata()
        || entries
            .iter()
            .all(|entry| !matches!(entry.filesystem_metadata(), DirectoryMetadataState::Pending))
}

pub fn sort_discovered_entry_indices(
    entries: &[DiscoveredDirectoryEntry],
    options: &ScanOptions,
) -> Vec<usize> {
    let names = NameSortKeys::new(entries.iter().map(DiscoveredDirectoryEntry::name));
    let mut indices = (0..entries.len()).collect::<Vec<_>>();
    indices.sort_unstable_by(|&left, &right| {
        compare_discovered_entries(&entries[left], &entries[right], options, || {
            names.compare(left, right)
        })
    });
    indices
}

/// `names` 是两者名字的先后：单个比较时现比，一大批一起排时查算好的排序键。
fn compare_discovered_entries(
    left: &DiscoveredDirectoryEntry,
    right: &DiscoveredDirectoryEntry,
    options: &ScanOptions,
    names: impl FnOnce() -> Ordering,
) -> Ordering {
    if let Some(ordering) = directory_position(left.kind(), right.kind(), options) {
        return ordering;
    }

    let ordering = match options.sort_field {
        SortField::Kind => kind_rank(left.kind())
            .cmp(&kind_rank(right.kind()))
            .then_with(names),
        SortField::Name => names(),
        SortField::Size => {
            return compare_discovered_filesystem_metadata(
                left,
                right,
                options,
                |metadata| metadata.len,
                names,
            );
        }
        SortField::Modified => {
            return compare_discovered_filesystem_metadata(
                left,
                right,
                options,
                |metadata| metadata.modified,
                names,
            );
        }
        SortField::Changed => {
            return compare_discovered_filesystem_metadata(
                left,
                right,
                options,
                |metadata| metadata.changed,
                names,
            );
        }
    };
    apply_sort_direction(ordering, options.sort_direction)
}

fn compare_discovered_filesystem_metadata<T: Ord>(
    left: &DiscoveredDirectoryEntry,
    right: &DiscoveredDirectoryEntry,
    options: &ScanOptions,
    value: impl Fn(&crate::DirectoryFilesystemMetadata) -> T,
    names: impl FnOnce() -> Ordering,
) -> Ordering {
    match (left.filesystem_metadata(), right.filesystem_metadata()) {
        (
            DirectoryMetadataState::Complete(left_metadata),
            DirectoryMetadataState::Complete(right_metadata),
        ) => apply_sort_direction(
            value(left_metadata)
                .cmp(&value(right_metadata))
                .then_with(names),
            options.sort_direction,
        ),
        (DirectoryMetadataState::Complete(_), _) => Ordering::Less,
        (_, DirectoryMetadataState::Complete(_)) => Ordering::Greater,
        (DirectoryMetadataState::Unavailable(_), DirectoryMetadataState::Pending) => Ordering::Less,
        (DirectoryMetadataState::Pending, DirectoryMetadataState::Unavailable(_)) => {
            Ordering::Greater
        }
        _ => apply_sort_direction(names(), options.sort_direction),
    }
}

pub fn compare_entries(
    left: &DirectoryEntry,
    right: &DirectoryEntry,
    options: &ScanOptions,
) -> Ordering {
    compare_entries_with(left, right, options, || {
        compare_names(left.name(), right.name())
    })
}

fn compare_entries_with(
    left: &DirectoryEntry,
    right: &DirectoryEntry,
    options: &ScanOptions,
    names: impl FnOnce() -> Ordering,
) -> Ordering {
    if let Some(ordering) = directory_position(left.kind(), right.kind(), options) {
        return ordering;
    }

    let ordering = match options.sort_field {
        SortField::Name => names(),
        SortField::Size => left.size().cmp(&right.size()).then_with(names),
        SortField::Kind => kind_rank(left.kind())
            .cmp(&kind_rank(right.kind()))
            .then_with(names),
        SortField::Modified => left.modified().cmp(&right.modified()).then_with(names),
        SortField::Changed => left.changed().cmp(&right.changed()).then_with(names),
    };

    apply_sort_direction(ordering, options.sort_direction)
}

fn directory_position(left: FileKind, right: FileKind, options: &ScanOptions) -> Option<Ordering> {
    if !options.directories_first {
        return None;
    }
    match (left == FileKind::Directory, right == FileKind::Directory) {
        (true, false) => Some(Ordering::Less),
        (false, true) => Some(Ordering::Greater),
        _ => None,
    }
}

fn apply_sort_direction(ordering: Ordering, direction: SortDirection) -> Ordering {
    match direction {
        SortDirection::Ascending => ordering,
        SortDirection::Descending => ordering.reverse(),
    }
}

fn kind_rank(kind: FileKind) -> u8 {
    match kind {
        FileKind::Directory => 0,
        FileKind::File => 1,
        FileKind::Symlink => 2,
        FileKind::Other => 3,
    }
}

/// 名字按人读的顺序：不分大小写，连着的数字按数值比（`第2集` 在 `第10集` 前面）。这样比一样
/// 的（只差大小写、数字前面的 0）再按原始字节定先后，任何一批名字排出来只有一种顺序。不是
/// UTF-8 的字节当作替换字符比。
pub fn compare_names(left: &OsStr, right: &OsStr) -> Ordering {
    let (left, right) = (left.as_encoded_bytes(), right.as_encoded_bytes());
    natural_order(left, right).then_with(|| left.cmp(right))
}

fn natural_order(mut left: &[u8], mut right: &[u8]) -> Ordering {
    loop {
        let (Some(&left_byte), Some(&right_byte)) = (left.first(), right.first()) else {
            return right.is_empty().cmp(&left.is_empty());
        };
        let ordering = if left_byte.is_ascii_digit() && right_byte.is_ascii_digit() {
            let (left_digits, left_rest) = split_digit_run(left);
            let (right_digits, right_rest) = split_digit_run(right);
            (left, right) = (left_rest, right_rest);
            left_digits
                .len()
                .cmp(&right_digits.len())
                .then_with(|| left_digits.cmp(right_digits))
        } else if left_byte.is_ascii() && right_byte.is_ascii() {
            (left, right) = (&left[1..], &right[1..]);
            left_byte
                .to_ascii_lowercase()
                .cmp(&right_byte.to_ascii_lowercase())
        } else {
            let (left_char, left_width) = leading_char(left);
            let (right_char, right_width) = leading_char(right);
            (left, right) = (&left[left_width..], &right[right_width..]);
            folded_case(left_char).cmp(&folded_case(right_char))
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
}

/// 一批名字各自的排序键：按字节比两个键，和 `natural_order` 比两个名字的先后一样（UTF-8 按
/// 字节比就是按码位比；一段数字记成 `0`、去掉前导 0 后的位数和这些数字，`0` 和任何不是数字
/// 的字比都与原来的数字比得一样）。一起排时每个名字只算一次键，比逐对现比快好几倍。
struct NameSortKeys<'a> {
    names: Vec<&'a [u8]>,
    keys: Vec<Vec<u8>>,
}

impl<'a> NameSortKeys<'a> {
    fn new(names: impl Iterator<Item = &'a OsStr>) -> Self {
        let names = names.map(OsStr::as_encoded_bytes).collect::<Vec<_>>();
        let keys = names.iter().map(|name| name_sort_key(name)).collect();
        Self { names, keys }
    }

    fn compare(&self, left: usize, right: usize) -> Ordering {
        self.keys[left]
            .cmp(&self.keys[right])
            .then_with(|| self.names[left].cmp(self.names[right]))
    }
}

fn name_sort_key(mut name: &[u8]) -> Vec<u8> {
    let mut key = Vec::with_capacity(name.len() + 8);
    while let Some(&byte) = name.first() {
        if byte.is_ascii_digit() {
            let (digits, rest) = split_digit_run(name);
            key.push(b'0');
            key.extend_from_slice(&(digits.len() as u32).to_be_bytes());
            key.extend_from_slice(digits);
            name = rest;
        } else if byte.is_ascii() {
            key.push(byte.to_ascii_lowercase());
            name = &name[1..];
        } else {
            let (character, width) = leading_char(name);
            let mut encoded = [0; 4];
            key.extend_from_slice(folded_case(character).encode_utf8(&mut encoded).as_bytes());
            name = &name[width..];
        }
    }
    key
}

/// 开头连着的数字去掉前导 0 后的那段，和后面剩下的。
fn split_digit_run(text: &[u8]) -> (&[u8], &[u8]) {
    let end = text
        .iter()
        .position(|byte| !byte.is_ascii_digit())
        .unwrap_or(text.len());
    let start = text[..end]
        .iter()
        .position(|byte| *byte != b'0')
        .unwrap_or(end);
    (&text[start..end], &text[end..])
}

/// 开头的一个字和它占几个字节；不是合法 UTF-8 的一个字节当替换字符。
fn leading_char(text: &[u8]) -> (char, usize) {
    let width = match text[0] {
        0x00..=0x7F => 1,
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => 0,
    };
    match text
        .get(..width)
        .and_then(|encoded| std::str::from_utf8(encoded).ok())
        .and_then(|encoded| encoded.chars().next())
    {
        Some(character) => (character, width),
        None => (char::REPLACEMENT_CHARACTER, 1),
    }
}

fn folded_case(character: char) -> char {
    if character.is_ascii() {
        character.to_ascii_lowercase()
    } else {
        character.to_lowercase().next().unwrap_or(character)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    use super::*;
    use crate::EntryMetadata;

    fn file_changed_at(name: &str, changed_seconds: u64) -> DirectoryEntry {
        DirectoryEntry::new(
            PathBuf::from("/folder").join(name),
            FileKind::File,
            EntryMetadata {
                // 修改时间和状态改变时间反着来：排序必须看对字段。
                modified: Some(
                    SystemTime::UNIX_EPOCH + Duration::from_secs(1000 - changed_seconds),
                ),
                changed: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(changed_seconds)),
                ..EntryMetadata::default()
            },
            false,
            false,
            false,
        )
    }

    #[test]
    fn sorting_by_status_change_time_uses_ctime_not_the_modified_time() {
        let mut entries = vec![
            file_changed_at("b", 30),
            file_changed_at("a", 10),
            file_changed_at("c", 20),
        ];
        let options = ScanOptions {
            sort_field: SortField::Changed,
            ..ScanOptions::default()
        };

        sort_entries(&mut entries, &options);

        let names: Vec<_> = entries
            .iter()
            .map(|entry| entry.name().to_owned())
            .collect();
        assert_eq!(names, ["a", "c", "b"]);
    }

    /// 名字的顺序：数字按数值，大小写不分，只差这些的再按原样定先后。
    #[test]
    fn names_sort_by_number_value_and_ignore_case() {
        let mut names = [
            "img10.jpg",
            "Img2.jpg",
            "img2.jpg",
            "img02.jpg",
            "Zebra",
            "apple",
            "第10集",
            "第2集",
            "img1.jpg",
            "Ä",
            "a",
        ]
        .map(OsStr::new);
        names.sort_by(|left, right| compare_names(left, right));
        assert_eq!(
            names.map(|name| name.to_str().unwrap()),
            [
                "a",
                "apple",
                "img1.jpg",
                "Img2.jpg",
                "img02.jpg",
                "img2.jpg",
                "img10.jpg",
                "Zebra",
                "Ä",
                "第2集",
                "第10集",
            ]
        );
    }

    /// 一大批一起排用的排序键和逐对比较排出同一个顺序。
    #[test]
    fn sort_keys_order_names_like_the_comparison() {
        use std::os::unix::ffi::OsStrExt;
        let mut names = (0..400u32)
            .map(|index| {
                let number = index.wrapping_mul(2_654_435_761) % 97;
                match index % 5 {
                    0 => format!("IMG_{number:03}.jpg").into_bytes(),
                    1 => format!("img_{number}.JPG").into_bytes(),
                    2 => format!("照片{number}").into_bytes(),
                    3 => format!("Ä{number}b").into_bytes(),
                    _ => [b"x".as_slice(), &[0xff, b'0' + (index % 10) as u8]].concat(),
                }
            })
            .collect::<Vec<_>>();
        names.push(b"9".to_vec());
        names.push(b"10".to_vec());
        names.push(b"/".to_vec());
        let names = names
            .iter()
            .map(|name| OsStr::from_bytes(name))
            .collect::<Vec<_>>();
        let keys = NameSortKeys::new(names.iter().copied());
        let mut by_keys = (0..names.len()).collect::<Vec<_>>();
        by_keys.sort_by(|&left, &right| keys.compare(left, right));
        let mut by_comparison = (0..names.len()).collect::<Vec<_>>();
        by_comparison.sort_by(|&left, &right| compare_names(names[left], names[right]));
        assert_eq!(by_keys, by_comparison);
    }

    /// 比较对任何名字都前后一致：不合法的 UTF-8 也排得进去，同一批名字怎么打乱都排成一种顺序。
    #[test]
    fn name_order_is_total_even_for_names_that_are_not_utf8() {
        use std::os::unix::ffi::OsStrExt;
        let names = [
            OsStr::from_bytes(b"b\xff"),
            OsStr::new("b"),
            OsStr::new("B"),
            OsStr::from_bytes(b"\xfe"),
            OsStr::new("b2"),
            OsStr::new("b10"),
        ];
        let mut forward = names;
        let mut backward = names;
        backward.reverse();
        forward.sort_by(|left, right| compare_names(left, right));
        backward.sort_by(|left, right| compare_names(left, right));
        assert_eq!(forward, backward);
        for pair in forward.windows(2) {
            assert_eq!(compare_names(pair[0], pair[1]), Ordering::Less);
        }
    }

    #[test]
    fn only_name_and_kind_sort_without_reading_file_information() {
        assert!(!SortField::Name.needs_filesystem_metadata());
        assert!(!SortField::Kind.needs_filesystem_metadata());
        for field in [SortField::Size, SortField::Modified, SortField::Changed] {
            assert!(field.needs_filesystem_metadata(), "{field:?}");
        }
    }

    /// 改修改时间本身就是一次状态改变：ctime 落在「现在」，和改成 2001 年的修改时间不是一回事。
    #[test]
    fn status_change_time_follows_metadata_changes_not_the_modified_time() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("file");
        std::fs::write(&path, b"x").expect("write file");
        let long_ago = filetime::FileTime::from_unix_time(978_307_200, 0);
        filetime::set_file_mtime(&path, long_ago).expect("set modified time");
        let before = SystemTime::now() - Duration::from_secs(60);

        let metadata = std::fs::metadata(&path).expect("metadata");
        let changed = crate::status_changed(&metadata).expect("ctime");

        assert_eq!(
            metadata.modified().expect("modified"),
            SystemTime::UNIX_EPOCH + Duration::from_secs(978_307_200)
        );
        assert!(changed > before, "{changed:?}");
    }
}
