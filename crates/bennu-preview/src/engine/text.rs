//! 文本预览：把文档要的后台读取变成任务，把读回来的结果交回文档；查看器事件、文件
//! 变化、改编码重新打开也在这里。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use encoding_rs::Encoding;
use iced::futures::{future, stream, StreamExt};
use iced::{Subscription, Task};

use crate::preview::{PreviewContent, PreviewState};
use crate::preview_message::PreviewMessage;
use crate::text_document::{
    check_file_change, count_lines, highlight_worker, read_rows, text_file_watch_subscription,
    ClipboardTarget, FileChange, RowReader, TextJump, TextOpened, TextPreviewDocument, TextRead,
    TextReadReply, TextReadReplyKind, TextViewerEvent,
};
use crate::text_preview_loading::{reopen_text_preview, TextEncodingChoice};

impl super::PreviewEngine {
    /// 新打开的文本：建文档，开始后台统计行号。
    pub(crate) fn open_text_document(
        &mut self,
        path: PathBuf,
        opened: &Arc<TextOpened>,
    ) -> Task<PreviewMessage> {
        self.text_preview_generation = self.text_preview_generation.wrapping_add(1);
        let (document, reads) =
            TextPreviewDocument::open(self.text_preview_generation, path, opened);
        let task = text_read_tasks(&document, reads);
        self.text_preview_document = Some(document);
        task
    }

    pub fn handle_text_viewer_event(&mut self, event: TextViewerEvent) -> Task<PreviewMessage> {
        let Some(document) = self.active_text_preview_document_mut() else {
            return Task::none();
        };
        match event {
            TextViewerEvent::ViewportChanged(viewport) => {
                let reads = document.accept_viewport(viewport);
                text_read_tasks(document, reads)
            }
            TextViewerEvent::JumpRequested(jump) => {
                let reads = document.jump_to(jump);
                text_read_tasks(document, reads)
            }
            // 视图设置归宿主，引擎的 handle 已经把它还给宿主。
            TextViewerEvent::ChangeView(_) => Task::none(),
            TextViewerEvent::CopyRequested { from, to, target } => {
                // 超过 15 MB 不复制（子任务 4 补提示）。
                if !TextPreviewDocument::copy_fits(from, to) {
                    return Task::none();
                }
                let source = document.source();
                let decoding = document.decoding();
                let len = document.len();
                read_task(document.id(), move || {
                    let reader = RowReader {
                        source: source.as_ref(),
                        decoding,
                        len,
                    };
                    TextReadReplyKind::Selection {
                        target,
                        outcome: reader
                            .text_between(from, to)
                            .map_err(|error| error.to_string()),
                    }
                })
            }
        }
    }

    pub fn accept_text_read(
        &mut self,
        document_id: u64,
        reply: TextReadReply,
    ) -> Task<PreviewMessage> {
        let Some(document) = self
            .active_text_preview_document_mut()
            .filter(|document| document.id() == document_id)
        else {
            return Task::none();
        };
        match reply.0 {
            TextReadReplyKind::Rows {
                generation,
                request,
                outcome,
            } => {
                let reads = document.accept_rows(generation, request, outcome);
                text_read_tasks(document, reads)
            }
            TextReadReplyKind::Number {
                generation,
                line_start,
                outcome,
            } => {
                let reads = document.accept_numbering(generation, line_start, outcome);
                text_read_tasks(document, reads)
            }
            TextReadReplyKind::Index(progress) => {
                let reads = document.accept_index_progress(progress);
                text_read_tasks(document, reads)
            }
            TextReadReplyKind::HighlightLanguage(language) => {
                let reads = document.accept_highlight_language(language);
                text_read_tasks(document, reads)
            }
            TextReadReplyKind::HighlightRows(rows) => {
                let reads = document.accept_highlight_rows(rows);
                text_read_tasks(document, reads)
            }
            TextReadReplyKind::FileChecked(outcome) => self.accept_text_file_check(outcome),
            TextReadReplyKind::Selection {
                target,
                outcome: Ok(text),
            } if !text.is_empty() => match target {
                ClipboardTarget::Standard => iced::clipboard::write(text),
                ClipboardTarget::Primary => iced::clipboard::write_primary(text),
            },
            TextReadReplyKind::Selection { .. } => Task::none(),
            TextReadReplyKind::Reopened(outcome) => self.accept_reopened_text(outcome),
        }
    }

    /// 文件监视说可能变了：后台比对文件戳。
    pub fn handle_text_file_changed(&mut self, document_id: u64) -> Task<PreviewMessage> {
        let Some(document) = self
            .active_text_preview_document_mut()
            .filter(|document| document.id() == document_id)
        else {
            return Task::none();
        };
        let path = document.path().to_path_buf();
        let stamp = document.stamp().clone();
        read_task(document.id(), move || {
            TextReadReplyKind::FileChecked(
                check_file_change(&path, &stamp).map_err(|error| error.to_string()),
            )
        })
    }

    /// 用户指定编码：按新编码重新打开，停在原来的位置。
    pub fn select_text_preview_encoding(
        &mut self,
        encoding: &'static Encoding,
    ) -> Task<PreviewMessage> {
        self.reopen_text_document(TextEncodingChoice::Chosen(encoding))
    }

    /// 当前文本预览的文件监视；没有文本预览时没有订阅。
    pub fn text_file_watch_subscription(&self) -> Subscription<PreviewMessage> {
        match self.active_text_preview_document() {
            Some(document) => text_file_watch_subscription(document.id(), document.path()),
            None => Subscription::none(),
        }
    }

    fn accept_text_file_check(
        &mut self,
        outcome: Result<(FileChange, Option<crate::text_document::FileStamp>), String>,
    ) -> Task<PreviewMessage> {
        let Ok((change, stamp)) = outcome else {
            return Task::none();
        };
        let Some(document) = self.active_text_preview_document_mut() else {
            return Task::none();
        };
        match (change, stamp) {
            (FileChange::Appended { .. }, Some(stamp)) => {
                let reads = document.accept_append(stamp);
                text_read_tasks(document, reads)
            }
            (FileChange::Rewritten, _) => {
                let choice = if document.encoding_chosen() {
                    TextEncodingChoice::Chosen(document.encoding())
                } else {
                    TextEncodingChoice::Detect
                };
                self.reopen_text_document(choice)
            }
            // 被删：已打开的句柄还能读，继续显示（第六轮决定）。
            _ => Task::none(),
        }
    }

    /// 同一路径重新打开（文件改写、改编码），落点是原视口顶部那一行，原来贴着末尾的到末尾。
    fn reopen_text_document(&mut self, choice: TextEncodingChoice) -> Task<PreviewMessage> {
        let Some(document) = self.active_text_preview_document_mut() else {
            return Task::none();
        };
        let jump = match document.viewport() {
            Some(viewport) if viewport.at_bottom => TextJump::End,
            Some(viewport) => TextJump::Offset(viewport.first_row),
            None => TextJump::Start,
        };
        let path = document.path().to_path_buf();
        let format = document.format();
        let document_id = document.id();
        Task::perform(
            reopen_text_preview(path, choice, format, jump),
            move |outcome| PreviewMessage::TextRead {
                document: document_id,
                reply: TextReadReply(TextReadReplyKind::Reopened(outcome.map(Arc::new))),
            },
        )
    }

    fn accept_reopened_text(
        &mut self,
        outcome: Result<Arc<TextOpened>, String>,
    ) -> Task<PreviewMessage> {
        let Some(previous) = self.active_text_preview_document_mut() else {
            return Task::none();
        };
        let path = previous.path().to_path_buf();
        let mode = previous.markdown_preview_mode();
        match outcome {
            Ok(opened) => {
                let task = self.open_text_document(path.clone(), &opened);
                if let Some(document) = self.text_preview_document.as_mut() {
                    document.select_markdown_preview_mode(mode);
                }
                self.preview = Some(PreviewState::Ready(PreviewContent::Text { path, opened }));
                task
            }
            Err(error) => {
                self.text_preview_document = None;
                self.preview = Some(PreviewState::Error(error));
                Task::none()
            }
        }
    }

    pub fn active_text_preview_document_mut(&mut self) -> Option<&mut TextPreviewDocument> {
        let path = active_text_preview_path(self.preview.as_ref())?.to_path_buf();
        self.text_preview_document
            .as_mut()
            .filter(|document| document.path() == path)
    }

    pub fn active_text_preview_document(&self) -> Option<&TextPreviewDocument> {
        let path = active_text_preview_path(self.preview.as_ref())?;
        self.text_preview_document
            .as_ref()
            .filter(|document| document.path() == path)
    }
}

/// 文档要的读取变成后台任务。
fn text_read_tasks(document: &TextPreviewDocument, reads: Vec<TextRead>) -> Task<PreviewMessage> {
    let document_id = document.id();
    let source = document.source();
    let decoding = document.decoding();
    let len = document.len();
    let path = document.path().to_path_buf();
    Task::batch(reads.into_iter().map(|read| match read {
        TextRead::Rows {
            generation,
            request,
        } => {
            let source = Arc::clone(&source);
            read_task(document_id, move || TextReadReplyKind::Rows {
                generation,
                request,
                outcome: read_rows(source.as_ref(), decoding, len, request)
                    .map_err(|error| error.to_string()),
            })
        }
        TextRead::Number {
            generation,
            line_start,
            known,
        } => {
            let source = Arc::clone(&source);
            read_task(document_id, move || {
                let reader = RowReader {
                    source: source.as_ref(),
                    decoding,
                    len,
                };
                TextReadReplyKind::Number {
                    generation,
                    line_start,
                    outcome: reader
                        .lines_before(line_start, known)
                        .map_err(|error| error.to_string()),
                }
            })
        }
        TextRead::Count {
            start,
            first_checkpoint,
            len,
            cancel,
        } => Task::run(
            count_lines(
                Arc::clone(&source),
                decoding,
                len,
                start,
                first_checkpoint,
                cancel,
            ),
            move |progress| PreviewMessage::TextRead {
                document: document_id,
                reply: TextReadReply(TextReadReplyKind::Index(progress)),
            },
        ),
        TextRead::Highlight(start) => Task::perform(
            highlight_worker(Arc::clone(&source), decoding, path.clone(), len, start),
            move |language| PreviewMessage::TextRead {
                document: document_id,
                reply: TextReadReply(TextReadReplyKind::HighlightLanguage(language)),
            },
        ),
        // 被更新的请求替换（回复取消）时没有消息。
        TextRead::HighlightRows(reply) => Task::run(
            stream::once(reply).filter_map(|rows| future::ready(rows.ok())),
            move |rows| PreviewMessage::TextRead {
                document: document_id,
                reply: TextReadReply(TextReadReplyKind::HighlightRows(rows)),
            },
        ),
    }))
}

/// 阻塞读取放进线程池，结果包成消息。
fn read_task(
    document_id: u64,
    work: impl FnOnce() -> TextReadReplyKind + Send + 'static,
) -> Task<PreviewMessage> {
    Task::perform(
        async move {
            match tokio::task::spawn_blocking(work).await {
                Ok(reply) => reply,
                // 读取代码里的 panic 是缺陷，照样抛出；只有运行时关闭时才会被取消。
                Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
                Err(error) => TextReadReplyKind::Selection {
                    target: ClipboardTarget::Standard,
                    outcome: Err(error.to_string()),
                },
            }
        },
        move |reply| PreviewMessage::TextRead {
            document: document_id,
            reply: TextReadReply(reply),
        },
    )
}

fn active_text_preview_path(preview: Option<&PreviewState>) -> Option<&Path> {
    match preview? {
        PreviewState::Ready(PreviewContent::Text { path, .. }) => Some(path.as_path()),
        _ => None,
    }
}

#[cfg(test)]
#[path = "text_tests.rs"]
mod tests;
