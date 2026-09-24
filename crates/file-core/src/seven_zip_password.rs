//! 7z 密码类失败的共享分类:整包解压(archive_extraction)与包内成员
//! 提取(archive_vfs::extract)共同消费,避免两处输出解析各自漂移。

use std::path::Path;

use crate::FileError;

/// 7z 密码类失败的唯一分类口:输出含 password/encrypted 字样即判定为
/// 密码问题——已提供密码说明密码不对(ArchiveInvalidPassword),未提供
/// 说明需要密码(ArchivePasswordRequired)。不含密码字样时返回 None,
/// 由调用方走各自的普通错误包装。
pub(crate) fn seven_zip_password_error(
    combined_output: &str,
    password_provided: bool,
    archive: &Path,
) -> Option<FileError> {
    let lower = combined_output.to_ascii_lowercase();
    if !(lower.contains("password") || lower.contains("encrypted")) {
        return None;
    }
    Some(if password_provided {
        FileError::ArchiveInvalidPassword {
            path: archive.to_path_buf(),
        }
    } else {
        FileError::ArchivePasswordRequired {
            path: archive.to_path_buf(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// 成员级提取与整包解压共用同一个分类 helper:加密 rar 空密码提取时
    /// 7z 只在 stderr 给 "Wrong password" 且退出码 2,未提供密码必须归为
    /// 「需要密码」而不是裸 exit status。
    #[test]
    fn member_wrong_password_output_without_password_requires_password() {
        let archive = PathBuf::from("/tmp/locked.rar");
        let error = seven_zip_password_error(
            "ERROR: Cannot open encrypted archive. Wrong password?",
            false,
            &archive,
        );

        assert!(matches!(
            error,
            Some(FileError::ArchivePasswordRequired { path }) if path == archive
        ));
    }

    /// 同样的输出,调用方已提供密码时说明密码本身不对,上层据此
    /// 重新弹窗并提示 Incorrect password。
    #[test]
    fn member_wrong_password_output_with_password_reports_invalid_password() {
        let archive = PathBuf::from("/tmp/locked.rar");
        let error = seven_zip_password_error(
            "ERROR: Cannot open encrypted archive. Wrong password?",
            true,
            &archive,
        );

        assert!(matches!(
            error,
            Some(FileError::ArchiveInvalidPassword { path }) if path == archive
        ));
    }

    /// encrypted 字样同样命中分类:部分 7z 版本/场景输出不含 password 一词。
    #[test]
    fn member_encrypted_output_is_classified_as_password_error() {
        let archive = PathBuf::from("/tmp/data.7z");
        let error = seven_zip_password_error(
            "ERROR: Data Error in encrypted file. Broken file?",
            false,
            &archive,
        );

        assert!(matches!(
            error,
            Some(FileError::ArchivePasswordRequired { path }) if path == archive
        ));
    }

    /// 非密码失败(如分卷 Headers Error)必须返回 None,让调用方走
    /// 普通错误包装,不能被误报成密码问题。
    #[test]
    fn member_unrelated_output_is_not_classified_as_password_error() {
        let error = seven_zip_password_error(
            "ERROR: Headers Error",
            false,
            &PathBuf::from("/tmp/volume.r00"),
        );

        assert!(error.is_none());
    }
}
