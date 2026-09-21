use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

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
use crate::formatting::format_file_size;
use crate::model::Message;

const PDFINFO_TIMEOUT: Duration = Duration::from_secs(5);
const PDFTOPPM_TIMEOUT: Duration = Duration::from_secs(8);
const PDFINFO_STDOUT_LIMIT: usize = 2 * 1024 * 1024;
const PDFINFO_STDERR_LIMIT: usize = 16 * 1024;
const PDFTOPPM_STDOUT_LIMIT: usize = 16 * 1024;
const PDFTOPPM_STDERR_LIMIT: usize = 16 * 1024;
const PAGE_PNG_FILE_LIMIT: u64 = 64 * 1024 * 1024;
const PAGE_RENDER_CONCURRENCY: usize = 2;
const POPPLER_INSTALL_ERROR: &str = "PDF preview requires Poppler. Install the poppler package.";
const ENCRYPTED_PDF_ERROR: &str = "Encrypted PDF preview is not supported";

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

pub(crate) fn prepare_document_command(request: DocumentPrepareRequest) -> Task<Message> {
    Task::perform(prepare_document(request), |outcome| {
        Message::DocumentPreview(DocumentPreviewMessage::Prepared(outcome))
    })
}

pub(crate) fn render_document_page_command(request: DocumentPageRenderRequest) -> Task<Message> {
    Task::perform(render_document_page(request), |outcome| {
        Message::DocumentPreview(DocumentPreviewMessage::PageRendered(outcome))
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
        DocumentPreviewFormat::Pdf => format!("Could not inspect PDF: {error}"),
        DocumentPreviewFormat::Office(_) => {
            format!("Could not inspect Office document: {error}")
        }
    })?;
    if !metadata.is_file() {
        return Err(match format {
            DocumentPreviewFormat::Pdf => "PDF preview source is not a regular file".to_owned(),
            DocumentPreviewFormat::Office(_) => {
                "Office preview source is not a regular file".to_owned()
            }
        });
    }
    if request.max_file_bytes != 0 && metadata.len() > request.max_file_bytes {
        return Err(format!(
            "File is too large to preview ({}). Maximum preview size is {}.",
            format_file_size(metadata.len()),
            format_file_size(request.max_file_bytes)
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
                        .map_err(|error| format!("Could not create PDF preview workspace: {error}"))?
                        .map_err(|error| format!("Could not create PDF preview workspace: {error}"))?
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
        return Err(ENCRYPTED_PDF_ERROR.to_owned());
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
        Err(error) if error == POPPLER_INSTALL_ERROR => {
            DocumentPageRenderOutcome::Failed(key, error)
        }
        Err(error) => {
            DocumentPageRenderOutcome::Failed(key, format!("Could not render PDF page: {error}"))
        }
    }
}

async fn render_document_page_inner(
    request: &DocumentPageRenderRequest,
    programs: &PopplerPrograms,
) -> Result<Option<DocumentPageRenderResult>, String> {
    let Some(_permit) = acquire_page_render_permit(request).await? else {
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
        Ok(Some(_)) => read_rendered_page(request, &output_file).await.map(Some),
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
            .map_err(|_| "PDF page renderer is unavailable".to_owned()),
    }
}

async fn read_rendered_page(
    request: &DocumentPageRenderRequest,
    output_file: &Path,
) -> Result<DocumentPageRenderResult, String> {
    let metadata = tokio::fs::metadata(output_file)
        .await
        .map_err(|error| format!("Could not read rendered PDF page: {error}"))?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err("Poppler produced an empty PDF page image".to_owned());
    }
    if metadata.len() > PAGE_PNG_FILE_LIMIT {
        return Err("Rendered PDF page image exceeds the file budget".to_owned());
    }

    let dimensions_path = output_file.to_path_buf();
    let (width, height) =
        tokio::task::spawn_blocking(move || image::image_dimensions(dimensions_path))
            .await
            .map_err(|error| format!("Could not inspect rendered PDF page: {error}"))?
            .map_err(|error| format!("Could not inspect rendered PDF page: {error}"))?;
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| "Rendered PDF page dimensions overflowed".to_owned())?;
    let rgba_bytes = pixels
        .checked_mul(4)
        .ok_or_else(|| "Rendered PDF page memory size overflowed".to_owned())?;
    if width == 0
        || height == 0
        || width > crate::document_preview::MAX_DOCUMENT_PAGE_EDGE
        || height > crate::document_preview::MAX_DOCUMENT_PAGE_EDGE
        || pixels > crate::document_preview::MAX_DOCUMENT_PAGE_PIXELS
        || width > request.plan.width
        || height > request.plan.height
        || rgba_bytes > request.plan.estimated_rgba_bytes
    {
        return Err("Rendered PDF page exceeds the planned rendering budget".to_owned());
    }
    let bytes = tokio::fs::read(output_file)
        .await
        .map_err(|error| format!("Could not read rendered PDF page: {error}"))?;

    Ok(DocumentPageRenderResult {
        key: request.key.clone(),
        handle: iced::widget::image::Handle::from_bytes(bytes),
        #[cfg(test)]
        width,
        #[cfg(test)]
        height,
        rgba_bytes,
    })
}

async fn run_poppler_command(
    program: &Path,
    arguments: &[OsString],
    label: &str,
    timeout: Duration,
    stdout_limit: usize,
    stderr_limit: usize,
    document_cancellation: &CancellationToken,
    render_cancellation: Option<&CancellationToken>,
) -> Result<Option<DocumentToolOutput>, String> {
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
    .map_err(|error| match error {
        DocumentToolError::NotFound => POPPLER_INSTALL_ERROR.to_owned(),
        DocumentToolError::Failed(message) => message,
    })
}

fn pdf_inspection_error(error: String) -> String {
    if error == POPPLER_INSTALL_ERROR {
        error
    } else if error.to_ascii_lowercase().contains("incorrect password") {
        ENCRYPTED_PDF_ERROR.to_owned()
    } else {
        format!("Could not inspect PDF: {error}")
    }
}

#[cfg(test)]
#[path = "document_preview/tests.rs"]
mod tests;
