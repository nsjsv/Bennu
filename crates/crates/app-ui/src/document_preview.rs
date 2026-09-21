mod format;
mod model;
mod pdfinfo;
mod resources;
mod workspace;

pub(crate) use format::{
    document_preview_format_for_path, DocumentPreviewFormat, OfficeDocumentFormat,
};
pub(crate) use model::{
    document_viewport_height, DocumentPageRenderOutcome, DocumentPageRenderRequest,
    DocumentPageRenderResult, DocumentPageView, DocumentPrepareOutcome, DocumentPrepareRequest,
    DocumentPreviewMessage, DocumentPreviewRequestKey, DocumentScaleAxis, DocumentViewportKey,
    PagedDocumentPreview, PendingDocumentPreview, PreparedDocumentPreview,
};
#[cfg(test)]
pub(crate) use model::{
    DocumentPageRenderPlan, DocumentPageRequestKey, DocumentPageSize, DocumentRenderKey,
};
pub(crate) use pdfinfo::{parse_pdfinfo_pages, parse_pdfinfo_summary};
pub(crate) use resources::{MAX_DOCUMENT_PAGE_EDGE, MAX_DOCUMENT_PAGE_PIXELS};
pub(crate) use workspace::{DocumentPreviewWorkspace, OfficeDocumentPreviewWorkspace};
