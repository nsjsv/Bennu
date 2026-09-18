use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::SystemTime;

use tokio::process::Command;

use crate::{
    archive_extraction_format_for_path, ArchiveExtractionFormat, FileError, FileKind,
    SEVEN_ZIP_COMMAND_NAMES,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveListingEntry {
    pub path: String,
    pub kind: FileKind,
    /// 未压缩大小；目录为 0。虚拟目录的 Size 排序与属性面板消费。
    pub len: u64,
    /// 归档头记录的修改时间；7z 外部命令不解析时区敏感的 MTime，保持 None。
    pub modified: Option<SystemTime>,
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

    match format {
        ArchiveExtractionFormat::Zip => {
            spawn_archive_listing(archive, |archive| read_zip_members(&archive)).await
        }
        ArchiveExtractionFormat::Tar => {
            spawn_archive_listing(archive, |archive| {
                read_tar_members(&archive, TarCompression::Plain)
            })
            .await
        }
        ArchiveExtractionFormat::TarGz => {
            spawn_archive_listing(archive, |archive| {
                read_tar_members(&archive, TarCompression::Gzip)
            })
            .await
        }
        ArchiveExtractionFormat::SevenZip | ArchiveExtractionFormat::Rar => {
            read_seven_zip_members(&archive, archive_format_label(format)).await
        }
    }
}

/// 读取文件头识别归档格式。调用方已确认该路径属于归档类型，
/// 无法识别魔数时按 tar 兜底——识别错误由解压/列成员环节报错。
pub async fn sniff_archive_extraction_format(
    archive: impl Into<PathBuf>,
) -> ArchiveExtractionFormat {
    let archive = archive.into();
    let Some(header) = read_archive_header(&archive).await else {
        return ArchiveExtractionFormat::Tar;
    };

    if header.starts_with(&[0x50, 0x4b, 0x03, 0x04]) {
        return ArchiveExtractionFormat::Zip;
    }
    if header.starts_with(&[0x1f, 0x8b]) {
        // gzip 单文件归档不在预览支持范围，维持内置 .tar.gz 语义。
        return ArchiveExtractionFormat::TarGz;
    }
    if header.starts_with(&[0x37, 0x7a, 0xbc, 0xaf, 0x27, 0x1c]) {
        return ArchiveExtractionFormat::SevenZip;
    }
    if header.starts_with(&[0x52, 0x61, 0x72, 0x21, 0x1a, 0x07, 0x00]) {
        return ArchiveExtractionFormat::Rar;
    }
    // ustar 魔数位于偏移 257；旧式 tar 无魔数，靠上面的兜底处理。
    if header.len() >= 262 && &header[257..262] == b"ustar" {
        return ArchiveExtractionFormat::Tar;
    }
    ArchiveExtractionFormat::Tar
}

async fn read_archive_header(path: &Path) -> Option<Vec<u8>> {
    use tokio::io::AsyncReadExt;

    let mut file = tokio::fs::File::open(path).await.ok()?;
    let mut header = [0u8; 512];
    let read = file.read(&mut header).await.ok()?;
    Some(header[..read].to_vec())
}

async fn spawn_archive_listing(
    archive: PathBuf,
    read_members: impl FnOnce(PathBuf) -> Result<Vec<ArchiveListingEntry>, FileError> + Send + 'static,
) -> Result<Vec<ArchiveListingEntry>, FileError> {
    let join_path = archive.clone();
    tokio::task::spawn_blocking(move || read_members(archive))
        .await
        .map_err(|error| FileError::Archive {
            path: join_path,
            message: error.to_string(),
        })?
}

fn read_zip_members(path: &Path) -> Result<Vec<ArchiveListingEntry>, FileError> {
    let file = open_archive_file(path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|source| FileError::Archive {
        path: path.to_path_buf(),
        message: source.to_string(),
    })?;
    let mut members = Vec::with_capacity(archive.len());

    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|source| FileError::Archive {
                path: path.to_path_buf(),
                message: source.to_string(),
            })?;
        let kind = if entry.is_dir() {
            FileKind::Directory
        } else if entry.is_symlink() {
            FileKind::Symlink
        } else {
            FileKind::File
        };
        members.push(ArchiveListingEntry {
            path: entry.name().to_owned(),
            kind,
            len: entry.size(),
            modified: zip_datetime_to_system_time(entry.last_modified()),
        });
    }

    Ok(members)
}

fn read_tar_members(
    path: &Path,
    compression: TarCompression,
) -> Result<Vec<ArchiveListingEntry>, FileError> {
    let file = open_archive_file(path)?;
    match compression {
        TarCompression::Plain => read_tar_members_from(path, file),
        TarCompression::Gzip => read_tar_members_from(path, flate2::read::GzDecoder::new(file)),
    }
}

fn read_tar_members_from<R: io::Read>(
    archive_path: &Path,
    reader: R,
) -> Result<Vec<ArchiveListingEntry>, FileError> {
    let mut archive = tar::Archive::new(reader);
    let entries = archive.entries().map_err(|source| FileError::Archive {
        path: archive_path.to_path_buf(),
        message: source.to_string(),
    })?;
    let mut members = Vec::new();

    for entry_outcome in entries {
        let entry = entry_outcome.map_err(|source| FileError::Archive {
            path: archive_path.to_path_buf(),
            message: source.to_string(),
        })?;
        let entry_type = entry.header().entry_type();
        let kind = if entry_type.is_dir() {
            FileKind::Directory
        } else if entry_type.is_symlink() {
            FileKind::Symlink
        } else if entry_type.is_file() {
            FileKind::File
        } else {
            FileKind::Other
        };
        let path = entry
            .path()
            .map_err(|source| FileError::Archive {
                path: archive_path.to_path_buf(),
                message: source.to_string(),
            })?
            .to_string_lossy()
            .into_owned();
        let header = entry.header();
        let modified = header
            .mtime()
            .ok()
            .map(|seconds| SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(seconds));
        members.push(ArchiveListingEntry {
            path,
            kind,
            len: header.size().unwrap_or(0),
            modified,
        });
    }

    Ok(members)
}

async fn read_seven_zip_members(
    path: &Path,
    format_label: &str,
) -> Result<Vec<ArchiveListingEntry>, FileError> {
    for command_name in SEVEN_ZIP_COMMAND_NAMES {
        let output = Command::new(command_name)
            .arg("l")
            .arg("-slt")
            .arg("-bd")
            .arg("-bsp0")
            .arg("--")
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output()
            .await;
        let output = match output {
            Ok(output) => output,
            Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(FileError::Archive {
                    path: path.to_path_buf(),
                    message: format!("could not run {command_name} for {format_label}: {source}"),
                });
            }
        };

        if output.status.success() {
            let listing = String::from_utf8_lossy(&output.stdout);
            return Ok(parse_seven_zip_listing(&listing));
        }

        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let message = if stderr.is_empty() {
            format!(
                "could not list {format_label} archive with {command_name}: {}",
                output.status
            )
        } else {
            format!("could not list {format_label} archive with {command_name}: {stderr}")
        };
        return Err(FileError::Archive {
            path: path.to_path_buf(),
            message,
        });
    }

    Err(FileError::Unsupported(
        "7z, 7zz or 7za command is required to preview this archive",
    ))
}

fn parse_seven_zip_listing(technical_listing: &str) -> Vec<ArchiveListingEntry> {
    let mut members = Vec::new();
    let mut in_archive_entries = false;
    let mut current_entry: Option<SevenZipListedEntry> = None;

    for line in technical_listing.lines() {
        if line.trim() == "----------" {
            in_archive_entries = true;
            continue;
        }
        if !in_archive_entries {
            continue;
        }
        if line.trim().is_empty() {
            push_listed_archive_member(current_entry.take(), &mut members);
            continue;
        }

        if let Some(path) = line.strip_prefix("Path = ") {
            push_listed_archive_member(current_entry.take(), &mut members);
            current_entry = Some(SevenZipListedEntry {
                path: path.to_owned(),
                is_directory: path.ends_with('/') || path.ends_with('\\'),
                len: 0,
            });
            continue;
        }

        let Some(entry) = current_entry.as_mut() else {
            continue;
        };
        if let Some(folder) = line.strip_prefix("Folder = ") {
            entry.is_directory |= seven_zip_folder_field_is_directory(folder);
        } else if let Some(attributes) = line.strip_prefix("Attributes = ") {
            entry.is_directory |= seven_zip_attributes_field_is_directory(attributes);
        } else if let Some(size) = line.strip_prefix("Size = ") {
            entry.len = size.trim().parse().unwrap_or(0);
        }
    }

    push_listed_archive_member(current_entry, &mut members);
    members
}

fn push_listed_archive_member(
    listed_entry: Option<SevenZipListedEntry>,
    members: &mut Vec<ArchiveListingEntry>,
) {
    let Some(listed_entry) = listed_entry else {
        return;
    };
    if listed_entry.path.is_empty() {
        return;
    }

    members.push(ArchiveListingEntry {
        path: listed_entry.path,
        kind: if listed_entry.is_directory {
            FileKind::Directory
        } else {
            FileKind::File
        },
        len: listed_entry.len,
        modified: None,
    });
}

fn seven_zip_folder_field_is_directory(folder: &str) -> bool {
    let folder = folder.trim();
    folder == "+" || folder.eq_ignore_ascii_case("true")
}

fn seven_zip_attributes_field_is_directory(attributes: &str) -> bool {
    attributes.trim_start().starts_with('D')
}

fn archive_format_label(format: ArchiveExtractionFormat) -> &'static str {
    match format {
        ArchiveExtractionFormat::Zip => "zip",
        ArchiveExtractionFormat::Tar => "tar",
        ArchiveExtractionFormat::TarGz => "tar.gz",
        ArchiveExtractionFormat::SevenZip => "7z",
        ArchiveExtractionFormat::Rar => "rar",
    }
}

fn open_archive_file(path: &Path) -> Result<File, FileError> {
    File::open(path).map_err(|source| FileError::Archive {
        path: path.to_path_buf(),
        message: source.to_string(),
    })
}

#[derive(Debug, Clone, Copy)]
enum TarCompression {
    Plain,
    Gzip,
}

struct SevenZipListedEntry {
    path: String,
    is_directory: bool,
    len: u64,
}

/// zip 的 MS-DOS 时间戳按 UTC 解释成 `SystemTime`。DOS 时间本身不
/// 携带时区，主流工具（info-zip）同样按 UTC 存取；显示语义足够。
fn zip_datetime_to_system_time(datetime: Option<zip::DateTime>) -> Option<SystemTime> {
    fn civil_days_since_epoch(year: i64, month: u32, day: u32) -> i64 {
        let shifted_year = year - i64::from(month <= 2);
        let era = shifted_year.div_euclid(400);
        let year_of_era = shifted_year - era * 400;
        let month_pattern = (month + 9) % 12;
        let day_of_year = (153 * i64::from(month_pattern) + 2) / 5 + i64::from(day) - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        era * 146_097 + day_of_era - 719_468
    }

    let datetime = datetime?;
    let (year, month, day) = (i64::from(datetime.year()), datetime.month(), datetime.day());
    if year < 1970 {
        return None;
    }
    let seconds = civil_days_since_epoch(year, u32::from(month), u32::from(day)) * 86_400
        + i64::from(datetime.hour()) * 3_600
        + i64::from(datetime.minute()) * 60
        + i64::from(datetime.second());
    Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(seconds as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_seven_zip_listing_reads_directory_markers() {
        let members = parse_seven_zip_listing(
            r#"
Path = archive.rar
Type = Rar

----------
Path = src
Folder = +
Attributes = D_ drwxr-xr-x

Path = src/main.rs
Folder = -
Attributes = A_ -rw-r--r--

Path = docs\guide.md
Folder = -
Attributes = A_ -rw-r--r--
"#,
        );

        assert_eq!(members.len(), 3);
        assert_eq!(members[0].path, "src");
        assert_eq!(members[0].kind, FileKind::Directory);
        assert_eq!(members[1].path, "src/main.rs");
        assert_eq!(members[1].kind, FileKind::File);
        assert_eq!(members[2].path, "docs\\guide.md");
        assert_eq!(members[2].kind, FileKind::File);
    }
}

#[cfg(test)]
mod sniff_tests {
    use super::*;

    #[tokio::test]
    async fn sniffs_archive_formats_from_file_headers() {
        let directory = tempfile::tempdir().expect("temp dir");
        let cases: Vec<(&str, Vec<u8>, ArchiveExtractionFormat)> = vec![
            (
                "a.zip",
                b"PK\x03\x04rest".to_vec(),
                ArchiveExtractionFormat::Zip,
            ),
            (
                "a.tgz",
                [0x1f, 0x8b]
                    .iter()
                    .copied()
                    .chain(b"rest".iter().copied())
                    .collect(),
                ArchiveExtractionFormat::TarGz,
            ),
            (
                "a.7z",
                [0x37, 0x7a, 0xbc, 0xaf, 0x27, 0x1c]
                    .iter()
                    .copied()
                    .chain(b"rest".iter().copied())
                    .collect(),
                ArchiveExtractionFormat::SevenZip,
            ),
            (
                "a.rar",
                [0x52, 0x61, 0x72, 0x21, 0x1a, 0x07, 0x00]
                    .iter()
                    .copied()
                    .chain(b"rest".iter().copied())
                    .collect(),
                ArchiveExtractionFormat::Rar,
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

    #[tokio::test]
    async fn sniff_falls_back_to_tar_for_unrecognized_headers() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("a.datpack");
        std::fs::write(&path, b"not a known archive").expect("write fixture");
        assert_eq!(
            sniff_archive_extraction_format(&path).await,
            ArchiveExtractionFormat::Tar
        );
    }
}
