use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::archive_extraction::archive_extraction_format_for_path;
use crate::{
    ArchiveExtractionFormat, ArchiveExtractionProgress, ArchivePassword, FileError,
    FileOperationControls, SEVEN_ZIP_COMMAND_NAMES,
};

/// 批量提取请求：把包内虚拟路径成员解压到真实目标目录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveMemberExtractionRequest {
    /// 包内成员的虚拟路径（可包含文件夹，文件夹递归）。
    pub sources: Vec<PathBuf>,
    /// 真实文件系统目标目录。
    pub destination: PathBuf,
    pub password: Option<ArchivePassword>,
}

impl ArchiveMemberExtractionRequest {
    pub fn new(sources: Vec<PathBuf>, destination: PathBuf) -> Self {
        Self {
            sources,
            destination,
            password: None,
        }
    }

    pub fn with_password(mut self, password: Option<ArchivePassword>) -> Self {
        self.password = password;
        self
    }
}

/// 批量提取包内成员到真实目录：文件夹成员递归展开保持结构；
/// 目标重名的文件自动追加 ` (n)` 序号，绝不覆盖既有内容。
pub async fn extract_archive_members_with_controls_and_progress(
    request: ArchiveMemberExtractionRequest,
    mut controls: FileOperationControls,
    mut progress: impl FnMut(ArchiveExtractionProgress) + Send,
) -> Result<PathBuf, FileError> {
    controls.wait_until_running().await?;
    let destination = request.destination.clone();
    let groups = group_sources_by_archive(&request.sources)?;

    for group in groups {
        let tree = member_tree_for_archive(&group.boundaries).await?;
        for source in group.sources {
            if controls.cancellation_token().is_cancelled() {
                return Err(FileError::Cancelled);
            }
            let inner = source_relative_member(&group.boundaries, &source);
            let worklist = member_worklist(&tree, &inner).ok_or_else(|| FileError::Archive {
                path: group.boundaries[0].clone(),
                message: format!("archive member not found: {}", inner.display()),
            })?;
            let total_entries = worklist.len();
            let mut completed_entries = 0;
            let mut completed_bytes = 0;
            for (member_path, len) in worklist {
                let relative = member_path
                    .strip_prefix(&inner)
                    .ok()
                    .filter(|relative| !relative.as_os_str().is_empty())
                    .map(Path::to_path_buf);
                let target = match &relative {
                    Some(relative) => destination.join(relative),
                    None => destination.join(
                        member_path
                            .file_name()
                            .map(PathBuf::from)
                            .unwrap_or_else(|| PathBuf::from("member")),
                    ),
                };
                let target = unique_destination(&target);
                if let Some(parent) = target.parent() {
                    tokio::fs::create_dir_all(parent).await.map_err(|source| {
                        FileError::CreateDirectory {
                            path: parent.to_path_buf(),
                            source,
                        }
                    })?;
                }
                let archive_file = super::temp_store::materialize_innermost_archive(
                    &group.boundaries,
                    controls.cancellation_token().clone(),
                )
                .await?;
                let member_text = member_path.to_string_lossy().into_owned();
                extract_member(
                    &archive_file,
                    &member_text,
                    &target,
                    request.password.as_ref(),
                )
                .await?;
                completed_entries += 1;
                completed_bytes += len;
                progress(ArchiveExtractionProgress {
                    completed_bytes,
                    total_bytes: 0,
                    completed_entries,
                    total_entries,
                });
            }
        }
    }
    Ok(destination)
}

/// 把单个包内成员物化为可直接打开的临时文件，返回真实文件路径。
/// 加密 zip 未提供密码时返回 `ArchivePasswordRequired`。
pub(crate) async fn materialize_member_for_open(
    resolved: &ResolvedArchivePathRef,
    password: Option<&ArchivePassword>,
    cancellation: CancellationToken,
) -> Result<PathBuf, FileError> {
    let archive_file =
        super::temp_store::materialize_innermost_archive(&resolved.boundaries, cancellation)
            .await?;
    let member = resolved.inner.to_string_lossy().into_owned();
    let destination = super::temp_store::opened_member_path(&resolved.boundaries, &resolved.inner);
    if tokio::fs::try_exists(&destination).await.unwrap_or(false) {
        return Ok(destination);
    }
    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| FileError::CreateDirectory {
                path: parent.to_path_buf(),
                source,
            })?;
    }
    extract_member(&archive_file, &member, &destination, password).await?;
    Ok(destination)
}

type ResolvedArchivePathRef = super::ResolvedArchivePath;

/// 统一的成员提取入口：按格式选择 blocking（zip/tar）或 async（7z/rar）路径。
pub(crate) async fn extract_member(
    archive: &Path,
    member: &str,
    destination: &Path,
    password: Option<&ArchivePassword>,
) -> Result<(), FileError> {
    let format = archive_extraction_format_for_path(archive).ok_or(FileError::Unsupported(
        "archive format is not supported for member extraction",
    ))?;
    match format {
        ArchiveExtractionFormat::SevenZip | ArchiveExtractionFormat::Rar => {
            extract_member_with_seven_zip(archive, member, destination, password).await
        }
        ArchiveExtractionFormat::Zip
        | ArchiveExtractionFormat::Tar
        | ArchiveExtractionFormat::TarGz => {
            let archive_path = archive.to_path_buf();
            let destination_path = destination.to_path_buf();
            let member = member.to_owned();
            let password = password.cloned();
            tokio::task::spawn_blocking(move || match format {
                ArchiveExtractionFormat::Zip => {
                    extract_zip_member(&archive_path, &member, &destination_path, password.as_ref())
                }
                ArchiveExtractionFormat::Tar => {
                    extract_tar_member(&archive_path, &member, &destination_path, TarInput::Plain)
                }
                ArchiveExtractionFormat::TarGz => {
                    extract_tar_member(&archive_path, &member, &destination_path, TarInput::Gzip)
                }
                _ => unreachable!("seven-zip formats are dispatched to the async path"),
            })
            .await
            .map_err(|error| FileError::Archive {
                path: archive.to_path_buf(),
                message: error.to_string(),
            })?
        }
    }
}

struct SourceArchiveGroup {
    boundaries: Vec<PathBuf>,
    sources: Vec<PathBuf>,
}

/// 按最内层归档边界分组，同一压缩包的多个成员只列一次成员表。
fn group_sources_by_archive(sources: &[PathBuf]) -> Result<Vec<SourceArchiveGroup>, FileError> {
    let mut groups: Vec<SourceArchiveGroup> = Vec::new();
    for source in sources {
        let resolved =
            super::resolve_archive_virtual_path(source).ok_or(FileError::InvalidInput {
                path: source.clone(),
                message: "source path is not inside an archive".to_owned(),
            })?;
        match groups
            .iter_mut()
            .find(|group| group.boundaries == resolved.boundaries)
        {
            Some(group) => group.sources.push(source.clone()),
            None => groups.push(SourceArchiveGroup {
                boundaries: resolved.boundaries,
                sources: vec![source.clone()],
            }),
        }
    }
    Ok(groups)
}

/// 成员相对于最内层归档的路径。
fn source_relative_member(boundaries: &[PathBuf], source: &Path) -> PathBuf {
    source
        .strip_prefix(boundaries.last().expect("boundaries never empty"))
        .expect("source shares the innermost boundary prefix")
        .to_path_buf()
}

async fn member_tree_for_archive(
    boundaries: &[PathBuf],
) -> Result<super::tree::ArchiveMemberTree, FileError> {
    let archive_file =
        super::temp_store::materialize_innermost_archive(boundaries, CancellationToken::new())
            .await?;
    let format = archive_extraction_format_for_path(&archive_file).ok_or(
        FileError::Unsupported("archive format is not supported for member extraction"),
    )?;
    let members =
        crate::archive_listing::list_archive_members_with_format(archive_file, format).await?;
    Ok(super::tree::ArchiveMemberTree::build(members))
}

/// 成员的工作清单：文件成员是它自身，文件夹成员是子树内全部文件。
/// 返回 (成员路径, 未压缩大小)。
fn member_worklist(
    tree: &super::tree::ArchiveMemberTree,
    inner_member: &Path,
) -> Option<Vec<(PathBuf, u64)>> {
    tree.worklist_under(inner_member)
}

/// 目标重名自动追加 ` (n)`：包内成员落地绝不覆盖既有内容。
fn unique_destination(target: &Path) -> PathBuf {
    if !target.exists() {
        return target.to_path_buf();
    }
    let parent = target.parent().unwrap_or(Path::new(".")).to_path_buf();
    let file_name = target
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_else(|| std::ffi::OsString::from("member"));
    let stem = Path::new(&file_name)
        .file_stem()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_else(|| file_name.clone());
    let extension = Path::new(&file_name)
        .extension()
        .map(std::ffi::OsStr::to_os_string);

    for counter in 2.. {
        let mut candidate = std::ffi::OsString::from(&stem);
        candidate.push(format!(" ({counter})"));
        if let Some(extension) = &extension {
            candidate.push(".");
            candidate.push(extension);
        }
        let candidate = parent.join(candidate);
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("counter loop always returns on a free name")
}

enum TarInput {
    Plain,
    Gzip,
}

fn extract_zip_member(
    archive: &Path,
    member: &str,
    destination: &Path,
    password: Option<&ArchivePassword>,
) -> Result<(), FileError> {
    let file = std::fs::File::open(archive).map_err(|source| FileError::Archive {
        path: archive.to_path_buf(),
        message: source.to_string(),
    })?;
    let mut archive_handle = zip::ZipArchive::new(file).map_err(|source| FileError::Archive {
        path: archive.to_path_buf(),
        message: source.to_string(),
    })?;
    let opened = match password {
        Some(password) => archive_handle.by_name_decrypt(member, password.as_str().as_bytes()),
        None => archive_handle.by_name(member),
    };
    let mut entry = opened.map_err(|source| match source {
        zip::result::ZipError::InvalidPassword => FileError::ArchiveInvalidPassword {
            path: archive.to_path_buf(),
        },
        source => zip_member_lookup_error(archive, member, &source),
    })?;
    if entry.is_dir() {
        return Err(FileError::Archive {
            path: archive.to_path_buf(),
            message: format!("archive member is a directory: {member}"),
        });
    }

    let mut output =
        std::fs::File::create(destination).map_err(|source| FileError::CreateFile {
            path: destination.to_path_buf(),
            source,
        })?;
    std::io::copy(&mut entry, &mut output).map_err(|source| FileError::Archive {
        path: archive.to_path_buf(),
        message: source.to_string(),
    })?;
    Ok(())
}

fn zip_member_lookup_error(
    archive: &Path,
    member: &str,
    source: &zip::result::ZipError,
) -> FileError {
    FileError::Archive {
        path: archive.to_path_buf(),
        message: format!("archive member not found: {member}: {source}"),
    }
}

fn extract_tar_member(
    archive: &Path,
    member: &str,
    destination: &Path,
    input: TarInput,
) -> Result<(), FileError> {
    let file = std::fs::File::open(archive).map_err(|source| FileError::Archive {
        path: archive.to_path_buf(),
        message: source.to_string(),
    })?;
    let mut archive_handle = tar::Archive::new(match input {
        TarInput::Plain => Box::new(file) as Box<dyn Read>,
        TarInput::Gzip => Box::new(flate2::read::GzDecoder::new(file)),
    });

    for entry_outcome in archive_handle
        .entries()
        .map_err(|source| FileError::Archive {
            path: archive.to_path_buf(),
            message: source.to_string(),
        })?
    {
        let mut entry = entry_outcome.map_err(|source| FileError::Archive {
            path: archive.to_path_buf(),
            message: source.to_string(),
        })?;
        let entry_path = entry
            .path()
            .map_err(|source| FileError::Archive {
                path: archive.to_path_buf(),
                message: source.to_string(),
            })?
            .to_string_lossy()
            .into_owned();
        if entry_path.trim_end_matches('/') != member {
            continue;
        }
        let mut output =
            std::fs::File::create(destination).map_err(|source| FileError::CreateFile {
                path: destination.to_path_buf(),
                source,
            })?;
        std::io::copy(&mut entry, &mut output).map_err(|source| FileError::Archive {
            path: archive.to_path_buf(),
            message: source.to_string(),
        })?;
        return Ok(());
    }

    Err(FileError::Archive {
        path: archive.to_path_buf(),
        message: format!("archive member not found: {member}"),
    })
}

/// 7z/rar 成员提取：`-so` 把成员解到 stdout，流式拷入目标文件。
async fn extract_member_with_seven_zip(
    archive: &Path,
    member: &str,
    destination: &Path,
    password: Option<&ArchivePassword>,
) -> Result<(), FileError> {
    for command_name in SEVEN_ZIP_COMMAND_NAMES {
        let mut command = Command::new(command_name);
        command
            .arg("e")
            .arg("-so")
            .arg("-bd")
            .arg("-bsp0")
            .arg("-y");
        // 空密码让 7z 在需要时从 stdin 询问（stdin 已置 null 即失败），
        // 与既有解压管线的无密码行为一致。
        command.arg(match password {
            Some(password) => format!("-p{}", password.as_str()),
            None => "-p".to_owned(),
        });
        let child = command
            .arg("--")
            .arg(archive)
            .arg(member)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn();

        let mut child = match child {
            Ok(child) => child,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(FileError::Archive {
                    path: archive.to_path_buf(),
                    message: format!("could not run {command_name}: {source}"),
                })
            }
        };

        let Some(mut stdout) = child.stdout.take() else {
            return Err(FileError::Archive {
                path: archive.to_path_buf(),
                message: format!("{command_name} produced no stdout"),
            });
        };
        let mut output = tokio::fs::File::create(destination)
            .await
            .map_err(|source| FileError::CreateFile {
                path: destination.to_path_buf(),
                source,
            })?;
        let copy_outcome = tokio::io::copy(&mut stdout, &mut output).await;
        output.flush().await.ok();
        copy_outcome.map_err(|source| FileError::Archive {
            path: archive.to_path_buf(),
            message: source.to_string(),
        })?;

        let exit = child.wait().await.map_err(|source| FileError::Archive {
            path: archive.to_path_buf(),
            message: source.to_string(),
        })?;
        if !exit.success() {
            return Err(FileError::Archive {
                path: archive.to_path_buf(),
                message: format!("{command_name} exited with status {exit}"),
            });
        }
        return Ok(());
    }

    Err(FileError::Unsupported(
        "7z, 7zz or 7za command is required to extract this archive",
    ))
}
