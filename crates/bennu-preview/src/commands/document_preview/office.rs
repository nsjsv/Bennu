use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::process::{
    run_document_tool_command, DocumentToolCommand, DocumentToolError, DocumentToolOutput,
};
use super::validate_document_source;
use crate::document_preview::{
    DocumentPrepareRequest, DocumentPreviewFormat, DocumentPreviewWorkspace,
    OfficeDocumentPreviewWorkspace,
};
use crate::formatting::format_file_size;

pub(super) const LIBREOFFICE_INSTALL_ERROR: &str =
    "Office preview requires LibreOffice. Install the libreoffice package.";
const OFFICE_CONVERSION_CONCURRENCY: usize = 1;
const OFFICE_CONVERSION_TIMEOUT: Duration = Duration::from_secs(30);
const OFFICE_CONVERSION_OUTPUT_LIMIT: usize = 64 * 1024;

static OFFICE_CONVERSION_SEMAPHORE: OnceLock<Arc<Semaphore>> = OnceLock::new();

#[derive(Debug, Clone)]
pub(super) struct OfficePrograms {
    pub(super) libreoffice: PathBuf,
    pub(super) soffice: PathBuf,
}

impl Default for OfficePrograms {
    fn default() -> Self {
        Self {
            libreoffice: PathBuf::from("libreoffice"),
            soffice: PathBuf::from("soffice"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct OfficeConversionLimits {
    timeout: Duration,
    stdout: usize,
    stderr: usize,
}

impl Default for OfficeConversionLimits {
    fn default() -> Self {
        Self {
            timeout: OFFICE_CONVERSION_TIMEOUT,
            stdout: OFFICE_CONVERSION_OUTPUT_LIMIT,
            stderr: OFFICE_CONVERSION_OUTPUT_LIMIT,
        }
    }
}

pub(super) async fn prepare_office_document_workspace(
    request: &DocumentPrepareRequest,
    format: DocumentPreviewFormat,
    programs: &OfficePrograms,
) -> Result<Option<DocumentPreviewWorkspace>, String> {
    let workspace = tokio::select! {
        _ = request.cancellation.cancelled() => return Ok(None),
        workspace = tokio::task::spawn_blocking(OfficeDocumentPreviewWorkspace::create) => {
            workspace
                .map_err(|error| format!("Could not create Office preview workspace: {error}"))?
                .map_err(|error| format!("Could not create Office preview workspace: {error}"))?
        },
    };
    let Some(pdf_path) = convert_office_document_in_workspace(
        request,
        format,
        &workspace,
        programs,
        OfficeConversionLimits::default(),
    )
    .await?
    else {
        return Ok(None);
    };
    workspace
        .into_document_workspace(pdf_path)
        .await
        .map(Some)
        .map_err(|error| format!("Could not convert Office document: {error}"))
}

async fn convert_office_document_in_workspace(
    request: &DocumentPrepareRequest,
    format: DocumentPreviewFormat,
    workspace: &OfficeDocumentPreviewWorkspace,
    programs: &OfficePrograms,
    limits: OfficeConversionLimits,
) -> Result<Option<PathBuf>, String> {
    let Some(_permit) = acquire_office_conversion_permit(request).await? else {
        return Ok(None);
    };
    // 等待全局转换许可期间源文件可能增长，因此必须在 spawn 前重新执行同一预算边界。
    if validate_document_source(request, format).await?.is_none() {
        return Ok(None);
    }

    let arguments = office_conversion_arguments(request, workspace);
    let environment = office_conversion_environment(workspace);
    let output = match run_office_program(
        &programs.libreoffice,
        "libreoffice",
        &arguments,
        &environment,
        request,
        limits,
    )
    .await
    {
        Err(DocumentToolError::NotFound) => {
            match run_office_program(
                &programs.soffice,
                "soffice",
                &arguments,
                &environment,
                request,
                limits,
            )
            .await
            {
                Err(DocumentToolError::NotFound) => {
                    return Err(LIBREOFFICE_INSTALL_ERROR.to_owned())
                }
                outcome => outcome,
            }
        }
        outcome => outcome,
    };
    let Some(_output) = output.map_err(|error| {
        format!(
            "Could not convert Office document: {}",
            error.into_message()
        )
    })?
    else {
        return Ok(None);
    };
    if request.cancellation.is_cancelled() {
        return Ok(None);
    }

    validate_office_conversion_output(workspace, request.max_file_bytes)
        .await
        .map(Some)
        .map_err(|error| format!("Could not convert Office document: {error}"))
}

async fn acquire_office_conversion_permit(
    request: &DocumentPrepareRequest,
) -> Result<Option<OwnedSemaphorePermit>, String> {
    let semaphore = OFFICE_CONVERSION_SEMAPHORE
        .get_or_init(|| Arc::new(Semaphore::new(OFFICE_CONVERSION_CONCURRENCY)))
        .clone();
    tokio::select! {
        _ = request.cancellation.cancelled() => Ok(None),
        permit = semaphore.acquire_owned() => permit
            .map(Some)
            .map_err(|_| "Office document converter is unavailable".to_owned()),
    }
}

async fn run_office_program(
    program: &Path,
    label: &str,
    arguments: &[OsString],
    environment: &[(OsString, OsString)],
    request: &DocumentPrepareRequest,
    limits: OfficeConversionLimits,
) -> Result<Option<DocumentToolOutput>, DocumentToolError> {
    run_document_tool_command(DocumentToolCommand {
        program,
        arguments,
        environment,
        label,
        timeout: limits.timeout,
        stdout_limit: limits.stdout,
        stderr_limit: limits.stderr,
        document_cancellation: &request.cancellation,
        render_cancellation: None,
    })
    .await
}

fn office_conversion_arguments(
    request: &DocumentPrepareRequest,
    workspace: &OfficeDocumentPreviewWorkspace,
) -> Vec<OsString> {
    vec![
        OsString::from("--headless"),
        OsString::from("--nologo"),
        OsString::from("--nodefault"),
        OsString::from("--norestore"),
        OsString::from(format!("-env:UserInstallation={}", workspace.profile_url())),
        OsString::from("--convert-to"),
        OsString::from("pdf"),
        OsString::from("--outdir"),
        workspace.output_dir().as_os_str().to_owned(),
        request.key.source_path.as_os_str().to_owned(),
    ]
}

fn office_conversion_environment(
    workspace: &OfficeDocumentPreviewWorkspace,
) -> Vec<(OsString, OsString)> {
    let mut environment = [
        ("HOME", workspace.home_dir()),
        ("TMPDIR", workspace.temporary_dir()),
        ("XDG_CONFIG_HOME", workspace.xdg_config_dir()),
        ("XDG_CACHE_HOME", workspace.xdg_cache_dir()),
        ("XDG_DATA_HOME", workspace.xdg_data_dir()),
    ]
    .into_iter()
    .map(|(name, path)| (OsString::from(name), path.as_os_str().to_owned()))
    .collect::<Vec<_>>();
    // 预览转换不需要 GPU 公式加速，禁用 OpenCL 可避免 fresh profile 初始化公式缓存。
    environment.push((OsString::from("SAL_DISABLE_OPENCL"), OsString::from("1")));
    environment
}

struct OfficeOutputEntry {
    path: PathBuf,
    file_type: std::fs::FileType,
    metadata: std::fs::Metadata,
}

async fn validate_office_conversion_output(
    workspace: &OfficeDocumentPreviewWorkspace,
    max_file_bytes: u64,
) -> Result<PathBuf, String> {
    verify_office_output_directory_identity(workspace).await?;
    let scan_outcome = scan_office_output_directory(workspace.output_dir()).await;
    // 子进程拥有目录内容写权限，只有枚举和 metadata 完成后的同一 inode 才能证明结果未逃逸。
    verify_office_output_directory_identity(workspace).await?;
    let (entry, has_extra_entry) = scan_outcome?;
    let entry = entry.ok_or_else(|| "LibreOffice did not produce a PDF".to_owned())?;
    if has_extra_entry {
        return Err("LibreOffice produced more than one output".to_owned());
    }
    if entry.file_type.is_symlink() || !entry.file_type.is_file() {
        return Err("LibreOffice output is not a regular PDF file".to_owned());
    }
    if entry.path.parent() != Some(workspace.output_dir()) {
        return Err("LibreOffice output escaped the preview workspace".to_owned());
    }
    let is_pdf = entry
        .path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"));
    if !is_pdf {
        return Err("LibreOffice output is not a PDF file".to_owned());
    }
    if !entry.metadata.file_type().is_file() || entry.metadata.len() == 0 {
        return Err("LibreOffice produced an empty PDF".to_owned());
    }
    if max_file_bytes != 0 && entry.metadata.len() > max_file_bytes {
        return Err(format!(
            "Converted Office PDF is too large to preview ({}). Maximum preview size is {}.",
            format_file_size(entry.metadata.len()),
            format_file_size(max_file_bytes)
        ));
    }
    Ok(entry.path)
}

async fn scan_office_output_directory(
    output_dir: &Path,
) -> Result<(Option<OfficeOutputEntry>, bool), String> {
    let mut entries = tokio::fs::read_dir(output_dir)
        .await
        .map_err(|error| format!("Could not inspect LibreOffice output: {error}"))?;
    let entry = entries
        .next_entry()
        .await
        .map_err(|error| format!("Could not inspect LibreOffice output: {error}"))?;
    let has_extra_entry = entries
        .next_entry()
        .await
        .map_err(|error| format!("Could not inspect LibreOffice output: {error}"))?
        .is_some();
    let entry = if let Some(entry) = entry {
        let path = entry.path();
        let file_type = entry
            .file_type()
            .await
            .map_err(|error| format!("Could not inspect LibreOffice output: {error}"))?;
        let metadata = tokio::fs::symlink_metadata(&path)
            .await
            .map_err(|error| format!("Could not inspect LibreOffice output: {error}"))?;
        Some(OfficeOutputEntry {
            path,
            file_type,
            metadata,
        })
    } else {
        None
    };
    Ok((entry, has_extra_entry))
}

async fn verify_office_output_directory_identity(
    workspace: &OfficeDocumentPreviewWorkspace,
) -> Result<(), String> {
    match workspace.output_directory_identity_is_current().await {
        Ok(true) => Ok(()),
        Ok(false) => Err("LibreOffice output directory identity changed".to_owned()),
        Err(error) => Err(format!(
            "Could not inspect LibreOffice output directory: {error}"
        )),
    }
}

#[cfg(test)]
#[path = "office/tests.rs"]
mod tests;
