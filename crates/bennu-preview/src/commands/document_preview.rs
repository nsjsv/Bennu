use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bennu_localization::tr;
use iced::Task;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

use self::office::{prepare_office_document_workspace, OfficePrograms};
use self::process::{
    run_document_tool_command, DocumentToolCommand, DocumentToolError, DocumentToolOutput,
};
use crate::document_preview::{
    document_preview_format_for_path, parse_pdfinfo_pages, parse_pdfinfo_summary,
    DocumentPageRenderOutcome, DocumentPageRenderRequest, DocumentPageRenderResult,
    DocumentPrepareOutcome, DocumentPrepareRequest, DocumentPreviewFormat, DocumentPreviewMessage,
    DocumentPreviewWorkspace, DocumentScaleAxis, OfficeDocumentFormat, PreparedDocumentPreview,
};
use crate::preview_loading::file_too_large_to_preview;
use crate::preview_message::PreviewMessage;

const PDFINFO_TIMEOUT: Duration = Duration::from_secs(5);
const PDFTOPPM_TIMEOUT: Duration = Duration::from_secs(8);
const PDFINFO_STDOUT_LIMIT: usize = 2 * 1024 * 1024;
const PDFINFO_STDERR_LIMIT: usize = 16 * 1024;
const PDFTOPPM_STDOUT_LIMIT: usize = 16 * 1024;
const PDFTOPPM_STDERR_LIMIT: usize = 16 * 1024;
const PAGE_PNG_FILE_LIMIT: u64 = 64 * 1024 * 1024;
const PAGE_RENDER_CONCURRENCY: usize = 2;

static PAGE_RENDER_SEMAPHORE: OnceLock<Arc<Semaphore>> = OnceLock::new();

mod office;
mod process;

#[derive(Debug, Clone)]
struct PopplerPrograms {
    pdfinfo: PathBuf,
    pdftoppm: PathBuf,
}

impl Default for PopplerPrograms {
    fn default() -> Self {
        Self {
            pdfinfo: PathBuf::from("pdfinfo"),
            pdftoppm: PathBuf::from("pdftoppm"),
        }
    }
}

#[derive(Debug, Clone, Default)]
struct DocumentPrograms {
    poppler: PopplerPrograms,
    office: OfficePrograms,
}

pub fn prepare_document_command(request: DocumentPrepareRequest) -> Task<PreviewMessage> {
    Task::perform(prepare_document(request), |outcome| {
        PreviewMessage::DocumentPreview(DocumentPreviewMessage::Prepared(outcome))
    })
}

pub fn render_document_page_command(request: DocumentPageRenderRequest) -> Task<PreviewMessage> {
    Task::perform(render_document_page(request), |outcome| {
        PreviewMessage::DocumentPreview(DocumentPreviewMessage::PageRendered(outcome))
    })
}

async fn prepare_document(request: DocumentPrepareRequest) -> DocumentPrepareOutcome {
    prepare_document_with_programs(request, DocumentPrograms::default()).await
}

async fn prepare_document_with_programs(
    request: DocumentPrepareRequest,
    programs: DocumentPrograms,
) -> DocumentPrepareOutcome {
    let key = request.key.clone();
    match prepare_document_inner(&request, &programs).await {
        Ok(Some(prepared)) => DocumentPrepareOutcome::Ready(prepared),
        Ok(None) => DocumentPrepareOutcome::Cancelled(key),
        Err(error) => DocumentPrepareOutcome::Failed(key, error),
    }
}

/// 在 prepare 异步体内解析文档格式：嗅探 %PDF 头识别 PDF（含自定义
/// 后缀的改名 PDF）；嗅探不中或读不到时按扩展名兜底（内置行为不变），
/// 自定义 Office 后缀交给 LibreOffice 内容自识别。格式是渲染器的
/// 内部实现细节，不再随请求跨层传递。
async fn detect_document_preview_format(request: &DocumentPrepareRequest) -> DocumentPreviewFormat {
    use tokio::io::AsyncReadExt;

    let fallback = || {
        document_preview_format_for_path(&request.key.source_path)
            .unwrap_or(DocumentPreviewFormat::Office(OfficeDocumentFormat::Docx))
    };
    let is_pdf = async {
        let mut file = tokio::fs::File::open(&request.key.source_path).await.ok()?;
        let mut header = [0u8; 5];
        let read = file.read(&mut header).await.ok()?;
        Some(header[..read] == *b"%PDF-")
    }
    .await;
    if is_pdf == Some(true) {
        DocumentPreviewFormat::Pdf
    } else {
        fallback()
    }
}

pub(super) async fn validate_document_source(
    request: &DocumentPrepareRequest,
    format: DocumentPreviewFormat,
) -> Result<Option<std::fs::Metadata>, String> {
    let metadata = tokio::select! {
        _ = request.cancellation.cancelled() => return Ok(None),
        metadata = tokio::fs::metadata(&request.key.source_path) => metadata,
    }
    .map_err(|error| match format {
        DocumentPreviewFormat::Pdf => tr!("Could not inspect PDF: %1", error),
        DocumentPreviewFormat::Office(_) => tr!("Could not inspect Office document: %1", error),
    })?;
    if !metadata.is_file() {
        return Err(match format {
            DocumentPreviewFormat::Pdf => tr!("PDF preview source is not a regular file"),
            DocumentPreviewFormat::Office(_) => tr!("Office preview source is not a regular file"),
        });
    }
    if request.max_file_bytes != 0 && metadata.len() > request.max_file_bytes {
        return Err(file_too_large_to_preview(
            metadata.len(),
            request.max_file_bytes,
        ));
    }
    Ok(Some(metadata))
}

async fn prepare_document_inner(
    request: &DocumentPrepareRequest,
    programs: &DocumentPrograms,
) -> Result<Option<PreparedDocumentPreview>, String> {
    let format = detect_document_preview_format(request).await;
    if validate_document_source(request, format).await?.is_none() {
        return Ok(None);
    }

    let workspace = match format {
        DocumentPreviewFormat::Pdf => {
            let pdf_path = request.key.source_path.clone();
            let workspace = tokio::select! {
                _ = request.cancellation.cancelled() => return Ok(None),
                workspace = tokio::task::spawn_blocking(move || DocumentPreviewWorkspace::create_for_pdf(pdf_path)) => {
                    workspace
                        .map_err(|error| tr!("Could not create PDF preview workspace: %1", error))?
                        .map_err(|error| tr!("Could not create PDF preview workspace: %1", error))?
                },
            };
            workspace
        }
        DocumentPreviewFormat::Office(_) => {
            let Some(workspace) =
                prepare_office_document_workspace(request, format, &programs.office).await?
            else {
                return Ok(None);
            };
            workspace
        }
    };
    let workspace = Arc::new(workspace);

    let summary_arguments = vec![
        OsString::from("-box"),
        workspace.pdf_path().as_os_str().to_owned(),
    ];
    let summary_output = match run_poppler_command(
        &programs.poppler.pdfinfo,
        &summary_arguments,
        "pdfinfo",
        PDFINFO_TIMEOUT,
        PDFINFO_STDOUT_LIMIT,
        PDFINFO_STDERR_LIMIT,
        &request.cancellation,
        None,
    )
    .await
    {
        Ok(Some(output)) => output,
        Ok(None) => return Ok(None),
        Err(error) => return Err(pdf_inspection_error(error)),
    };
    let summary = parse_pdfinfo_summary(&summary_output.stdout.bytes)?;
    if summary.encrypted {
        return Err(tr!("Encrypted PDF preview is not supported"));
    }

    let page_arguments = vec![
        OsString::from("-f"),
        OsString::from("1"),
        OsString::from("-l"),
        OsString::from(summary.page_count.to_string()),
        OsString::from("-box"),
        workspace.pdf_path().as_os_str().to_owned(),
    ];
    let Some(page_output) = run_poppler_command(
        &programs.poppler.pdfinfo,
        &page_arguments,
        "pdfinfo",
        PDFINFO_TIMEOUT,
        PDFINFO_STDOUT_LIMIT,
        PDFINFO_STDERR_LIMIT,
        &request.cancellation,
        None,
    )
    .await
    .map_err(pdf_inspection_error)?
    else {
        return Ok(None);
    };
    let pages = parse_pdfinfo_pages(&page_output.stdout.bytes, summary.page_count)?;

    Ok(Some(PreparedDocumentPreview {
        key: request.key.clone(),
        workspace,
        pages,
    }))
}

async fn render_document_page(request: DocumentPageRenderRequest) -> DocumentPageRenderOutcome {
    let key = request.key.clone();
    match render_document_page_inner(&request, &PopplerPrograms::default()).await {
        Ok(Some(result)) => DocumentPageRenderOutcome::Ready(result),
        Ok(None) => DocumentPageRenderOutcome::Cancelled(key),
        Err(DocumentToolError::NotFound) => {
            DocumentPageRenderOutcome::Failed(key, poppler_missing())
        }
        Err(DocumentToolError::Failed(detail)) => {
            DocumentPageRenderOutcome::Failed(key, tr!("Could not render PDF page: %1", detail))
        }
    }
}

/// 出错时 `Failed` 带的是给用户看的详情（Poppler 原话或本函数自己的检查）。
async fn render_document_page_inner(
    request: &DocumentPageRenderRequest,
    programs: &PopplerPrograms,
) -> Result<Option<DocumentPageRenderResult>, DocumentToolError> {
    let Some(_permit) = acquire_page_render_permit(request)
        .await
        .map_err(DocumentToolError::Failed)?
    else {
        return Ok(None);
    };
    let output_prefix = request.workspace.page_output_prefix(&request.key);
    let output_file = output_prefix.with_extension("png");
    let _ = tokio::fs::remove_file(&output_file).await;
    let page_number = request.key.page_index + 1;
    let mut arguments = vec![
        OsString::from("-f"),
        OsString::from(page_number.to_string()),
        OsString::from("-l"),
        OsString::from(page_number.to_string()),
        OsString::from("-singlefile"),
        OsString::from("-cropbox"),
        OsString::from("-png"),
    ];
    match request.plan.scale_axis {
        DocumentScaleAxis::Width(width) => {
            arguments.extend([
                OsString::from("-scale-to-x"),
                OsString::from(width.to_string()),
                OsString::from("-scale-to-y"),
                OsString::from("-1"),
            ]);
        }
        DocumentScaleAxis::Height(height) => {
            arguments.extend([
                OsString::from("-scale-to-x"),
                OsString::from("-1"),
                OsString::from("-scale-to-y"),
                OsString::from(height.to_string()),
            ]);
        }
    }
    arguments.push(request.workspace.pdf_path().as_os_str().to_owned());
    arguments.push(output_prefix.as_os_str().to_owned());

    let command_outcome = run_poppler_command(
        &programs.pdftoppm,
        &arguments,
        "pdftoppm",
        PDFTOPPM_TIMEOUT,
        PDFTOPPM_STDOUT_LIMIT,
        PDFTOPPM_STDERR_LIMIT,
        &request.document_cancellation,
        Some(&request.render_cancellation),
    )
    .await;
    let result = match command_outcome {
        Ok(Some(_)) => read_rendered_page(request, &output_file)
            .await
            .map(Some)
            .map_err(DocumentToolError::Failed),
        Ok(None) => Ok(None),
        Err(error) => Err(error),
    };
    let _ = tokio::fs::remove_file(&output_file).await;
    result
}

async fn acquire_page_render_permit(
    request: &DocumentPageRenderRequest,
) -> Result<Option<OwnedSemaphorePermit>, String> {
    let semaphore = PAGE_RENDER_SEMAPHORE
        .get_or_init(|| Arc::new(Semaphore::new(PAGE_RENDER_CONCURRENCY)))
        .clone();
    tokio::select! {
        _ = request.document_cancellation.cancelled() => Ok(None),
        _ = request.render_cancellation.cancelled() => Ok(None),
        permit = semaphore.acquire_owned() => permit
            .map(Some)
            .map_err(|_| tr!("PDF page renderer is unavailable")),
    }
}

async fn read_rendered_page(
    request: &DocumentPageRenderRequest,
    output_file: &Path,
) -> Result<DocumentPageRenderResult, String> {
    let metadata = tokio::fs::metadata(output_file)
        .await
        .map_err(|error| tr!("Could not read rendered PDF page: %1", error))?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err(tr!("Poppler produced an empty PDF page image"));
    }
    if metadata.len() > PAGE_PNG_FILE_LIMIT {
        return Err(tr!("Rendered PDF page image exceeds the file budget"));
    }

    let dimensions_path = output_file.to_path_buf();
    let (width, height) =
        tokio::task::spawn_blocking(move || image::image_dimensions(dimensions_path))
            .await
            .map_err(|error| tr!("Could not inspect rendered PDF page: %1", error))?
            .map_err(|error| tr!("Could not inspect rendered PDF page: %1", error))?;
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| tr!("Rendered PDF page dimensions overflowed"))?;
    let rgba_bytes = pixels
        .checked_mul(4)
        .ok_or_else(|| tr!("Rendered PDF page memory size overflowed"))?;
    if width == 0
        || height == 0
        || width > crate::document_preview::MAX_DOCUMENT_PAGE_EDGE
        || height > crate::document_preview::MAX_DOCUMENT_PAGE_EDGE
        || pixels > crate::document_preview::MAX_DOCUMENT_PAGE_PIXELS
        || width > request.plan.width
        || height > request.plan.height
        || rgba_bytes > request.plan.estimated_rgba_bytes
    {
        return Err(tr!(
            "Rendered PDF page exceeds the planned rendering budget"
        ));
    }
    let bytes = tokio::fs::read(output_file)
        .await
        .map_err(|error| tr!("Could not read rendered PDF page: %1", error))?;

    Ok(DocumentPageRenderResult {
        key: request.key.clone(),
        handle: iced::widget::image::Handle::from_bytes(bytes),
        width,
        height,
        rgba_bytes,
    })
}

// 参数即一次 poppler 子进程调用的完整配置（程序 + 限制 + 双取消源）
#[allow(clippy::too_many_arguments)]
async fn run_poppler_command(
    program: &Path,
    arguments: &[OsString],
    label: &str,
    timeout: Duration,
    stdout_limit: usize,
    stderr_limit: usize,
    document_cancellation: &CancellationToken,
    render_cancellation: Option<&CancellationToken>,
) -> Result<Option<DocumentToolOutput>, DocumentToolError> {
    let environment = [
        (OsString::from("LC_ALL"), OsString::from("C")),
        (OsString::from("LANG"), OsString::from("C")),
    ];
    run_document_tool_command(DocumentToolCommand {
        program,
        arguments,
        environment: &environment,
        label,
        timeout,
        stdout_limit,
        stderr_limit,
        document_cancellation,
        render_cancellation,
    })
    .await
}

fn poppler_missing() -> String {
    tr!("PDF preview requires Poppler. Install the poppler package.")
}

/// Poppler 原话只作详情：有用户密码的 PDF pdfinfo 打不开，也没有单独的退出码，
/// 用户从原话（"Incorrect password"）里就能看出来，不拿它来判断。
fn pdf_inspection_error(error: DocumentToolError) -> String {
    match error {
        DocumentToolError::NotFound => poppler_missing(),
        DocumentToolError::Failed(detail) => tr!("Could not inspect PDF: %1", detail),
    }
}

#[cfg(test)]
#[path = "document_preview/tests.rs"]
mod tests;
