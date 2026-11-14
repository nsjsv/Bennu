use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};

use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use super::tree::{ArchiveMemberTree, WorklistMember};
use crate::archive_extraction::archive_extraction_format_for_path;
use crate::seven_zip_job::{locate_seven_zip, seven_zip_failure};
use crate::tar_job::{collect_output, tar_command, tar_failure};
use crate::{
    ArchiveExtractionFormat, ArchivePassword, ArchiveProgress, FileError, FileOperationControls,
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
    mut progress: impl FnMut(ArchiveProgress) + Send,
) -> Result<PathBuf, FileError> {
    controls.wait_until_running().await?;
    let destination = request.destination.clone();
    let groups = group_sources_by_archive(&request.sources)?;

    for group in groups {
        let archive_file = super::temp_store::materialize_innermost_archive(
            &group.boundaries,
            controls.cancellation_token(),
        )
        .await?;
        let tree = member_tree(&archive_file).await?;
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
            let total_bytes = worklist.iter().map(|member| member.len).sum::<u64>();
            let mut completed_bytes = 0;
            // 落地锚点必须保留被拖成员自身这一层名字：文件夹成员 A 的
            // 子孙要落成 目标/A/<子结构>，而不是把 A 剥掉、把子项散在
            // 目标顶层（同时 A 自己只剩一个空目录，PRD 验收明确禁止）。
            // 对文件成员而言锚点就是 目标/文件名，与既有拖文件行为一致。
            let anchor = member_anchor(&destination, &inner);
            for (completed_index, member) in worklist.into_iter().enumerate() {
                let target = member_target(&anchor, &inner, &member.path);
                let target = unique_destination(&target);
                if let Some(parent) = target.parent() {
                    tokio::fs::create_dir_all(parent).await.map_err(|source| {
                        FileError::CreateDirectory {
                            path: parent.to_path_buf(),
                            source,
                        }
                    })?;
                }
                extract_member(
                    &archive_file,
                    &member.stored_path,
                    &target,
                    request.password.as_ref(),
                )
                .await?;
                completed_bytes += member.len;
                progress(ArchiveProgress::Entries {
                    completed_entries: completed_index + 1,
                    total_entries,
                    bytes: Some((completed_bytes, total_bytes)),
                    current_entry: None,
                });
            }
        }
    }
    Ok(destination)
}

/// 把单个包内成员物化为可直接打开的临时文件，返回真实文件路径。
/// 加密包未提供密码时返回 `ArchivePasswordRequired`。
pub(crate) async fn materialize_member_for_open(
    resolved: &ResolvedArchivePathRef,
    password: Option<&ArchivePassword>,
    cancellation: CancellationToken,
) -> Result<PathBuf, FileError> {
    let archive_file =
        super::temp_store::materialize_innermost_archive(&resolved.boundaries, cancellation)
            .await?;
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
    extract_file_member(&archive_file, &resolved.inner, &destination, password).await?;
    Ok(destination)
}

type ResolvedArchivePathRef = super::ResolvedArchivePath;

/// 按显示用的包内路径解出一个文件成员：先列目录找到它在包里原样存的名字
/// （中文 Windows 打的 zip 里是 GBK 字节），再按那个名字解。
pub(crate) async fn extract_file_member(
    archive: &Path,
    inner: &Path,
    destination: &Path,
    password: Option<&ArchivePassword>,
) -> Result<(), FileError> {
    let member = member_tree(archive)
        .await?
        .file_member(inner)
        .ok_or_else(|| FileError::Archive {
            path: archive.to_path_buf(),
            message: format!("archive member not found: {}", inner.display()),
        })?;
    extract_member(archive, &member.stored_path, destination, password).await
}

/// 成员解到 stdout、写进目标文件：zip/7z/rar 用 `7z e -so`，tar 一类用 `tar -x -O`。
/// 两边都按字面匹配名字，不当通配符。
async fn extract_member(
    archive: &Path,
    stored_member: &[u8],
    destination: &Path,
    password: Option<&ArchivePassword>,
) -> Result<(), FileError> {
    let format = archive_extraction_format_for_path(archive).ok_or(FileError::Unsupported(
        "archive format is not supported for member extraction",
    ))?;
    let member = OsStr::from_bytes(stored_member);
    let command = match format {
        ArchiveExtractionFormat::Tar => {
            let mut command = tar_command();
            command
                .arg("-x")
                .arg("-O")
                .arg("--no-wildcards")
                .arg("--occurrence=1")
                .arg("-f")
                .arg(archive)
                .arg("--")
                .arg(member);
            command
        }
        ArchiveExtractionFormat::Zip
        | ArchiveExtractionFormat::SevenZip
        | ArchiveExtractionFormat::Rar => {
            let mut command = Command::new(locate_seven_zip()?);
            // 没有密码时给空的 `-p`：7z 不会停下来问（stdin 已关），直接报需要密码。
            command
                .arg("e")
                .arg("-so")
                .arg("-bd")
                .arg("-bsp0")
                .arg("-y")
                .arg(match password {
                    Some(password) => format!("-p{}", password.as_str()),
                    None => "-p".to_owned(),
                })
                .arg("-spd")
                .arg("--")
                .arg(archive)
                .arg(member)
                .stdin(Stdio::null())
                .kill_on_drop(true);
            command
        }
    };
    match run_member_to_file(command, destination).await {
        Ok(()) => Ok(()),
        Err(MemberFailure::Io(source)) => Err(FileError::Archive {
            path: archive.to_path_buf(),
            message: source.to_string(),
        }),
        Err(MemberFailure::Exited { status, stderr }) => Err(match format {
            ArchiveExtractionFormat::Tar => tar_failure(archive, status, &stderr),
            _ => seven_zip_failure(archive, password.is_some(), status, &[], &stderr),
        }),
    }
}

enum MemberFailure {
    Io(std::io::Error),
    Exited { status: ExitStatus, stderr: Vec<u8> },
}

/// 子进程 stdout 拷进目标文件。stderr 与拷贝并发读尽：子进程往 stderr 写诊断的同时 stdout
/// 一直在流式输出，等 stdout 读完再收 stderr 会因管道写端阻塞互相卡死。
async fn run_member_to_file(mut command: Command, destination: &Path) -> Result<(), MemberFailure> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(MemberFailure::Io)?;
    let mut stdout = child.stdout.take().expect("member stdout is piped");
    let stderr = collect_output(child.stderr.take().expect("member stderr is piped"));
    let mut output = match tokio::fs::File::create(destination).await {
        Ok(output) => output,
        Err(source) => {
            let _ = child.kill().await;
            return Err(MemberFailure::Io(source));
        }
    };
    let copied = tokio::io::copy(&mut stdout, &mut output).await;
    if copied.is_err() {
        // 拷贝中断（目标盘写满等）时子进程会因 stdout 管道塞满而永不退出，stderr 也就永不
        // 结束：必须先杀掉它。
        let _ = child.start_kill();
    }
    output.flush().await.ok();
    let stderr = stderr.await.unwrap_or_default();
    let outcome = match copied {
        Err(source) => Err(MemberFailure::Io(source)),
        Ok(_) => match child.wait().await {
            Err(source) => Err(MemberFailure::Io(source)),
            Ok(status) if !status.success() => Err(MemberFailure::Exited { status, stderr }),
            Ok(_) => Ok(()),
        },
    };
    if outcome.is_err() {
        // 目标文件是本次新建的半成品（密码错误时通常为空）：留着会让重试触发
        // unique_destination 的 ` (2)` 改名，打开用的临时文件还会被当成已解好的缓存。
        let _ = tokio::fs::remove_file(destination).await;
    }
    outcome
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

async fn member_tree(archive_file: &Path) -> Result<ArchiveMemberTree, FileError> {
    let format = archive_extraction_format_for_path(archive_file).ok_or(FileError::Unsupported(
        "archive format is not supported for member extraction",
    ))?;
    let members =
        crate::archive_listing::list_archive_members_with_format(archive_file, format).await?;
    Ok(ArchiveMemberTree::build(members))
}

/// 成员的工作清单：文件成员是它自身，文件夹成员是子树内全部文件。
fn member_worklist(tree: &ArchiveMemberTree, inner_member: &Path) -> Option<Vec<WorklistMember>> {
    tree.worklist_under(inner_member)
}

/// 成员提取的落地锚点：目标目录下保留被拖成员自身这一层名字。
/// 拖/粘贴文件夹成员 A 时子孙要落成 目标/A/<子结构>，而不是把 A
/// 剥掉、子项散在目标顶层；整包提取（source 即归档本身，成员路径
/// 为空）的既有语义是把包内容直接解到目标目录，多套一层会破坏
/// 粘贴 zip 的行为，因此空成员锚回 destination。
fn member_anchor(destination: &Path, inner_member: &Path) -> PathBuf {
    match inner_member.file_name() {
        Some(name) => destination.join(name),
        None => destination.to_path_buf(),
    }
}

/// 工作清单条目的落地目标：子孙条目锚在 目标/<成员名>/ 下（保留
/// 文件夹成员自身这层）；根条目 / 文件成员本身直接落锚点。
fn member_target(anchor: &Path, inner_member: &Path, member_path: &Path) -> PathBuf {
    let relative = member_path
        .strip_prefix(inner_member)
        .ok()
        .filter(|relative| !relative.as_os_str().is_empty())
        .map(Path::to_path_buf);
    match relative {
        Some(relative) => anchor.join(relative),
        None => anchor.to_path_buf(),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive_listing::ArchiveListingEntry;
    use crate::archive_vfs::tree::ArchiveMemberTree;
    use crate::FileKind;

    fn member(path: &str, kind: FileKind, len: u64) -> ArchiveListingEntry {
        ArchiveListingEntry {
            path: path.to_owned(),
            stored_path: path.as_bytes().to_vec(),
            kind,
            len,
            modified: None,
        }
    }

    /// 把工作清单映射为落地目标的测试辅助：直接复用生产纯函数。
    fn worklist_targets(
        tree: &ArchiveMemberTree,
        destination: &Path,
        inner: &Path,
    ) -> Vec<PathBuf> {
        let anchor = member_anchor(destination, inner);
        member_worklist(tree, inner)
            .unwrap()
            .iter()
            .map(|member| member_target(&anchor, inner, &member.path))
            .collect()
    }

    /// PRD 验收：拖/粘贴顶层文件夹成员必须保留该文件夹自身作为根，
    /// 得到 目标/A/<子结构>，而不是把 A 剥掉、子项散在目标顶层。
    #[test]
    fn top_level_folder_member_keeps_itself_as_root() {
        let tree = ArchiveMemberTree::build([
            member("A/", FileKind::Directory, 0),
            member("A/2026-09/x.jpg", FileKind::File, 3),
            member("A/readme.txt", FileKind::File, 1),
        ]);
        let inner = Path::new("A");

        let targets = worklist_targets(&tree, Path::new("/dest"), inner);

        // worklist 按 BTreeMap 字典序遍历，比对前先排序消除顺序敏感。
        let mut actual = targets.clone();
        actual.sort();
        assert_eq!(
            actual,
            vec![
                PathBuf::from("/dest/A/2026-09/x.jpg"),
                PathBuf::from("/dest/A/readme.txt"),
            ]
        );
        // 目标顶层不允许出现 A 的子项散落（只有 A 自身这一层）。
        assert!(targets
            .iter()
            .all(|target| target.starts_with("/dest/A")
                && target.parent() != Some(Path::new("/dest"))));
    }

    /// PRD 验收：拖嵌套子文件夹 A/B 得到 目标/B/…，
    /// 锚点取成员自身文件名 B，而不是整条内部路径 A/B。
    #[test]
    fn nested_folder_member_anchors_at_its_own_name() {
        let tree = ArchiveMemberTree::build([
            member("A/B/", FileKind::Directory, 0),
            member("A/B/y.jpg", FileKind::File, 5),
            member("A/B/sub/w.txt", FileKind::File, 2),
        ]);
        let inner = Path::new("A/B");

        let targets = worklist_targets(&tree, Path::new("/dest"), inner);

        let mut actual = targets;
        actual.sort();
        assert_eq!(
            actual,
            vec![
                PathBuf::from("/dest/B/sub/w.txt"),
                PathBuf::from("/dest/B/y.jpg"),
            ]
        );
    }

    /// PRD 验收：拖文件成员行为不变，仍直接落为 目标/文件名。
    #[test]
    fn file_member_lands_directly_under_its_name() {
        let tree = ArchiveMemberTree::build([member("a.txt", FileKind::File, 4)]);
        let inner = Path::new("a.txt");

        let targets = worklist_targets(&tree, Path::new("/dest"), inner);

        assert_eq!(targets, vec![PathBuf::from("/dest/a.txt")]);
    }

    /// 整包提取（source 即归档本身，成员路径为空）沿用既有语义：
    /// 包内容直接解到目标目录，不额外套一层归档文件名的目录。
    #[test]
    fn whole_archive_extraction_anchors_at_destination_itself() {
        assert_eq!(
            member_anchor(Path::new("/dest"), Path::new("")),
            PathBuf::from("/dest")
        );
    }
}
