use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::archive_extraction::archive_extraction_format_for_path;
use crate::seven_zip_password::seven_zip_password_error;
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
            let mut completed_bytes = 0;
            for (completed_index, (member_path, len)) in worklist.into_iter().enumerate() {
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
                completed_bytes += len;
                progress(ArchiveExtractionProgress {
                    completed_bytes,
                    total_bytes: 0,
                    completed_entries: completed_index + 1,
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
    let mut entry = opened.map_err(|source| zip_member_open_error(archive, member, &source))?;
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

fn zip_member_open_error(
    archive: &Path,
    member: &str,
    source: &zip::result::ZipError,
) -> FileError {
    match source {
        // 密码错:by_name_decrypt 打开时即校验 ZipCrypto/AES 验证字节。
        zip::result::ZipError::InvalidPassword => FileError::ArchiveInvalidPassword {
            path: archive.to_path_buf(),
        },
        // zip 8.x 对加密条目无密码调用 by_name 会立即报这个错误
        // (UnsupportedArchive(PASSWORD_REQUIRED)),直接归入需要密码,
        // 不能落到「成员未找到」的普通包装里误导用户。
        zip::result::ZipError::UnsupportedArchive(message)
            if *message == zip::result::ZipError::PASSWORD_REQUIRED =>
        {
            FileError::ArchivePasswordRequired {
                path: archive.to_path_buf(),
            }
        }
        source => zip_member_lookup_error(archive, member, source),
    }
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
        // stderr 必须与 stdout 拷贝并发读尽:7z 往 stderr 写诊断的同时
        // stdout 一直在流式输出,等 stdout 读完再收 stderr 会因管道写端
        // 阻塞互相卡死。stdout 是载荷流不参与错误分类,诊断只看 stderr。
        // 收尾放进独立任务,便于拷贝中断时先杀 7z 再等 stderr EOF。
        let stderr_task = child.stderr.take().map(|mut stderr| {
            tokio::spawn(async move {
                let mut stderr_bytes = Vec::new();
                let _ = stderr.read_to_end(&mut stderr_bytes).await;
                stderr_bytes
            })
        });
        let mut output = tokio::fs::File::create(destination)
            .await
            .map_err(|source| FileError::CreateFile {
                path: destination.to_path_buf(),
                source,
            })?;
        let copy_outcome = tokio::io::copy(&mut stdout, &mut output).await;
        if copy_outcome.is_err() {
            // stdout 拷贝中断(目标盘写满等)时 7z 会因 stdout 管道塞满而
            // 永不退出,stderr 也就永不 EOF:必须杀掉子进程,下面的
            // stderr 收尾任务才能结束,否则本函数在 await 上卡死。
            let _ = child.start_kill();
        }
        output.flush().await.ok();
        let stderr_bytes = match stderr_task {
            Some(task) => task.await.unwrap_or_default(),
            None => Vec::new(),
        };
        if let Err(source) = copy_outcome {
            // 半成品目标文件同理必须清掉,不让重试触发 ` (2)` 改名。
            let _ = tokio::fs::remove_file(destination).await;
            return Err(FileError::Archive {
                path: archive.to_path_buf(),
                message: source.to_string(),
            });
        }

        let exit = child.wait().await.map_err(|source| FileError::Archive {
            path: archive.to_path_buf(),
            message: source.to_string(),
        })?;
        if !exit.success() {
            // 目标文件是本次用 File::create 造出来的半成品(密码错误时
            // 通常为空文件):留着会让重试触发 unique_destination 的
            // ` (2)` 改名,必须清掉让重试沿用原名。只删本次失败新建的
            // 目标,不触碰既有文件(既有重名早被 unique_destination 避开)。
            let _ = tokio::fs::remove_file(destination).await;
            let stderr_text = String::from_utf8_lossy(&stderr_bytes);
            if let Some(error) = seven_zip_password_error(&stderr_text, password.is_some(), archive)
            {
                return Err(error);
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// zip 8.x 对加密条目无密码调用 by_name 会立即返回
    /// UnsupportedArchive(PASSWORD_REQUIRED);必须归为「需要密码」,
    /// 不能落到「成员未找到」的普通包装里,否则上层弹不出密码框。
    #[test]
    fn zip_member_open_without_password_maps_to_password_required() {
        let archive = PathBuf::from("/tmp/docs.zip");
        let error = zip_member_open_error(
            &archive,
            "secret.txt",
            &zip::result::ZipError::UnsupportedArchive(zip::result::ZipError::PASSWORD_REQUIRED),
        );

        assert!(matches!(
            error,
            FileError::ArchivePasswordRequired { path } if path == archive
        ));
    }

    /// 密码错在 by_name_decrypt 打开时即被验证字节拦下,归为密码无效。
    #[test]
    fn zip_member_open_with_wrong_password_maps_to_invalid_password() {
        let archive = PathBuf::from("/tmp/docs.zip");
        let error = zip_member_open_error(
            &archive,
            "secret.txt",
            &zip::result::ZipError::InvalidPassword,
        );

        assert!(matches!(
            error,
            FileError::ArchiveInvalidPassword { path } if path == archive
        ));
    }
}
