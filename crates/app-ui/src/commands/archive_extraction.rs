use desktop_linux::{open_path_with_terminal_emulator, TerminalEmulator};
use file_core::{inspect_archive_extraction, ArchiveExtractionRequest, ArchivePassword, FileError};
use iced::Task;
use tokio_util::sync::CancellationToken;

use crate::app::archive_extraction::{ArchiveExtractionInspection, ArchiveExtractionMessage};
use crate::app::archive_member_password::{ArchiveMemberOpenOutcome, ArchiveMemberPasswordMessage};
use crate::model::Message;

pub(crate) fn inspect_archive_extraction_command(
    request: ArchiveExtractionRequest,
) -> Task<Message> {
    let issued_request = request.clone();
    Task::perform(
        async move {
            inspect_archive_extraction(request, CancellationToken::new())
                .await
                .map(|_| ArchiveExtractionInspection::Ready)
                .unwrap_or_else(archive_extraction_inspection_from_error)
        },
        move |outcome| {
            Message::ArchiveExtraction(ArchiveExtractionMessage::Inspected {
                request: issued_request.clone(),
                outcome,
            })
        },
    )
}

fn archive_extraction_inspection_from_error(error: FileError) -> ArchiveExtractionInspection {
    match error {
        FileError::ArchivePasswordRequired { .. } => ArchiveExtractionInspection::PasswordRequired,
        FileError::ArchiveInvalidPassword { .. } => ArchiveExtractionInspection::InvalidPassword,
        error => ArchiveExtractionInspection::Failed(error.to_string()),
    }
}

/// 首次物化的分类结果:Opened 保持原有 OpenFileFinished 语义;
/// PasswordRequired 转弹窗请求,不带错误字符串出 Task。
pub(crate) enum MemberOpenOutcome {
    Opened(Result<(), String>),
    PasswordRequired { invalid_retry: bool },
}

/// 成员密码弹窗提交后的 Open 重试:带密码重新物化并打开。密码错误
/// 不关闭弹窗(回提示重试),其余失败关弹窗转全局错误。
pub(crate) fn open_archive_member_with_password_command(
    path: std::path::PathBuf,
    password: ArchivePassword,
    terminal_emulator: TerminalEmulator,
) -> Task<Message> {
    Task::perform(
        async move {
            let outcome = match file_core::materialize_archive_member_for_open(
                &path,
                Some(password),
                CancellationToken::new(),
            )
            .await
            {
                Ok(materialized) => {
                    match open_path_with_terminal_emulator(materialized, terminal_emulator).await {
                        Ok(()) => ArchiveMemberOpenOutcome::Opened,
                        Err(error) => ArchiveMemberOpenOutcome::Failed(error.to_string()),
                    }
                }
                Err(FileError::ArchiveInvalidPassword { .. }) => {
                    ArchiveMemberOpenOutcome::InvalidPassword
                }
                Err(error) => ArchiveMemberOpenOutcome::Failed(error.to_string()),
            };
            (path, outcome)
        },
        move |(path, outcome)| {
            Message::ArchiveMemberPassword(ArchiveMemberPasswordMessage::OpenFinished {
                path,
                outcome,
            })
        },
    )
}
