//! 抽文档的文字层：pdftotext 一次抽整份（带每个词的位置），后台解析成页内比例。
//! 只在第一次搜索或拖选时抽；关掉文档时随文档一起取消。

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bennu_localization::tr;
use iced::Task;
use tokio_util::sync::CancellationToken;

use super::process::DocumentToolError;
use super::{poppler_missing, run_poppler_command};
use crate::document_preview::{
    parse_bbox_layout, DocumentPreviewMessage, DocumentPreviewRequestKey, DocumentPreviewWorkspace,
    DocumentText, DocumentTextMessage,
};
use crate::preview_message::PreviewMessage;

/// 几百页的文档要抽好几秒；超过这么久多半是坏文件。
const PDFTOTEXT_TIMEOUT: Duration = Duration::from_secs(60);
/// 每个词一行 XML 一百来字节：几千页的书也在这以内，再多就不搜了。
const PDFTOTEXT_STDOUT_LIMIT: usize = 128 * 1024 * 1024;
const PDFTOTEXT_STDERR_LIMIT: usize = 16 * 1024;

#[derive(Debug, Clone)]
pub struct DocumentTextRequest {
    pub key: DocumentPreviewRequestKey,
    pub workspace: Arc<DocumentPreviewWorkspace>,
    /// 每页显示出来的宽高（点），坐标按它换成比例。
    pub page_sizes: Vec<(f64, f64)>,
    pub cancellation: CancellationToken,
}

pub fn extract_document_text_command(request: DocumentTextRequest) -> Task<PreviewMessage> {
    let key = request.key.clone();
    Task::future(extract_document_text(request, PathBuf::from("pdftotext"))).and_then(
        move |outcome| {
            Task::done(PreviewMessage::DocumentPreview(
                DocumentPreviewMessage::Text(DocumentTextMessage::Extracted {
                    key: key.clone(),
                    outcome,
                }),
            ))
        },
    )
}

/// 取消了返回 None（不回消息）；其余带着成败回去。
async fn extract_document_text(
    request: DocumentTextRequest,
    program: PathBuf,
) -> Option<Result<Arc<DocumentText>, String>> {
    let arguments = [
        OsString::from("-cropbox"),
        OsString::from("-bbox-layout"),
        OsString::from("-enc"),
        OsString::from("UTF-8"),
        request.workspace.pdf_path().as_os_str().to_owned(),
        OsString::from("-"),
    ];
    let output = match run_poppler_command(
        &program,
        &arguments,
        "pdftotext",
        PDFTOTEXT_TIMEOUT,
        PDFTOTEXT_STDOUT_LIMIT,
        PDFTOTEXT_STDERR_LIMIT,
        &request.cancellation,
        None,
    )
    .await
    {
        Ok(Some(output)) => output,
        Ok(None) => return None,
        Err(DocumentToolError::NotFound) => return Some(Err(poppler_missing())),
        Err(DocumentToolError::Failed(detail)) => {
            return Some(Err(tr!("Could not read the document's text: %1", detail)))
        }
    };
    let page_sizes = request.page_sizes;
    let parsed = tokio::task::spawn_blocking(move || {
        let xml = String::from_utf8_lossy(&output.stdout.bytes);
        parse_bbox_layout(&xml, &page_sizes)
    })
    .await
    .map_err(|error| error.to_string())
    .and_then(|parsed| parsed);
    Some(
        parsed
            .map(Arc::new)
            .map_err(|detail| tr!("Could not read the document's text: %1", detail)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真 Poppler 抽装进仓库的样张：旋转过的第二页词的位置是显示方向的。
    #[tokio::test]
    #[ignore = "needs pdftotext on PATH"]
    async fn extracts_words_from_a_real_pdf() {
        let pdf = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/mixed-crop-rotation.pdf");
        let workspace = Arc::new(DocumentPreviewWorkspace::create_for_pdf(pdf).expect("workspace"));
        let text = extract_document_text(
            DocumentTextRequest {
                key: DocumentPreviewRequestKey {
                    source_path: PathBuf::from("/tmp/mixed.pdf"),
                    document_generation: 1,
                },
                workspace,
                page_sizes: vec![(500.0, 600.0), (400.0, 600.0)],
                cancellation: CancellationToken::new(),
            },
            PathBuf::from("pdftotext"),
        )
        .await
        .expect("not cancelled")
        .expect("text");
        let words: Vec<&str> = text.pages[1]
            .words
            .iter()
            .map(|word| word.text.as_str())
            .collect();
        assert_eq!(words, ["Rotated", "crop", "page"]);
        let rotated = text.pages[1].words[0].bounds;
        // 渲染出来这页的字在横向 0.48–0.56、纵向 0.17–0.65 一带。
        assert!(rotated.left > 0.45 && rotated.right < 0.6, "{rotated:?}");
        assert!(rotated.top > 0.15 && rotated.bottom < 0.4, "{rotated:?}");
    }
}
