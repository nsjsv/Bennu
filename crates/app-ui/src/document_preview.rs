// 纯搬移：文档预览模型已下沉 bennu-preview，re-export 维持
// crate::document_preview::* 既有调用路径（含 app-ui 测试用的渲染键类型）。
// 显式列出消费中的符号（不用 glob：glob re-export 不触发 unused 告警，
// 会掩盖孤儿，步骤 7 收尾时据此删掉了 13 个无消费者的符号）。
pub(crate) use bennu_preview::document_preview::{
    DocumentPageRenderOutcome, DocumentPrepareOutcome, DocumentPreviewMessage, DocumentViewportKey,
};
// 以下符号仅 app-ui 测试构造文档会话时消费，非测试构建下按需门控
// 避免未用告警。
#[cfg(test)]
pub(crate) use bennu_preview::document_preview::{
    DocumentPageRenderResult, DocumentPageRequestKey, DocumentPageSize, DocumentPageView,
    DocumentPreviewWorkspace, PreparedDocumentPreview,
};
