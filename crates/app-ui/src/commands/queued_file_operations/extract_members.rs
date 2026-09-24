//! 包内成员批量提取的队列执行：复制/拖出到真实目录的落地动作。

use std::path::PathBuf;

use file_core::{
    extract_archive_members_with_controls_and_progress, ArchiveMemberExtractionRequest,
    ArchivePassword, FileError, FileOperationControls,
};
use iced::futures::SinkExt;

use crate::app::archive_member_password::ArchiveMemberPasswordAction;
use crate::model::Message;
use crate::operation_history::FileOperationOutcome;
use crate::operation_progress::FileOperationProgressUpdate;

type IcedSender = iced::futures::channel::mpsc::Sender<Message>;

pub(crate) async fn run_queued_extract_archive_members(
    sources: Vec<PathBuf>,
    destination: PathBuf,
    password: Option<ArchivePassword>,
    controls: FileOperationControls,
    task_id: u64,
    output: &mut IcedSender,
) -> Result<FileOperationOutcome, String> {
    let _ = output
        .send(Message::FileOperationProgressed(
            task_id,
            FileOperationProgressUpdate::Indeterminate,
            Vec::new(),
        ))
        .await;
    let request = ArchiveMemberExtractionRequest::new(sources.clone(), destination.clone())
        .with_password(password);
    let outcome =
        extract_archive_members_with_controls_and_progress(request, controls, |_| {}).await;
    match outcome {
        Ok(_) => Ok(FileOperationOutcome::NoHistory),
        Err(error) => {
            // FileError 在队列完成边界会被字符串化,这里是唯一还能区分
            // 密码错误的位置:向 UI 发弹窗请求后照常按失败收尾,让用户
            // 带密码重新入队(取消则操作就地结束)。
            if matches!(
                error,
                FileError::ArchivePasswordRequired { .. }
                    | FileError::ArchiveInvalidPassword { .. }
            ) {
                let invalid_retry = matches!(error, FileError::ArchiveInvalidPassword { .. });
                let _ = output
                    .send(Message::ArchiveMemberPasswordRequested {
                        action: ArchiveMemberPasswordAction::Extract {
                            sources,
                            destination,
                        },
                        invalid_retry,
                    })
                    .await;
            }
            Err(error.to_string())
        }
    }
}
