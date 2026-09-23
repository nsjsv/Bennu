// 纯搬移：文档预览模型（分页状态机/渲染键/Poppler 元数据解析/LibreOffice
// 会话工作区）自 app-ui 下沉；宿主命令层仍在 app-ui（commands/document_preview），
// 经 app-ui 侧 re-export 消费本模块类型。
mod format;
mod model;
mod pdfinfo;
mod resources;
mod workspace;

pub use format::{document_preview_format_for_path, DocumentPreviewFormat, OfficeDocumentFormat};
pub use model::{
    document_viewport_height, DocumentPageRenderOutcome, DocumentPageRenderRequest,
    DocumentPageRenderResult, DocumentPageView, DocumentPrepareOutcome, DocumentPrepareRequest,
    DocumentPreviewMessage, DocumentPreviewRequestKey, DocumentScaleAxis, DocumentViewportKey,
    PagedDocumentPreview, PendingDocumentPreview, PreparedDocumentPreview,
};
// 原 app-ui 侧为 #[cfg(test)] 门控的测试用类型改为无条件 pub：app-ui 的
// cfg(test) 编译 bennu-preview 依赖时不会开启对方的 cfg(test)，门控会让
// app-ui 测试拿不到这些名字。
pub use model::{
    DocumentPageRenderPlan, DocumentPageRequestKey, DocumentPageSize, DocumentRenderKey,
};
pub use pdfinfo::{parse_pdfinfo_pages, parse_pdfinfo_summary};
pub use resources::{MAX_DOCUMENT_PAGE_EDGE, MAX_DOCUMENT_PAGE_PIXELS};
pub use workspace::{DocumentPreviewWorkspace, OfficeDocumentPreviewWorkspace};
