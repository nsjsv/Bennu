use std::ffi::OsStr;
#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::process::ExitStatusExt;

use super::*;
use crate::archive_listing::StoredMember;
use crate::{ArchivePassword, FileKind};

fn seven_zip_test_request(password: Option<ArchivePassword>) -> ArchiveExtractionRequest {
    ArchiveExtractionRequest {
        archive: PathBuf::from("/tmp/locked.7z"),
        destination: PathBuf::from("/tmp/locked"),
        password,
    }
}

fn seven_zip_exit_status(code: i32) -> std::process::ExitStatus {
    std::process::ExitStatus::from_raw(code << 8)
}

#[test]
fn seven_zip_extract_output_switch_precedes_archive_operand() {
    let request = seven_zip_test_request(None);
    let command = seven_zip_extract_command(Path::new("7z"), &request);
    let arguments = command
        .as_std()
        .get_args()
        .map(OsStr::to_os_string)
        .collect::<Vec<_>>();
    let output_switch = seven_zip_output_directory_switch(&request.destination);
    let output_index = arguments
        .iter()
        .position(|argument| argument == &output_switch)
        .unwrap();
    let archive_separator_index = arguments
        .iter()
        .position(|argument| argument == OsStr::new("--"))
        .unwrap();

    assert!(output_index < archive_separator_index);
    assert_eq!(
        arguments.get(archive_separator_index + 1),
        Some(&request.archive.as_os_str().to_os_string())
    );
}

#[cfg(unix)]
#[test]
fn seven_zip_extract_output_switch_preserves_destination_bytes() {
    let destination_bytes = b"/tmp/archive-parent-\x80/output".to_vec();
    let mut expected_bytes = b"-o".to_vec();
    expected_bytes.extend_from_slice(&destination_bytes);

    for extension in ["7z", "rar"] {
        let mut request = seven_zip_test_request(None);
        request.archive = PathBuf::from(format!("/tmp/locked.{extension}"));
        request.destination = PathBuf::from(OsString::from_vec(destination_bytes.clone()));

        let command = seven_zip_extract_command(Path::new("7z"), &request);
        let output_switch = command
            .as_std()
            .get_args()
            .find(|argument| argument.as_bytes().starts_with(b"-o"))
            .expect("output directory switch");

        assert_eq!(output_switch.as_bytes(), expected_bytes);
    }
}

#[test]
fn seven_zip_stdout_password_prompt_requires_password() {
    let request = seven_zip_test_request(None);
    let error = seven_zip_failure(
        &request.archive,
        request.password.is_some(),
        seven_zip_exit_status(255),
        b"Enter password:",
        b"Break signaled",
    );

    assert!(matches!(
        error,
        FileError::ArchivePasswordRequired { path } if path == request.archive
    ));
}

#[test]
fn seven_zip_wrong_password_reports_invalid_password() {
    let request = seven_zip_test_request(ArchivePassword::new("wrong"));
    let error = seven_zip_failure(
        &request.archive,
        request.password.is_some(),
        seven_zip_exit_status(2),
        b"",
        b"Cannot open encrypted archive. Wrong password?",
    );

    assert!(matches!(
        error,
        FileError::ArchiveInvalidPassword { path } if path == request.archive
    ));
}

fn listed(path: &str, len: u64, encrypted: bool, block: Option<u64>) -> SevenZipListedEntry {
    SevenZipListedEntry {
        member: StoredMember {
            stored_path: path.as_bytes().to_vec(),
            kind: FileKind::File,
            len,
            modified: None,
        },
        encrypted,
        block,
    }
}

/// solid block 只能从块头按顺序解：块尾的小文件要先解完前面的大文件，
/// 代价反而最大。试解要挑“块内到它为止”累计最少的那个。
#[test]
fn password_probe_picks_the_cheapest_encrypted_file_to_decode() {
    let entries = vec![
        SevenZipListedEntry {
            member: StoredMember {
                stored_path: b"dir".to_vec(),
                kind: FileKind::Directory,
                len: 0,
                modified: None,
            },
            encrypted: false,
            block: None,
        },
        listed("empty.txt", 0, false, None),
        listed("big.bin", 3_000_000_000, true, Some(0)),
        listed("tiny-after-big.txt", 10, true, Some(0)),
        listed("medium-first-in-block.bin", 5_000, true, Some(1)),
        listed("small-second-in-block.txt", 100, true, Some(1)),
    ];

    assert_eq!(
        password_probe_entry(&entries).map(|entry| entry.member.stored_path.as_slice()),
        Some(&b"medium-first-in-block.bin"[..])
    );
    assert!(password_probe_entry(&entries[..2]).is_none());
}
