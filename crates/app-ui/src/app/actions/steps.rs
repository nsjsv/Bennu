//! 每种步骤 → 队列操作。准备时要读盘（分辨能不能转、找空着的名字、看目标文件夹在不在），
//! 在后台做，做完交回界面线程入队。不覆盖的规则就定在这里：复制、移动遇到同名一律
//! 自动编号，压缩包找一个空着的名字，转换和解压由引擎自己避开已有的名字。

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use file_core::{
    ArchiveCompressionLevel, ArchiveExtractionRequest, ArchiveFormat, EntryNameKind, FileKind,
    FileOperationVerification, OutputLocation, TransferConflictStrategy,
};
use file_operations::{CommandInvocation, QueuedFileOperation, QueuedTransfer};

use crate::actions::{ActionStep, ConvertFormat, PathTemplate, RunValues, TrashScope};
use crate::app::archive_creation::{archive_target_directory, default_archive_name};
use crate::app::convert::classify_convert_sources;
use crate::model::{ConversionSettings, ConvertInputError};

/// 一次运行里每一步都用到的东西，开始时定下。
#[derive(Debug, Clone)]
pub(crate) struct StepContext {
    /// 动作开始时选中的项（空白处右键时是当前文件夹）。
    pub(crate) selected: Vec<PathBuf>,
    pub(crate) values: RunValues,
    /// 执行命令的文件夹、压缩包落不下时的退路：当前标签页的文件夹。
    pub(crate) working_directory: PathBuf,
}

/// 准备好的一步。
#[derive(Debug, Clone)]
pub(crate) enum PreparedStep {
    Ready(StepPlan),
    Failed(String),
}

#[derive(Debug, Clone)]
pub(crate) struct StepPlan {
    pub(crate) work: Vec<StepWork>,
    /// 这一步不动、原样交给下一步的项：执行命令的输入，移动时已经在目标文件夹里的项。
    pub(crate) passed_through: Vec<PathBuf>,
    /// 这一批里这一步处理不了的项，"名字：原因"，在最后的通知里报出来。
    pub(crate) skipped: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct StepWork {
    pub(crate) operation: QueuedFileOperation,
    /// 这个操作做完以后接着要做的。
    pub(crate) then: Option<FollowUp>,
}

#[derive(Debug, Clone)]
pub(crate) enum FollowUp {
    /// 目标文件夹刚建好（实际建成的路径要等结果出来才知道），把这些项复制进去。
    CopyInto(Vec<PathBuf>),
}

impl FollowUp {
    pub(crate) fn operation(self, created_folder: &Path) -> QueuedFileOperation {
        match self {
            Self::CopyInto(sources) => QueuedFileOperation::Copy {
                transfers: transfers_into(&sources, created_folder),
                verification: FileOperationVerification::default(),
            },
        }
    }
}

pub(crate) async fn prepare_step(
    step: ActionStep,
    input: Vec<PathBuf>,
    context: StepContext,
) -> PreparedStep {
    let needs_items = !matches!(step, ActionStep::Run(_));
    if needs_items && input.is_empty() {
        return PreparedStep::Failed(tr!("There is nothing left to work on"));
    }
    let prepared = match step {
        ActionStep::Convert(format) => prepare_convert(format, input).await,
        ActionStep::Run(command) => {
            let invocations = command
                .invocations(&input, &context.values)
                .into_iter()
                .filter_map(|mut words| {
                    let arguments = words.split_off(1);
                    let program = words.pop()?;
                    Some(CommandInvocation {
                        program,
                        arguments,
                        working_directory: context.working_directory.clone(),
                    })
                })
                .collect::<Vec<_>>();
            if invocations.is_empty() {
                return PreparedStep::Failed(tr!("There is nothing left to work on"));
            }
            Ok(StepPlan {
                work: vec![StepWork {
                    operation: QueuedFileOperation::RunCommands { invocations },
                    then: None,
                }],
                passed_through: input,
                skipped: Vec::new(),
            })
        }
        ActionStep::Trash(scope) => {
            let paths = match scope {
                TrashScope::Current => input,
                TrashScope::Selected => context.selected.clone(),
            };
            Ok(plan(vec![QueuedFileOperation::Trash { paths }]))
        }
        // 剩下的要读盘。
        step => tokio::task::spawn_blocking(move || prepare_on_disk(step, input, &context))
            .await
            .unwrap_or_else(|error| Err(error.to_string())),
    };
    match prepared {
        Ok(plan) if plan.work.is_empty() && plan.passed_through.is_empty() => {
            PreparedStep::Failed(no_item_fits(&plan.skipped))
        }
        Ok(plan) => PreparedStep::Ready(plan),
        Err(reason) => PreparedStep::Failed(reason),
    }
}

fn plan(operations: Vec<QueuedFileOperation>) -> StepPlan {
    StepPlan {
        work: operations
            .into_iter()
            .map(|operation| StepWork {
                operation,
                then: None,
            })
            .collect(),
        passed_through: Vec::new(),
        skipped: Vec::new(),
    }
}

fn no_item_fits(skipped: &[String]) -> String {
    match skipped.first() {
        Some(first) => tr!("None of the items fit this step (%1)", first),
        None => tr!("There is nothing left to work on"),
    }
}

async fn prepare_convert(format: ConvertFormat, input: Vec<PathBuf>) -> Result<StepPlan, String> {
    let sources = classify_convert_sources(input.clone()).await;
    let mut skipped = input
        .iter()
        .filter(|path| !sources.iter().any(|source| source.path == **path))
        .map(|path| tr!("%1: cannot be converted", display_name(path)))
        .collect::<Vec<_>>();
    let encoders = file_core::available_conversion_encoders().await;
    let mut requests = Vec::new();
    let mut missing_encoder = false;
    for source in sources {
        let Some(target) = format.target_for(source.kind) else {
            skipped.push(tr!(
                "%1: cannot be converted to %2",
                display_name(&source.path),
                format.name()
            ));
            continue;
        };
        match ConversionSettings::factory_default(target, &encoders) {
            Ok(settings) => {
                requests.push(settings.request(&source.path, &OutputLocation::BesideSource))
            }
            Err(ConvertInputError::MissingEncoder) => missing_encoder = true,
            Err(_) => skipped.push(tr!(
                "%1: cannot be converted to %2",
                display_name(&source.path),
                format.name()
            )),
        }
    }
    if requests.is_empty() && missing_encoder {
        return Err(tr!(
            "This computer cannot convert to %1 (ffmpeg or its encoder is missing)",
            format.name()
        ));
    }
    let mut plan = if requests.is_empty() {
        plan(Vec::new())
    } else {
        plan(vec![QueuedFileOperation::Convert { requests }])
    };
    plan.skipped = skipped;
    Ok(plan)
}

fn prepare_on_disk(
    step: ActionStep,
    input: Vec<PathBuf>,
    context: &StepContext,
) -> Result<StepPlan, String> {
    match step {
        ActionStep::Compress(format) => prepare_compress(format, input, context),
        ActionStep::Extract(placement) => {
            let (archives, others): (Vec<_>, Vec<_>) = input
                .into_iter()
                .partition(|path| file_core::is_supported_archive_path(path));
            let mut plan = plan(
                archives
                    .into_iter()
                    .map(|archive| {
                        ArchiveExtractionRequest::new(archive, placement)
                            .map(|request| QueuedFileOperation::ExtractArchive { request })
                            .map_err(|error| error.to_string())
                    })
                    .collect::<Result<_, _>>()?,
            );
            plan.skipped = others
                .iter()
                .map(|path| tr!("%1: not an archive", display_name(path)))
                .collect();
            Ok(plan)
        }
        ActionStep::CopyTo(folder) => prepare_transfer(&folder, input, context, Transfer::Copy),
        ActionStep::MoveTo(folder) => prepare_transfer(&folder, input, context, Transfer::Move),
        ActionStep::Convert(_)
        | ActionStep::Run(_)
        | ActionStep::Trash(_)
        | ActionStep::CopyPaths => {
            unreachable!("prepared without reading the disk")
        }
    }
}

fn prepare_compress(
    format: ArchiveFormat,
    input: Vec<PathBuf>,
    context: &StepContext,
) -> Result<StepPlan, String> {
    let directory = archive_target_directory(&input, &context.working_directory);
    let single_kind = match input.as_slice() {
        [only] => std::fs::metadata(only).ok().map(|metadata| {
            if metadata.is_dir() {
                FileKind::Directory
            } else {
                FileKind::File
            }
        }),
        _ => None,
    };
    let preferred = format!(
        "{}.{}",
        default_archive_name(&input, single_kind),
        format.extension()
    );
    // 压缩时用 create_new 新建，这里只是挑一个现在空着的名字；万一被抢先，任务失败而不是覆盖。
    let target = file_core::entry_name_candidates(preferred.as_ref(), EntryNameKind::File)
        .map(|name| directory.join(name))
        .find(|candidate| is_free(candidate))
        .ok_or_else(|| tr!("No free name for the archive in %1", directory.display()))?;
    Ok(plan(vec![QueuedFileOperation::CreateArchive {
        sources: input,
        target,
        format,
        compression_level: ArchiveCompressionLevel::Balanced,
        password: None,
    }]))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transfer {
    Copy,
    Move,
}

fn prepare_transfer(
    folder: &PathTemplate,
    input: Vec<PathBuf>,
    context: &StepContext,
    transfer: Transfer,
) -> Result<StepPlan, String> {
    let mut by_folder = BTreeMap::<PathBuf, Vec<PathBuf>>::new();
    for item in input {
        by_folder
            .entry(folder.resolve(&item, &context.values))
            .or_default()
            .push(item);
    }
    let mut plan = plan(Vec::new());
    for (target, items) in by_folder {
        match std::fs::metadata(&target) {
            Ok(metadata) if metadata.is_dir() => {
                let (already_there, items): (Vec<_>, Vec<_>) = items
                    .into_iter()
                    .partition(|item| transfer == Transfer::Move && item.parent() == Some(&target));
                plan.passed_through.extend(already_there);
                if items.is_empty() {
                    continue;
                }
                let transfers = transfers_into(&items, &target);
                let verification = FileOperationVerification::default();
                plan.work.push(StepWork {
                    operation: match transfer {
                        Transfer::Copy => QueuedFileOperation::Copy {
                            transfers,
                            verification,
                        },
                        Transfer::Move => QueuedFileOperation::Move {
                            transfers,
                            verification,
                        },
                    },
                    then: None,
                });
            }
            Ok(_) => return Err(tr!("%1 is not a folder", target.display())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                plan.work.push(into_new_folder(target, items, transfer)?);
            }
            Err(error) => return Err(tr!("%1: %2", target.display(), error)),
        }
    }
    Ok(plan)
}

/// 目标文件夹还不存在。移动：新建文件夹时整批移进去，撤销时挪回原处、删掉新建的空文件夹；
/// 只差最后一层时用"用选中项新建文件夹"，重做也能用。复制：先建文件夹，等它建好再按实际
/// 建成的路径复制进去。
fn into_new_folder(
    target: PathBuf,
    items: Vec<PathBuf>,
    transfer: Transfer,
) -> Result<StepWork, String> {
    let (parent, relative) = missing_folder(&target)?;
    let single_level = parent.as_path() == target.parent().unwrap_or(&target);
    Ok(match transfer {
        Transfer::Move if single_level => StepWork {
            operation: QueuedFileOperation::GatherSelectionIntoNewFolder {
                directory: target,
                sources: items,
            },
            then: None,
        },
        Transfer::Move => StepWork {
            operation: QueuedFileOperation::CreateDirectories {
                parent,
                names: vec![relative],
                gather_sources: items,
            },
            then: None,
        },
        Transfer::Copy => StepWork {
            operation: QueuedFileOperation::CreateDirectories {
                parent,
                names: vec![relative],
                gather_sources: Vec::new(),
            },
            then: Some(FollowUp::CopyInto(items)),
        },
    })
}

/// 还不存在的目标文件夹拆成最近的已有上级和它下面要新建的那一段（`a/b`）。
fn missing_folder(target: &Path) -> Result<(PathBuf, String), String> {
    let mut parent = target;
    while let Some(next) = parent.parent() {
        parent = next;
        match std::fs::metadata(parent) {
            Ok(metadata) if metadata.is_dir() => break,
            Ok(_) => return Err(tr!("%1 is not a folder", parent.display())),
            Err(_) => continue,
        }
    }
    let relative = target
        .strip_prefix(parent)
        .ok()
        .and_then(Path::to_str)
        .filter(|relative| !relative.is_empty())
        .ok_or_else(|| tr!("Cannot create the folder %1", target.display()))?;
    Ok((parent.to_path_buf(), relative.to_owned()))
}

/// 复制、移动进 `folder`：同名时自动编号（"名字 2"），不覆盖、不合并。
fn transfers_into(items: &[PathBuf], folder: &Path) -> Vec<QueuedTransfer> {
    items
        .iter()
        .map(|item| QueuedTransfer {
            source: item.clone(),
            target: folder.join(item.file_name().unwrap_or(item.as_os_str())),
            conflict_strategy: TransferConflictStrategy::KeepBoth,
        })
        .collect()
}

fn is_free(path: &Path) -> bool {
    matches!(
        std::fs::symlink_metadata(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound
    )
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(OsString::from)
        .unwrap_or_else(|| path.as_os_str().to_owned())
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
#[path = "steps_tests.rs"]
mod tests;
