use std::ffi::OsString;
#[cfg(test)]
use std::fs;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use crate::ScanWarning;

use super::catalog::trash_object_identity;
use super::model::{OriginalPathBase, TrashObjectIdentity};

const MAX_TRASH_INFO_BYTES: u64 = 64 * 1024;

#[derive(Debug)]
pub(super) struct ParsedTrashInfo {
    pub original_path: PathBuf,
    pub deletion_date: Option<String>,
    pub identity: TrashObjectIdentity,
    pub warnings: Vec<ScanWarning>,
}

pub(super) fn normalize_new_volume_trash_info(
    info_path: &Path,
    expected_info_identity: &TrashObjectIdentity,
    top_directory: &Path,
    expected_original_path: &Path,
) -> Result<bool, ScanWarning> {
    let mut file = open_trash_info_for_update_no_follow(info_path).map_err(|error| {
        warning(
            info_path,
            format!("cannot open new volume .trashinfo safely: {error}"),
        )
    })?;
    let before = file
        .metadata()
        .map_err(|error| warning(info_path, format!("cannot inspect .trashinfo: {error}")))?;
    if !before.file_type().is_file() {
        return Err(warning(info_path, ".trashinfo is not a regular file"));
    }
    let before_identity = trash_object_identity(&before);
    if !before_identity.same_object(expected_info_identity) {
        return Err(warning(
            info_path,
            ".trashinfo identity changed before it could be normalized",
        ));
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(MAX_TRASH_INFO_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| warning(info_path, format!("cannot read .trashinfo: {error}")))?;
    if bytes.len() as u64 > MAX_TRASH_INFO_BYTES {
        return Err(warning(
            info_path,
            format!(".trashinfo exceeds {MAX_TRASH_INFO_BYTES} bytes"),
        ));
    }
    let fields = parse_trash_info_fields(info_path, &bytes)?;
    let decoded = percent_decode(&fields.path)
        .map_err(|message| warning(info_path, format!("invalid Path value: {message}")))?;
    let absolute_path = path_from_bytes(decoded);
    if !absolute_path.is_absolute() || absolute_path != expected_original_path {
        return Ok(false);
    }
    let relative = expected_original_path
        .strip_prefix(top_directory)
        .map_err(|_| {
            warning(
                info_path,
                "volume Path is outside the mounted top directory",
            )
        })?;
    let relative = validate_path_components(relative, PathForm::Relative)
        .map_err(|message| warning(info_path, format!("invalid Path value: {message}")))?;
    let encoded_relative = percent_encode_path(&relative);
    let mut normalized = Vec::new();
    normalized.extend_from_slice(b"[Trash Info]\nPath=");
    normalized.extend_from_slice(&encoded_relative);
    normalized.push(b'\n');
    if let Some(deletion_date) = fields.deletion_date {
        normalized.extend_from_slice(b"DeletionDate=");
        normalized.extend_from_slice(&deletion_date);
        normalized.push(b'\n');
    }

    file.seek(SeekFrom::Start(0))
        .and_then(|_| file.write_all(&normalized))
        .and_then(|_| file.set_len(normalized.len() as u64))
        .and_then(|_| file.sync_all())
        .map_err(|error| warning(info_path, format!("cannot normalize .trashinfo: {error}")))?;
    let after = file.metadata().map_err(|error| {
        warning(
            info_path,
            format!("cannot re-inspect normalized .trashinfo: {error}"),
        )
    })?;
    if !trash_object_identity(&after).same_object(&before_identity) {
        return Err(warning(
            info_path,
            ".trashinfo identity changed while it was normalized",
        ));
    }
    let path_identity = std::fs::symlink_metadata(info_path)
        .map(|metadata| trash_object_identity(&metadata))
        .map_err(|error| {
            warning(
                info_path,
                format!("cannot re-open normalized .trashinfo path: {error}"),
            )
        })?;
    if !path_identity.same_object(&before_identity) {
        return Err(warning(
            info_path,
            ".trashinfo path was replaced while it was normalized",
        ));
    }
    Ok(true)
}

fn percent_encode_path(path: &Path) -> Vec<u8> {
    let mut encoded = Vec::new();
    for byte in path_bytes(path) {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte);
        } else {
            encoded.push(b'%');
            encoded.push(b"0123456789ABCDEF"[(byte >> 4) as usize]);
            encoded.push(b"0123456789ABCDEF"[(byte & 0x0f) as usize]);
        }
    }
    encoded
}

pub(super) fn read_trash_info(
    info_path: &Path,
    base: OriginalPathBase,
    top_directory: &Path,
) -> Result<ParsedTrashInfo, ScanWarning> {
    let mut file = open_trash_info_no_follow(info_path)
        .map_err(|error| warning(info_path, format!("cannot open .trashinfo safely: {error}")))?;
    let before = file
        .metadata()
        .map_err(|error| warning(info_path, format!("cannot inspect .trashinfo: {error}")))?;
    if !before.file_type().is_file() {
        return Err(warning(info_path, ".trashinfo is not a regular file"));
    }
    let identity = trash_object_identity(&before);
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(MAX_TRASH_INFO_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| warning(info_path, format!("cannot read .trashinfo: {error}")))?;
    if bytes.len() as u64 > MAX_TRASH_INFO_BYTES {
        return Err(warning(
            info_path,
            format!(".trashinfo exceeds {MAX_TRASH_INFO_BYTES} bytes"),
        ));
    }
    let after = file
        .metadata()
        .map_err(|error| warning(info_path, format!("cannot re-inspect .trashinfo: {error}")))?;
    if trash_object_identity(&after) != identity {
        return Err(warning(info_path, ".trashinfo changed while it was read"));
    }

    let fields = parse_trash_info_fields(info_path, &bytes)?;
    let decoded_path = percent_decode(&fields.path)
        .map_err(|message| warning(info_path, format!("invalid Path value: {message}")))?;
    let original_path = resolve_original_path(base, top_directory, decoded_path)
        .map_err(|message| warning(info_path, format!("invalid Path value: {message}")))?;
    let mut warnings = Vec::new();
    let deletion_date = match fields.deletion_date {
        Some(value) if valid_deletion_date(&value) => Some(
            String::from_utf8(value)
                .expect("validated trash deletion date contains only ASCII bytes"),
        ),
        Some(_) => {
            warnings.push(warning(info_path, "invalid DeletionDate value"));
            None
        }
        None => {
            warnings.push(warning(info_path, "missing DeletionDate value"));
            None
        }
    };

    Ok(ParsedTrashInfo {
        original_path,
        deletion_date,
        identity,
        warnings,
    })
}

struct TrashInfoFields {
    path: Vec<u8>,
    deletion_date: Option<Vec<u8>>,
}

fn parse_trash_info_fields(info_path: &Path, bytes: &[u8]) -> Result<TrashInfoFields, ScanWarning> {
    let mut lines = bytes.split(|byte| *byte == b'\n');
    let header = trim_carriage_return(lines.next().unwrap_or_default());
    if header != b"[Trash Info]" {
        return Err(warning(info_path, "missing [Trash Info] header"));
    }

    let mut path = None;
    let mut deletion_date = None;
    for line in lines {
        let line = trim_carriage_return(line);
        if line.is_empty() {
            continue;
        }
        let Some(separator) = line.iter().position(|byte| *byte == b'=') else {
            continue;
        };
        let (key, value_with_separator) = line.split_at(separator);
        let value = &value_with_separator[1..];
        match key {
            b"Path" if path.is_none() => path = Some(value.to_vec()),
            b"Path" => return Err(warning(info_path, "duplicate Path value")),
            b"DeletionDate" if deletion_date.is_none() => {
                deletion_date = Some(value.to_vec());
            }
            b"DeletionDate" => {
                return Err(warning(info_path, "duplicate DeletionDate value"));
            }
            _ => {}
        }
    }

    let path = path.ok_or_else(|| warning(info_path, "missing Path value"))?;
    if path.is_empty() {
        return Err(warning(info_path, "empty Path value"));
    }
    Ok(TrashInfoFields {
        path,
        deletion_date,
    })
}

fn percent_decode(encoded: &[u8]) -> Result<Vec<u8>, &'static str> {
    let mut decoded = Vec::with_capacity(encoded.len());
    let mut index = 0;
    while index < encoded.len() {
        let value = if encoded[index] == b'%' {
            let high = *encoded.get(index + 1).ok_or("truncated percent escape")?;
            let low = *encoded.get(index + 2).ok_or("truncated percent escape")?;
            index += 3;
            decode_hex(high)
                .and_then(|high| decode_hex(low).map(|low| (high << 4) | low))
                .ok_or("non-hexadecimal percent escape")?
        } else {
            let value = encoded[index];
            index += 1;
            value
        };
        if value == 0 {
            return Err("decoded path contains a NUL byte");
        }
        decoded.push(value);
    }
    Ok(decoded)
}

fn decode_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn resolve_original_path(
    base: OriginalPathBase,
    top_directory: &Path,
    decoded: Vec<u8>,
) -> Result<PathBuf, &'static str> {
    let path = path_from_bytes(decoded);
    match base {
        OriginalPathBase::Absolute => {
            if !path.is_absolute() {
                return Err("Home Trash paths must be absolute");
            }
            validate_path_components(&path, PathForm::Absolute)?;
            Ok(path)
        }
        OriginalPathBase::RelativeToTopDirectory => {
            if path.is_absolute() {
                return Err("volume Trash paths must be relative");
            }
            let relative = validate_path_components(&path, PathForm::Relative)?;
            Ok(top_directory.join(relative))
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum PathForm {
    Absolute,
    Relative,
}

fn validate_path_components(path: &Path, form: PathForm) -> Result<PathBuf, &'static str> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir if matches!(form, PathForm::Absolute) => {
                normalized.push(component.as_os_str());
            }
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir => return Err("path contains a parent-directory component"),
            Component::RootDir => return Err("relative path contains a root component"),
            Component::Prefix(_) => return Err("path contains an unsupported prefix"),
        }
    }
    if normalized.as_os_str().is_empty() || normalized == Path::new("/") {
        return Err("path does not identify a file-system entry");
    }
    Ok(normalized)
}

fn valid_deletion_date(value: &[u8]) -> bool {
    if value.len() != 19 {
        return false;
    }
    if value[4] != b'-'
        || value[7] != b'-'
        || value[10] != b'T'
        || value[13] != b':'
        || value[16] != b':'
    {
        return false;
    }
    if value
        .iter()
        .enumerate()
        .any(|(index, byte)| !matches!(index, 4 | 7 | 10 | 13 | 16) && !byte.is_ascii_digit())
    {
        return false;
    }
    let number = |range: std::ops::Range<usize>| {
        value[range]
            .iter()
            .fold(0_u32, |number, digit| number * 10 + u32::from(digit - b'0'))
    };
    (1..=12).contains(&number(5..7))
        && (1..=31).contains(&number(8..10))
        && number(11..13) <= 23
        && number(14..16) <= 59
        && number(17..19) <= 59
}

fn trim_carriage_return(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\r").unwrap_or(line)
}

#[cfg(unix)]
fn open_trash_info_for_update_no_follow(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
}

#[cfg(not(unix))]
fn open_trash_info_for_update_no_follow(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).write(true).open(path)
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn path_bytes(path: &Path) -> Vec<u8> {
    path.as_os_str().to_string_lossy().as_bytes().to_vec()
}

#[cfg(unix)]
fn open_trash_info_no_follow(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
}

#[cfg(not(unix))]
fn open_trash_info_no_follow(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).open(path)
}

#[cfg(unix)]
fn path_from_bytes(bytes: Vec<u8>) -> PathBuf {
    PathBuf::from(OsString::from_vec(bytes))
}

#[cfg(not(unix))]
fn path_from_bytes(bytes: Vec<u8>) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(&bytes).into_owned())
}

fn warning(path: &Path, message: impl Into<String>) -> ScanWarning {
    ScanWarning {
        path: path.to_path_buf(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_info(path: &Path, content: &[u8]) {
        fs::write(path, content).unwrap();
    }

    #[test]
    fn volume_path_preserves_non_utf8_bytes_and_is_resolved_under_top_directory() {
        use std::os::unix::ffi::OsStrExt;

        let fixture = tempdir().unwrap();
        let info = fixture.path().join("entry.trashinfo");
        write_info(
            &info,
            b"[Trash Info]\nPath=folder/nonutf8-%FF.txt\nDeletionDate=2026-08-02T12:34:56\n",
        );

        let parsed = read_trash_info(
            &info,
            OriginalPathBase::RelativeToTopDirectory,
            Path::new("/media/volume"),
        )
        .unwrap();

        assert_eq!(
            parsed.original_path.as_os_str().as_bytes(),
            b"/media/volume/folder/nonutf8-\xff.txt"
        );
        assert_eq!(parsed.deletion_date.as_deref(), Some("2026-08-02T12:34:56"));
        assert!(parsed.warnings.is_empty());
    }

    #[test]
    fn malformed_percent_escapes_duplicates_and_path_escape_are_rejected() {
        let fixture = tempdir().unwrap();
        for (name, content) in [
            (
                "percent.trashinfo",
                b"[Trash Info]\nPath=/tmp/%ZZ\n".as_slice(),
            ),
            (
                "duplicate.trashinfo",
                b"[Trash Info]\nPath=/tmp/a\nPath=/tmp/b\n".as_slice(),
            ),
            (
                "escape.trashinfo",
                b"[Trash Info]\nPath=../outside\n".as_slice(),
            ),
        ] {
            let path = fixture.path().join(name);
            write_info(&path, content);
            let error = read_trash_info(
                &path,
                OriginalPathBase::RelativeToTopDirectory,
                fixture.path(),
            )
            .unwrap_err();
            assert!(error.message.contains("Path"));
        }
    }

    #[test]
    fn wrong_path_forms_and_invalid_header_are_rejected() {
        let fixture = tempdir().unwrap();
        for (name, content, base, expected_message) in [
            (
                "absolute-volume.trashinfo",
                b"[Trash Info]\nPath=/tmp/item\n".as_slice(),
                OriginalPathBase::RelativeToTopDirectory,
                "relative",
            ),
            (
                "relative-home.trashinfo",
                b"[Trash Info]\nPath=tmp/item\n".as_slice(),
                OriginalPathBase::Absolute,
                "absolute",
            ),
            (
                "header.trashinfo",
                b"[Wrong Section]\nPath=/tmp/item\n".as_slice(),
                OriginalPathBase::Absolute,
                "header",
            ),
        ] {
            let path = fixture.path().join(name);
            write_info(&path, content);
            let error = read_trash_info(&path, base, fixture.path()).unwrap_err();
            assert!(error.message.contains(expected_message));
        }
    }

    #[cfg(unix)]
    #[test]
    fn trash_info_symlinks_are_rejected_without_following_them() {
        use std::os::unix::fs::symlink;

        let fixture = tempdir().unwrap();
        let target = fixture.path().join("target");
        write_info(&target, b"[Trash Info]\nPath=/tmp/a\n");
        let link = fixture.path().join("link.trashinfo");
        symlink(&target, &link).unwrap();

        let error = read_trash_info(&link, OriginalPathBase::Absolute, fixture.path()).unwrap_err();

        assert!(error.message.contains("safely"));
    }

    #[cfg(unix)]
    #[test]
    fn newly_created_absolute_volume_info_is_normalized_to_relative_bytes() {
        use std::os::unix::ffi::OsStringExt;

        let fixture = tempdir().unwrap();
        let top = fixture.path().join("volume");
        fs::create_dir(&top).unwrap();
        let original = top.join(OsString::from_vec(b"folder/nonutf8-\xff.txt".to_vec()));
        let info = fixture.path().join("entry.trashinfo");
        let mut content = b"[Trash Info]\nPath=".to_vec();
        content.extend_from_slice(&percent_encode_path(&original));
        content.extend_from_slice(b"\nDeletionDate=2026-08-02T12:34:56\n");
        write_info(&info, &content);
        let expected_identity = trash_object_identity(&fs::symlink_metadata(&info).unwrap());

        assert!(
            normalize_new_volume_trash_info(&info, &expected_identity, &top, &original).unwrap()
        );
        let parsed =
            read_trash_info(&info, OriginalPathBase::RelativeToTopDirectory, &top).unwrap();

        assert_eq!(parsed.original_path, original);
        assert_eq!(parsed.deletion_date.as_deref(), Some("2026-08-02T12:34:56"));
        assert!(!fs::read(&info)
            .unwrap()
            .starts_with(b"[Trash Info]\nPath=/"));
    }

    #[cfg(unix)]
    #[test]
    fn normalization_rejects_an_info_path_replaced_after_discovery() {
        let fixture = tempdir().unwrap();
        let top = fixture.path().join("volume");
        fs::create_dir(&top).unwrap();
        let original = top.join("item.txt");
        let info = fixture.path().join("entry.trashinfo");
        write_info(
            &info,
            format!("[Trash Info]\nPath={}\n", original.display()).as_bytes(),
        );
        let expected_identity = trash_object_identity(&fs::symlink_metadata(&info).unwrap());
        fs::rename(&info, fixture.path().join("discovered.trashinfo")).unwrap();
        let replacement = b"[Trash Info]\nPath=/replacement\n";
        write_info(&info, replacement);

        let error = normalize_new_volume_trash_info(&info, &expected_identity, &top, &original)
            .unwrap_err();

        assert!(error.message.contains("identity changed"));
        assert_eq!(fs::read(info).unwrap(), replacement);
    }

    #[test]
    fn missing_or_invalid_deletion_date_is_a_warning_not_an_entry_failure() {
        let fixture = tempdir().unwrap();
        let info = fixture.path().join("entry.trashinfo");
        write_info(&info, b"[Trash Info]\nPath=/tmp/a\nDeletionDate=bad\n");

        let parsed = read_trash_info(&info, OriginalPathBase::Absolute, fixture.path()).unwrap();

        assert_eq!(parsed.deletion_date, None);
        assert_eq!(parsed.warnings.len(), 1);
    }
}
