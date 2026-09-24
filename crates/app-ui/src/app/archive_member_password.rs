//! 包内成员密码弹窗:成员提取(队列失败补救)与双击打开(物化失败补救)
//! 共用同一状态机与弹窗视图。与整包解压的 ArchiveExtractionState 同构,
//! 但不做「先检查后执行」——成员级只能失败后补救,密码正确性由重试
//! 操作本身裁决。

use std::fmt;
use std::path::PathBuf;

use file_core::ArchivePassword;
use iced::Task;

use super::archive_password::ArchivePasswordDraft;
use super::FileBrowser;
use crate::model::Message;
use crate::operation_queue::QueuedFileOperation;

/// 弹窗绑定的重试动作:提交后带密码重走对应管线。提取动作重新入队,
/// 打开动作带密码重新物化并打开——两种动作共用一套弹窗,不复制两套。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ArchiveMemberPasswordAction {
    Extract {
        sources: Vec<PathBuf>,
        destination: PathBuf,
    },
    Open {
        path: PathBuf,
    },
}

#[derive(Debug, Clone)]
pub(crate) enum ArchiveMemberPasswordMessage {
    PasswordChanged(ArchivePasswordDraft),
    Submitted,
    /// Open 重试命令的终态:Opened 关闭弹窗;InvalidPassword 保留弹窗
    /// 并提示密码错误;Failed 关闭弹窗并转全局错误。
    OpenFinished {
        path: PathBuf,
        outcome: ArchiveMemberOpenOutcome,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ArchiveMemberOpenOutcome {
    Opened,
    InvalidPassword,
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArchiveMemberPasswordPhase {
    WaitingForPassword,
    /// Open 重试在途:物化命令未返回前禁用提交,防止重复物化。
    RetryingOpen,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ArchiveMemberPasswordState {
    action: ArchiveMemberPasswordAction,
    password: ArchivePasswordDraft,
    validation_error: Option<String>,
    phase: ArchiveMemberPasswordPhase,
}

impl ArchiveMemberPasswordState {
    /// 弹窗入口。invalid_retry=true 表示上一次密码不对,直接带
    /// 「Incorrect password」提示;队列失败通道在弹窗已被提交关闭的
    /// 情况下也会走到这里(重新开一个新弹窗)。
    pub(crate) fn requested(action: ArchiveMemberPasswordAction, invalid_retry: bool) -> Self {
        Self {
            action,
            password: ArchivePasswordDraft::new(String::new()),
            validation_error: invalid_retry.then(|| "Incorrect password. Try again.".to_owned()),
            phase: ArchiveMemberPasswordPhase::WaitingForPassword,
        }
    }

    pub(crate) fn action(&self) -> &ArchiveMemberPasswordAction {
        &self.action
    }

    pub(crate) fn password(&self) -> &ArchivePasswordDraft {
        &self.password
    }

    pub(crate) fn validation_error(&self) -> Option<&str> {
        self.validation_error.as_deref()
    }

    pub(crate) fn can_submit_password(&self) -> bool {
        self.phase == ArchiveMemberPasswordPhase::WaitingForPassword
    }

    fn update_password(&mut self, password: ArchivePasswordDraft) {
        if self.can_submit_password() {
            self.password = password;
            self.validation_error = None;
        }
    }

    fn archive_password(&self) -> Option<ArchivePassword> {
        self.password.to_archive_password()
    }

    fn wait_for_password(&mut self, validation_error: Option<String>) {
        self.phase = ArchiveMemberPasswordPhase::WaitingForPassword;
        self.validation_error = validation_error;
    }

    fn begin_open_retry(&mut self) {
        self.phase = ArchiveMemberPasswordPhase::RetryingOpen;
        self.validation_error = None;
    }
}

impl fmt::Debug for ArchiveMemberPasswordState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ArchiveMemberPasswordState")
            .field("action", &self.action)
            .field("password", &self.password)
            .field("validation_error", &self.validation_error)
            .field("phase", &self.phase)
            .finish()
    }
}

impl FileBrowser {
    pub(super) fn handle_archive_member_password_message(
        &mut self,
        message: ArchiveMemberPasswordMessage,
    ) -> Task<Message> {
        match message {
            ArchiveMemberPasswordMessage::PasswordChanged(password) => {
                if let Some(state) = self.archive_member_password.as_mut() {
                    state.update_password(password);
                }
                Task::none()
            }
            ArchiveMemberPasswordMessage::Submitted => self.submit_archive_member_password(),
            ArchiveMemberPasswordMessage::OpenFinished { path, outcome } => {
                self.accept_archive_member_open_retry(path, outcome)
            }
        }
    }

    /// 队列失败/双击物化失败请求弹窗:与整包解压弹窗互斥,复用同一套
    /// 模态互斥清理(关上下文菜单、收起队列面板、取消拖拽等)。
    pub(super) fn request_archive_member_password(
        &mut self,
        action: ArchiveMemberPasswordAction,
        invalid_retry: bool,
    ) -> Task<Message> {
        self.clear_state_for_archive_extraction();
        self.archive_member_password =
            Some(ArchiveMemberPasswordState::requested(action, invalid_retry));
        Task::none()
    }

    fn submit_archive_member_password(&mut self) -> Task<Message> {
        let Some(state) = self.archive_member_password.as_mut() else {
            return Task::none();
        };
        if !state.can_submit_password() {
            return Task::none();
        }
        let Some(password) = state.archive_password() else {
            state.wait_for_password(Some("Enter the archive password.".to_owned()));
            return Task::none();
        };

        match state.action().clone() {
            ArchiveMemberPasswordAction::Extract {
                sources,
                destination,
            } => {
                // 提交即入队并关闭弹窗;密码错误由队列失败通道再弹
                // (invalid_retry=true),与整包解压的检查-重试循环等价。
                self.archive_member_password = None;
                self.enqueue_file_operation(QueuedFileOperation::ExtractArchiveMembers {
                    sources,
                    destination,
                    password: Some(password),
                })
            }
            ArchiveMemberPasswordAction::Open { path } => {
                state.begin_open_retry();
                crate::commands::open_archive_member_with_password_command(
                    path,
                    password,
                    self.terminal_emulator,
                )
            }
        }
    }

    fn accept_archive_member_open_retry(
        &mut self,
        path: PathBuf,
        outcome: ArchiveMemberOpenOutcome,
    ) -> Task<Message> {
        // 迟到结果与弹窗归属校验:弹窗已被用户取消(重试在途时 Esc/取消),
        // 或已切换为其它动作(其它成员/提取动作)时,本次 Open 重试结果
        // 一律丢弃——不重新弹窗、不反向报错,与整包解压「弹窗关掉后丢弃
        // 检查结果」同一语义。三种终态共用同一个归属判定,避免只拦
        // InvalidPassword 而 Opened/Failed 误关别的弹窗。
        let Some(state) = self.archive_member_password.as_mut() else {
            return Task::none();
        };
        if !matches!(
            state.action(),
            ArchiveMemberPasswordAction::Open { path: open_path }
                if *open_path == path
        ) {
            return Task::none();
        }
        match outcome {
            ArchiveMemberOpenOutcome::Opened => {
                self.archive_member_password = None;
                self.clear_global_error();
                Task::none()
            }
            ArchiveMemberOpenOutcome::InvalidPassword => {
                state.wait_for_password(Some("Incorrect password. Try again.".to_owned()));
                Task::none()
            }
            ArchiveMemberOpenOutcome::Failed(error) => {
                self.archive_member_password = None;
                self.show_global_error(error);
                Task::none()
            }
        }
    }
}
