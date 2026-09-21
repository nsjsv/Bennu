use std::io;
use std::path::Path;

use tokio::fs;
use tokio::io::AsyncReadExt;

use crate::FileError;

use super::copy::FileOperationControls;

pub(super) async fn verify_copied_file(
    from: &Path,
    to: &Path,
    source_metadata: &std::fs::Metadata,
    controls: &mut FileOperationControls,
    buffer: &mut [u8],
    expected_content_hash: Option<blake3::Hash>,
) -> Result<(), FileError> {
    controls.wait_until_running().await?;
    let target_metadata = fs::symlink_metadata(to)
        .await
        .map_err(|source| FileError::Copy {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
            source,
        })?;
    if !target_metadata.file_type().is_file() {
        return Err(copy_verification_error(
            from,
            to,
            "target is not a regular file after copy",
        ));
    }
    if target_metadata.len() != source_metadata.len() {
        return Err(copy_verification_error(
            from,
            to,
            "target file size differs from source after copy",
        ));
    }
    if let Some(expected_content_hash) = expected_content_hash {
        let target_content_hash = copied_file_content_hash(from, to, controls, buffer).await?;
        if target_content_hash != expected_content_hash {
            return Err(copy_verification_error(
                from,
                to,
                "target file content hash differs from source after copy",
            ));
        }
    }
    Ok(())
}

pub(super) async fn verify_copied_symbolic_link(
    from: &Path,
    to: &Path,
    expected_target: &Path,
) -> Result<(), FileError> {
    let target_metadata = fs::symlink_metadata(to)
        .await
        .map_err(|source| FileError::Copy {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
            source,
        })?;
    if !target_metadata.file_type().is_symlink() {
        return Err(copy_verification_error(
            from,
            to,
            "target is not a symbolic link after copy",
        ));
    }
    let copied_target = fs::read_link(to).await.map_err(|source| FileError::Copy {
        from: from.to_path_buf(),
        to: to.to_path_buf(),
        source,
    })?;
    if copied_target != expected_target {
        return Err(copy_verification_error(
            from,
            to,
            "symbolic link target differs from source after copy",
        ));
    }
    Ok(())
}

pub(super) async fn verify_copied_directory(from: &Path, to: &Path) -> Result<(), FileError> {
    let target_metadata = fs::symlink_metadata(to)
        .await
        .map_err(|source| FileError::Copy {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
            source,
        })?;
    if !target_metadata.file_type().is_dir() {
        return Err(copy_verification_error(
            from,
            to,
            "target is not a directory after copy",
        ));
    }
    Ok(())
}

fn copy_verification_error(from: &Path, to: &Path, message: &'static str) -> FileError {
    FileError::Copy {
        from: from.to_path_buf(),
        to: to.to_path_buf(),
        source: io::Error::new(io::ErrorKind::InvalidData, message),
    }
}

async fn copied_file_content_hash(
    from: &Path,
    to: &Path,
    controls: &mut FileOperationControls,
    buffer: &mut [u8],
) -> Result<blake3::Hash, FileError> {
    let mut target = fs::File::open(to).await.map_err(|source| FileError::Copy {
        from: from.to_path_buf(),
        to: to.to_path_buf(),
        source,
    })?;
    let mut target_content_hasher = blake3::Hasher::new();

    loop {
        controls.wait_until_running().await?;
        let read = target
            .read(buffer)
            .await
            .map_err(|source| FileError::Copy {
                from: from.to_path_buf(),
                to: to.to_path_buf(),
                source,
            })?;
        if read == 0 {
            return Ok(target_content_hasher.finalize());
        }
        target_content_hasher.update(&buffer[..read]);
    }
}

#[cfg(test)]
#[path = "copy/tests.rs"]
mod tests;
