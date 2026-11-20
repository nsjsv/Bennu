//! 列压缩包目录：zip、7z、rar 交给系统 7z，tar 一类交给系统 tar。两边都给出包里原样存的
//! 名字字节，再统一转成显示用的名字（见 `member_names`）。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tokio_util::sync::CancellationToken;

use crate::{archive_extraction_format_for_path, ArchiveExtractionFormat, FileError, FileKind};

mod member_names;
mod seven_zip;
mod tar;

pub(crate) use member_names::decode_member_names;
pub(crate) use seven_zip::{list_seven_zip_entries, SevenZipListedEntry};
pub(crate) use tar::list_tar_members;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveListingEntry {
    /// 显示用路径：包里存的不是 UTF-8 时已按识别出的编码转好。
    pub path: String,
    /// 包里原样存的路径字节（列目录工具打印的样子），解出单个成员时按它找。
    pub(crate) stored_path: Vec<u8>,
    pub kind: FileKind,
    /// 未压缩大小；目录为 0。虚拟目录的 Size 排序与属性面板消费。
    pub len: u64,
    pub modified: Option<SystemTime>,
}

/// 列目录工具给出的一条，名字还没转成显示用的。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredMember {
    pub(crate) stored_path: Vec<u8>,
    pub(crate) kind: FileKind,
    pub(crate) len: u64,
    pub(crate) modified: Option<SystemTime>,
}

pub async fn list_archive_members(
    archive: impl Into<PathBuf>,
) -> Result<Vec<ArchiveListingEntry>, FileError> {
    let archive = archive.into();
    let format = archive_extraction_format_for_path(&archive).ok_or(FileError::Unsupported(
        "archive format is not supported for listing",
    ))?;
    list_archive_members_with_format(archive, format).await
}

/// 供预览管道在嗅探文件头后显式指定格式：自定义归档后缀无法从
/// 扩展名推断解压后端，必须由内容嗅探结果决定。
pub async fn list_archive_members_with_format(
    archive: impl Into<PathBuf>,
    format: ArchiveExtractionFormat,
) -> Result<Vec<ArchiveListingEntry>, FileError> {
    let archive = archive.into();
    let members = match format {
        ArchiveExtractionFormat::Tar => tar::list_tar_members(&archive).await?,
        ArchiveExtractionFormat::Zip
        | ArchiveExtractionFormat::SevenZip
        | ArchiveExtractionFormat::Rar => {
            list_seven_zip_entries(&archive, None, &CancellationToken::new())
                .await?
                .into_iter()
                .map(|entry| entry.member)
                .collect()
        }
    };
    Ok(listing_entries(members))
}

/// 包里存的成员 → 显示用的条目：不是 UTF-8 的名字按识别出的编码转好。
pub(crate) fn listing_entries(members: Vec<StoredMember>) -> Vec<ArchiveListingEntry> {
    let display_paths = decode_member_names(members.iter().map(|member| &member.stored_path[..]));
    members
        .into_iter()
        .zip(display_paths)
        .map(|(member, path)| ArchiveListingEntry {
            path,
            stored_path: member.stored_path,
            kind: member.kind,
            len: member.len,
            modified: member.modified,
        })
        .collect()
}

/// 读取文件头识别归档格式。调用方已确认该路径属于归档类型，
/// 无法识别魔数时按 tar 兜底——识别错误由列成员环节报错。
pub async fn sniff_archive_extraction_format(
    archive: impl Into<PathBuf>,
) -> ArchiveExtractionFormat {
    let archive = archive.into();
    let Some(header) = read_archive_header(&archive).await else {
        return ArchiveExtractionFormat::Tar;
    };
    if header.starts_with(b"PK\x03\x04") || header.starts_with(b"PK\x05\x06") {
        return ArchiveExtractionFormat::Zip;
    }
    if header.starts_with(&[0x37, 0x7a, 0xbc, 0xaf, 0x27, 0x1c]) {
        return ArchiveExtractionFormat::SevenZip;
    }
    if header.starts_with(b"Rar!\x1a\x07") {
        return ArchiveExtractionFormat::Rar;
    }
    // 压缩过的 tar（gzip/xz/zstd/bzip2）和旧式无魔数的 tar 都交给 tar 自己认。
    ArchiveExtractionFormat::Tar
}

async fn read_archive_header(path: &Path) -> Option<Vec<u8>> {
    use tokio::io::AsyncReadExt;

    let mut file = tokio::fs::File::open(path).await.ok()?;
    let mut header = [0u8; 8];
    let read = file.read(&mut header).await.ok()?;
    Some(header[..read].to_vec())
}

/// 分卷后续卷在 7z 眼里是残缺包,只报 Headers Error,报错信息对用户
/// 没有指向性。列表与提取两处错误包装共用这个提示文案,拼接为
/// "...; this is a continuation volume ..."。文案与 FileError 其余消息
/// 同为英文。
const SPLIT_VOLUME_CONTINUATION_HINT: &str =
    "this is a continuation volume of a split archive, open the first volume (e.g. part1) instead";

// ponytail: 文件名启发式,不解析 7z 报错语义——7z 对后续卷只给裸的
// Headers Error,没有可判别的结构化输出,文件名形态是唯一可用信号。
/// 新式分卷 `xxx.partN.rar`:N≥2 是后续卷,part1 与普通 `xxx.rar` 是
/// 首卷/单卷。老式分卷首卷是 `.rar`,后续从 `.r00` 起按
/// `.rNN/.sNN/.tNN/.uNN/.vNN` 两位数字递进,命中即后续卷。
pub(crate) fn split_volume_hint(path: &Path) -> Option<&'static str> {
    let file_name = path.file_name()?.to_string_lossy();
    let lower = file_name.to_ascii_lowercase();

    if let Some(stem) = lower.strip_suffix(".rar") {
        let is_continuation = stem
            .rsplit_once(".part")
            .and_then(|(_, digits)| digits.parse::<u32>().ok())
            .is_some_and(|number| number >= 2);
        return is_continuation.then_some(SPLIT_VOLUME_CONTINUATION_HINT);
    }

    // 老式后缀恰为三段:单字母 r→v 加两位数字;其余(.zip/.txt/.r000)
    // 都不是后续卷。
    let (_, extension) = lower.rsplit_once('.')?;
    let mut bytes = extension.bytes();
    match (bytes.next(), bytes.next(), bytes.next(), bytes.next()) {
        (Some(kind), Some(tens), Some(ones), None)
            if matches!(kind, b'r' | b's' | b't' | b'u' | b'v')
                && tens.is_ascii_digit()
                && ones.is_ascii_digit() =>
        {
            Some(SPLIT_VOLUME_CONTINUATION_HINT)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_volume_hint_flags_continuation_volumes_only() {
        let hint = |name: &str| split_volume_hint(Path::new(name));

        // 新式:part2 起是后续卷;part1 与普通 rar 不是。
        assert_eq!(
            hint("/tmp/movie.part2.rar"),
            Some(SPLIT_VOLUME_CONTINUATION_HINT)
        );
        assert_eq!(
            hint("/tmp/movie.part10.rar"),
            Some(SPLIT_VOLUME_CONTINUATION_HINT)
        );
        assert_eq!(
            hint("/tmp/MOVIE.PART3.RAR"),
            Some(SPLIT_VOLUME_CONTINUATION_HINT)
        );
        assert_eq!(hint("/tmp/movie.part1.rar"), None);
        assert_eq!(hint("/tmp/movie.rar"), None);

        // 老式:首卷是 .rar,后续从 .r00 起按 r→v 五组两位数字递进。
        assert_eq!(hint("/tmp/movie.r00"), Some(SPLIT_VOLUME_CONTINUATION_HINT));
        assert_eq!(hint("/tmp/movie.s17"), Some(SPLIT_VOLUME_CONTINUATION_HINT));
        assert_eq!(hint("/tmp/movie.v99"), Some(SPLIT_VOLUME_CONTINUATION_HINT));

        // 非分卷文件名不提示。
        assert_eq!(hint("/tmp/movie.zip"), None);
        assert_eq!(hint("/tmp/movie.txt"), None);
        assert_eq!(hint("/tmp/movie.r000"), None);
        assert_eq!(hint("/tmp/movie.part2x.rar"), None);

        // 无扩展名:早退返回 None,不 panic。
        assert_eq!(hint("/tmp/noext"), None);
    }

    #[tokio::test]
    async fn sniffs_archive_formats_from_file_headers() {
        let directory = tempfile::tempdir().expect("temp dir");
        let cases: [(&str, &[u8], ArchiveExtractionFormat); 6] = [
            ("a.zip", b"PK\x03\x04rest", ArchiveExtractionFormat::Zip),
            ("empty.zip", b"PK\x05\x06rest", ArchiveExtractionFormat::Zip),
            (
                "a.7z",
                b"7z\xbc\xaf\x27\x1crest",
                ArchiveExtractionFormat::SevenZip,
            ),
            (
                "a.rar",
                b"Rar!\x1a\x07\x00rest",
                ArchiveExtractionFormat::Rar,
            ),
            ("a.tgz", b"\x1f\x8brest", ArchiveExtractionFormat::Tar),
            (
                "a.datpack",
                b"not a known archive",
                ArchiveExtractionFormat::Tar,
            ),
        ];
        for (name, bytes, expected) in cases {
            let path = directory.path().join(name);
            std::fs::write(&path, bytes).expect("write fixture");
            assert_eq!(
                sniff_archive_extraction_format(&path).await,
                expected,
                "sniffing {name}"
            );
        }
    }
}
