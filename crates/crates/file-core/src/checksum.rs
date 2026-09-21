//! 文件校验值计算与校验文件解析。
//!
//! 引擎在一次读取中同时喂入五个 hasher,调用方通过 mpsc 接收节流后的
//! 进度上报,并通过取消令牌中止读取;解析器负责 md5sum/sha256sum 标准
//! 格式与裸单值校验文件,校验结果比对逻辑与 UI 无关,便于独立测试。

use std::path::Path;
use std::path::PathBuf;

use md5::{Digest, Md5};
use sha1::Sha1;
use sha2::{Sha256, Sha512};
use tokio::fs;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::FileError;

/// 读取块大小:兼顾小文件的调度开销与大文件的内存占用。
const READ_CHUNK_BYTES: usize = 256 * 1024;
/// 进度上报节流间隔:避免高频 send 拖慢 UI 消息循环。
const PROGRESS_SEND_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);
/// 受支持算法的摘要十六进制长度;64 同时命中 SHA-256 与默认 256 位输出的 BLAKE3。
const DIGEST_HEX_LENGTHS: [usize; 4] = [32, 40, 64, 128];

pub const ALL_CHECKSUM_ALGORITHMS: [ChecksumAlgorithm; 5] = [
    ChecksumAlgorithm::Md5,
    ChecksumAlgorithm::Sha1,
    ChecksumAlgorithm::Sha256,
    ChecksumAlgorithm::Sha512,
    ChecksumAlgorithm::Blake3,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChecksumAlgorithm {
    Md5,
    Sha1,
    Sha256,
    Sha512,
    Blake3,
}

impl ChecksumAlgorithm {
    pub fn label(self) -> &'static str {
        match self {
            Self::Md5 => "MD5",
            Self::Sha1 => "SHA-1",
            Self::Sha256 => "SHA-256",
            Self::Sha512 => "SHA-512",
            Self::Blake3 => "BLAKE3",
        }
    }

    pub fn hex_length(self) -> usize {
        match self {
            Self::Md5 => 32,
            Self::Sha1 => 40,
            Self::Sha256 | Self::Blake3 => 64,
            Self::Sha512 => 128,
        }
    }
}

/// 单个文件一次计算出的全部算法摘要(十六进制小写)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChecksums {
    md5: String,
    sha1: String,
    sha256: String,
    sha512: String,
    blake3: String,
}

impl FileChecksums {
    pub fn digest(&self, algorithm: ChecksumAlgorithm) -> &str {
        match algorithm {
            ChecksumAlgorithm::Md5 => &self.md5,
            ChecksumAlgorithm::Sha1 => &self.sha1,
            ChecksumAlgorithm::Sha256 => &self.sha256,
            ChecksumAlgorithm::Sha512 => &self.sha512,
            ChecksumAlgorithm::Blake3 => &self.blake3,
        }
    }
}

/// 读取进度;total_bytes 在文件元数据读取前为 0。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChecksumProgress {
    pub bytes_done: u64,
    pub total_bytes: u64,
}

/// 单趟读取文件并计算全部受支持算法的摘要。
///
/// 进度发送失败按既有约定忽略(进度通道不是操作成败的一部分);
/// 取消后返回 [`FileError::Cancelled`],由调用方决定如何呈现。
pub async fn compute_file_checksums(
    path: PathBuf,
    progress: mpsc::Sender<ChecksumProgress>,
    cancel: CancellationToken,
) -> Result<FileChecksums, FileError> {
    let mut file = fs::File::open(&path)
        .await
        .map_err(|source| checksum_io_error(&path, source))?;
    let total_bytes = file
        .metadata()
        .await
        .map_err(|source| checksum_io_error(&path, source))?
        .len();

    let mut md5_hasher = Md5::new();
    let mut sha1_hasher = Sha1::new();
    let mut sha256_hasher = Sha256::new();
    let mut sha512_hasher = Sha512::new();
    let mut blake3_hasher = blake3::Hasher::new();

    let mut buffer = vec![0u8; READ_CHUNK_BYTES];
    let mut bytes_done: u64 = 0;
    let mut last_sent = tokio::time::Instant::now() - PROGRESS_SEND_INTERVAL;
    loop {
        if cancel.is_cancelled() {
            return Err(FileError::Cancelled);
        }
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|source| checksum_io_error(&path, source))?;
        if read == 0 {
            break;
        }
        let chunk = &buffer[..read];
        md5_hasher.update(chunk);
        sha1_hasher.update(chunk);
        sha256_hasher.update(chunk);
        sha512_hasher.update(chunk);
        blake3_hasher.update(chunk);
        bytes_done += read as u64;
        if last_sent.elapsed() >= PROGRESS_SEND_INTERVAL {
            let _ = progress
                .send(ChecksumProgress {
                    bytes_done,
                    total_bytes,
                })
                .await;
            last_sent = tokio::time::Instant::now();
        }
    }
    let _ = progress
        .send(ChecksumProgress {
            bytes_done: total_bytes,
            total_bytes,
        })
        .await;

    Ok(FileChecksums {
        md5: hex_encode(&md5_hasher.finalize()),
        sha1: hex_encode(&sha1_hasher.finalize()),
        sha256: hex_encode(&sha256_hasher.finalize()),
        sha512: hex_encode(&sha512_hasher.finalize()),
        blake3: hex_encode(blake3_hasher.finalize().as_bytes()),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChecksumFileEntry {
    pub hash_hex: String,
    pub file_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChecksumFileContent {
    /// 标准 md5sum/sha256sum 格式的一到多条目。
    Entries(Vec<ChecksumFileEntry>),
    /// 整个文件只有一个裸哈希值,直接当期望值使用。
    BareHash(String),
}

/// 解析校验文件文本。
///
/// 行格式 `哈希  文件名`(标准双空格;容忍单空格/制表符)、`哈希 *文件名`
/// (二进制模式标记)与 `#`/`;` 注释行;文件名里的空格保留。只有当所有
/// 行都解析不出条目、且整个文件恰好是一个合法长度的十六进制串时,才识别
/// 为裸单值。
pub fn parse_checksum_file(text: &str) -> Result<ChecksumFileContent, String> {
    let mut entries = Vec::new();
    for (line_index, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim_end_matches('\r').trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let Some((hash_text, file_name)) = split_entry_line(line) else {
            // 没有文件名部分的行(如裸哈希值)不在此报错,留给裸值判断。
            continue;
        };
        if !is_plausible_digest(hash_text) {
            return Err(format!(
                "unrecognized digest length on checksum file line {}",
                line_index + 1
            ));
        }
        entries.push(ChecksumFileEntry {
            hash_hex: hash_text.to_ascii_lowercase(),
            file_name: file_name.to_owned(),
        });
    }

    if !entries.is_empty() {
        return Ok(ChecksumFileContent::Entries(entries));
    }

    // 单行裸值:先于报错判断,避免把合法的裸值文件当成格式错误。
    let trimmed = text.trim();
    if is_plausible_digest(trimmed) {
        return Ok(ChecksumFileContent::BareHash(trimmed.to_ascii_lowercase()));
    }
    Err("checksum file contains no entries".to_owned())
}

/// 拆出条目的哈希与文件名;没有文件名部分时返回 None。
fn split_entry_line(line: &str) -> Option<(&str, &str)> {
    let (hash_text, rest) = line.split_once(char::is_whitespace)?;
    let file_name = rest.trim().strip_prefix('*').map_or(rest, |name| name);
    let file_name = file_name.trim();
    if file_name.is_empty() {
        return None;
    }
    Some((hash_text, file_name))
}

/// 期望值是否是任意受支持算法长度的合法十六进制摘要。
pub fn is_plausible_digest(expected: &str) -> bool {
    let trimmed = expected.trim();
    !trimmed.is_empty()
        && trimmed.bytes().all(|byte| byte.is_ascii_hexdigit())
        && DIGEST_HEX_LENGTHS.contains(&trimmed.len())
}

/// 计算结果中与期望值一致(不区分大小写)的算法;64 长度可能同时命中
/// SHA-256 与 BLAKE3,所以返回列表而不是单个算法。
pub fn algorithms_matching_digest(
    computed: &FileChecksums,
    expected: &str,
) -> Vec<ChecksumAlgorithm> {
    let trimmed = expected.trim();
    ALL_CHECKSUM_ALGORITHMS
        .into_iter()
        .filter(|algorithm| computed.digest(*algorithm).eq_ignore_ascii_case(trimmed))
        .collect()
}

/// 在校验文件条目中按文件名(比较 basename)查找当前文件的条目。
pub fn find_checksum_entry<'a>(
    entries: &'a [ChecksumFileEntry],
    file_path: &Path,
) -> Option<&'a ChecksumFileEntry> {
    let target_name = file_path.file_name()?;
    entries
        .iter()
        .find(|entry| Path::new(&entry.file_name).file_name() == Some(target_name))
}

fn checksum_io_error(path: &Path, source: std::io::Error) -> FileError {
    FileError::Checksum {
        path: path.to_path_buf(),
        source,
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "abc" 的标准测试向量,覆盖全部受支持算法。
    const ABC_DIGESTS: [(ChecksumAlgorithm, &str); 5] = [
        (
            ChecksumAlgorithm::Md5,
            "900150983cd24fb0d6963f7d28e17f72",
        ),
        (
            ChecksumAlgorithm::Sha1,
            "a9993e364706816aba3e25717850c26c9cd0d89d",
        ),
        (
            ChecksumAlgorithm::Sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        ),
        (
            ChecksumAlgorithm::Sha512,
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
        ),
        (
            ChecksumAlgorithm::Blake3,
            "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85",
        ),
    ];

    async fn write_temp_file(contents: &[u8]) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().expect("create temp file");
        std::io::Write::write_all(&mut file, contents).expect("write temp file");
        file
    }

    #[tokio::test]
    async fn compute_matches_known_vectors() {
        let file = write_temp_file(b"abc").await;
        let (progress_sender, mut progress_receiver) = mpsc::channel(4);
        let checksums = compute_file_checksums(
            file.path().to_path_buf(),
            progress_sender,
            CancellationToken::new(),
        )
        .await
        .expect("compute checksums");

        for (algorithm, expected) in ABC_DIGESTS {
            assert_eq!(
                checksums.digest(algorithm),
                expected,
                "{}",
                algorithm.label()
            );
        }
        // 计算完成前一定会发出 100% 的收尾进度。
        let final_progress = progress_receiver.recv().await.expect("final progress");
        assert_eq!(final_progress.bytes_done, 3);
        assert_eq!(final_progress.total_bytes, 3);
    }

    #[tokio::test]
    async fn cancel_aborts_computation() {
        let file = write_temp_file(b"abc").await;
        let (progress_sender, _progress_receiver) = mpsc::channel(4);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let outcome =
            compute_file_checksums(file.path().to_path_buf(), progress_sender, cancel).await;
        assert!(matches!(outcome, Err(FileError::Cancelled)));
    }

    #[tokio::test]
    async fn missing_file_returns_structured_error() {
        let (progress_sender, _progress_receiver) = mpsc::channel(4);
        let outcome = compute_file_checksums(
            PathBuf::from("/nonexistent/checksum-target"),
            progress_sender,
            CancellationToken::new(),
        )
        .await;
        assert!(matches!(outcome, Err(FileError::Checksum { .. })));
    }

    #[test]
    fn parse_accepts_standard_double_space_entries() {
        let content = parse_checksum_file(
            "900150983cd24fb0d6963f7d28e17f72  abc.txt\n# comment\n\nba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  def.iso\n",
        )
        .expect("parse entries");
        assert_eq!(
            content,
            ChecksumFileContent::Entries(vec![
                ChecksumFileEntry {
                    hash_hex: "900150983cd24fb0d6963f7d28e17f72".to_owned(),
                    file_name: "abc.txt".to_owned(),
                },
                ChecksumFileEntry {
                    hash_hex: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
                        .to_owned(),
                    file_name: "def.iso".to_owned(),
                },
            ])
        );
    }

    #[test]
    fn parse_accepts_binary_marker_single_space_and_crlf() {
        let content = parse_checksum_file(
            "900150983cd24fb0d6963f7d28e17f72 *abc.txt\r\n900150983cd24fb0d6963f7d28e17f72\tdef iso name.txt\r\n",
        )
        .expect("parse entries");
        assert_eq!(
            content,
            ChecksumFileContent::Entries(vec![
                ChecksumFileEntry {
                    hash_hex: "900150983cd24fb0d6963f7d28e17f72".to_owned(),
                    file_name: "abc.txt".to_owned(),
                },
                ChecksumFileEntry {
                    hash_hex: "900150983cd24fb0d6963f7d28e17f72".to_owned(),
                    file_name: "def iso name.txt".to_owned(),
                },
            ])
        );
    }

    #[test]
    fn parse_recognizes_bare_single_hash() {
        let content = parse_checksum_file("900150983CD24FB0D6963F7D28E17F72\n").expect("bare hash");
        assert_eq!(
            content,
            ChecksumFileContent::BareHash("900150983cd24fb0d6963f7d28e17f72".to_owned())
        );
    }

    #[test]
    fn parse_single_standard_entry_is_not_bare_hash() {
        let content =
            parse_checksum_file("900150983cd24fb0d6963f7d28e17f72  only.txt").expect("entry");
        assert_eq!(
            content,
            ChecksumFileContent::Entries(vec![ChecksumFileEntry {
                hash_hex: "900150983cd24fb0d6963f7d28e17f72".to_owned(),
                file_name: "only.txt".to_owned(),
            }])
        );
    }

    #[test]
    fn parse_rejects_unknown_digest_length_and_garbage() {
        assert!(parse_checksum_file("12345  abc.txt").is_err());
        assert!(parse_checksum_file("MD5 (abc.txt) = 900150983cd24fb0d6963f7d28e17f72").is_err());
        assert!(parse_checksum_file("").is_err());
    }

    #[test]
    fn digest_matching_is_case_insensitive_and_lists_ambiguous_length() {
        let sha256_hex = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let mut checksums = FileChecksums {
            md5: String::new(),
            sha1: String::new(),
            sha256: sha256_hex.to_owned(),
            sha512: String::new(),
            blake3: sha256_hex.to_owned(),
        };
        // SHA-256 与 BLAKE3 摘要长度相同:同时命中时按列表返回。
        checksums.blake3 = sha256_hex.to_owned();
        let matched = algorithms_matching_digest(&checksums, &sha256_hex.to_uppercase());
        assert_eq!(
            matched,
            vec![ChecksumAlgorithm::Sha256, ChecksumAlgorithm::Blake3]
        );
        assert!(
            algorithms_matching_digest(&checksums, "0000000000000000000000000000000f").is_empty()
        );
    }

    #[test]
    fn find_entry_compares_basenames() {
        let entries = vec![ChecksumFileEntry {
            hash_hex: "900150983cd24fb0d6963f7d28e17f72".to_owned(),
            file_name: "subdir/abc.txt".to_owned(),
        }];
        assert_eq!(
            find_checksum_entry(&entries, Path::new("/tmp/other/abc.txt")),
            Some(&entries[0])
        );
        assert_eq!(
            find_checksum_entry(&entries, Path::new("/tmp/other/xyz.txt")),
            None
        );
    }
}
