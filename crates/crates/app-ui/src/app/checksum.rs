//! 文件校验面板:状态、消息处理与后台计算命令。
//!
//! 计算跑在 tokio 后台,进度经 stream 通道转发进 UI;每次计算持有全局唯一
//! 代际号,面板重开、切换文件或取消后,迟到消息按代际丢弃,不会污染新计算。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use file_core::{
    algorithms_matching_digest, compute_file_checksums, find_checksum_entry, is_plausible_digest,
    parse_checksum_file, ChecksumAlgorithm, ChecksumFileContent, FileChecksums, FileKind,
};
use iced::futures::SinkExt;
use iced::{stream, Task};
use tokio_util::sync::CancellationToken;

use super::FileBrowser;
use crate::model::Message;

/// 校验代际号的全局计数器:跨面板实例单调递增,保证迟到消息永不串扰。
static NEXT_CHECKSUM_GENERATION: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChecksumComputation {
    Computing { bytes_done: u64, total_bytes: u64 },
    Completed(FileChecksums),
    Canceled,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChecksumFailure {
    Canceled,
    Message(String),
}

/// 已加载的校验文件:标准条目列表或裸单值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChecksumFileVerification {
    path: PathBuf,
    content: ChecksumFileContent,
}

impl ChecksumFileVerification {
    pub(crate) fn path(&self) -> &std::path::Path {
        &self.path
    }

    pub(crate) fn content(&self) -> &ChecksumFileContent {
        &self.content
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ChecksumState {
    files: Vec<PathBuf>,
    active_index: usize,
    generation: u64,
    cancel_token: CancellationToken,
    computation: ChecksumComputation,
    expected_text: String,
    checksum_file: Option<ChecksumFileVerification>,
    last_copied: Option<ChecksumAlgorithm>,
}

impl ChecksumState {
    fn new(files: Vec<PathBuf>) -> Self {
        Self {
            files,
            active_index: 0,
            generation: 0,
            cancel_token: CancellationToken::new(),
            computation: ChecksumComputation::Computing {
                bytes_done: 0,
                total_bytes: 0,
            },
            expected_text: String::new(),
            checksum_file: None,
            last_copied: None,
        }
    }

    pub(crate) fn files(&self) -> &[PathBuf] {
        &self.files
    }

    pub(crate) fn active_index(&self) -> usize {
        self.active_index
    }

    pub(crate) fn active_file(&self) -> &std::path::Path {
        &self.files[self.active_index]
    }

    pub(crate) fn computation(&self) -> &ChecksumComputation {
        &self.computation
    }

    pub(crate) fn expected_text(&self) -> &str {
        &self.expected_text
    }

    pub(crate) fn checksum_file(&self) -> Option<&ChecksumFileVerification> {
        self.checksum_file.as_ref()
    }

    pub(crate) fn last_copied(&self) -> Option<ChecksumAlgorithm> {
        self.last_copied
    }

    /// 启动当前文件的计算;上一次计算(若有)被取消并按新代际号失效。
    fn begin_active_computation(&mut self) -> Task<Message> {
        self.cancel_token.cancel();
        self.generation = NEXT_CHECKSUM_GENERATION.fetch_add(1, Ordering::Relaxed);
        let cancel_token = CancellationToken::new();
        self.cancel_token = cancel_token.clone();
        self.computation = ChecksumComputation::Computing {
            bytes_done: 0,
            total_bytes: 0,
        };
        checksum_compute_command(
            self.files[self.active_index].clone(),
            self.generation,
            cancel_token,
        )
    }

    /// 期望值输入的比对结论;计算未完成时为待定。
    pub(crate) fn expected_value_verdict(&self) -> ChecksumExpectedVerdict {
        if self.expected_text.trim().is_empty() {
            return ChecksumExpectedVerdict::Empty;
        }
        if !is_plausible_digest(&self.expected_text) {
            return ChecksumExpectedVerdict::Invalid;
        }
        let ChecksumComputation::Completed(digests) = &self.computation else {
            return ChecksumExpectedVerdict::Pending;
        };
        let matched = algorithms_matching_digest(digests, &self.expected_text);
        if matched.is_empty() {
            ChecksumExpectedVerdict::NoMatch
        } else {
            ChecksumExpectedVerdict::Matched(matched)
        }
    }

    /// 校验文件的比对结论;未加载时为 None,计算未完成时为待定。
    pub(crate) fn checksum_file_verdict(&self) -> Option<ChecksumFileVerdict> {
        let verification = self.checksum_file.as_ref()?;
        let ChecksumComputation::Completed(digests) = &self.computation else {
            return Some(ChecksumFileVerdict::Pending);
        };
        Some(match verification.content() {
            ChecksumFileContent::BareHash(expected) => {
                if algorithms_matching_digest(digests, expected).is_empty() {
                    ChecksumFileVerdict::BareMismatch
                } else {
                    ChecksumFileVerdict::BareMatched
                }
            }
            ChecksumFileContent::Entries(entries) => {
                match find_checksum_entry(entries, self.active_file()) {
                    None => ChecksumFileVerdict::EntryNotFound,
                    Some(entry) => {
                        if algorithms_matching_digest(digests, &entry.hash_hex).is_empty() {
                            ChecksumFileVerdict::EntryMismatch
                        } else {
                            ChecksumFileVerdict::EntryMatched
                        }
                    }
                }
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChecksumExpectedVerdict {
    Empty,
    Invalid,
    Pending,
    NoMatch,
    Matched(Vec<ChecksumAlgorithm>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChecksumFileVerdict {
    Pending,
    /// 裸单值校验文件与当前文件一致。
    BareMatched,
    BareMismatch,
    /// 按文件名命中的条目与当前文件一致。
    EntryMatched,
    EntryMismatch,
    /// 校验文件里没有当前文件名的条目。
    EntryNotFound,
}

#[derive(Debug, Clone)]
pub(crate) enum ChecksumMessage {
    OpenSelected,
    FileSelected(usize),
    Progress {
        generation: u64,
        bytes_done: u64,
        total_bytes: u64,
    },
    Completed {
        generation: u64,
        result: Result<FileChecksums, ChecksumFailure>,
    },
    /// 取消或失败后重新计算当前文件。
    RetryPressed,
    ExpectedValueChanged(String),
    HashCopyRequested(ChecksumAlgorithm),
    HashCopied(Result<(), String>),
    ChecksumFileLoadPressed,
    ChecksumFileLoaded(ChecksumFileLoad),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChecksumFileLoad {
    Canceled,
    Loaded {
        path: PathBuf,
        content: ChecksumFileContent,
    },
    Failed(String),
}

impl FileBrowser {
    pub(super) fn handle_checksum_message(&mut self, message: ChecksumMessage) -> Task<Message> {
        match message {
            ChecksumMessage::OpenSelected => self.open_checksum(),
            ChecksumMessage::FileSelected(index) => self.select_checksum_file(index),
            ChecksumMessage::Progress {
                generation,
                bytes_done,
                total_bytes,
            } => {
                self.update_checksum_progress(generation, bytes_done, total_bytes);
                Task::none()
            }
            ChecksumMessage::Completed { generation, result } => {
                self.finish_checksum_computation(generation, result)
            }
            ChecksumMessage::RetryPressed => {
                if let Some(state) = &mut self.checksum {
                    return state.begin_active_computation();
                }
                Task::none()
            }
            ChecksumMessage::ExpectedValueChanged(text) => {
                if let Some(state) = &mut self.checksum {
                    state.expected_text = text;
                }
                Task::none()
            }
            ChecksumMessage::HashCopyRequested(algorithm) => self.copy_checksum_digest(algorithm),
            ChecksumMessage::HashCopied(Ok(())) => Task::none(),
            ChecksumMessage::HashCopied(Err(error)) => {
                self.show_global_error(error);
                Task::none()
            }
            ChecksumMessage::ChecksumFileLoadPressed => checksum_file_pick_command(),
            ChecksumMessage::ChecksumFileLoaded(load) => {
                self.accept_checksum_file_load(load);
                Task::none()
            }
        }
    }

    fn open_checksum(&mut self) -> Task<Message> {
        if self.is_trash_view {
            return Task::none();
        }
        let files: Vec<PathBuf> = self
            .selected_paths_for_operation()
            .into_iter()
            .filter(|path| self.entry_kind(path) == Some(FileKind::File))
            .collect();
        if files.is_empty() {
            return Task::none();
        }

        self.context_menu = None;
        self.open_with = None;
        self.archive_creation = None;
        self.archive_extraction = None;
        self.shortcut_capture = None;
        self.operation_queue.close_panel();
        self.cancel_file_drag_interaction();
        self.sidebar_bookmark_drag = None;
        self.sidebar_bookmark_drop_slot = None;
        self.selection_marquee = None;
        let _ = self.cancel_address_editing();

        // 面板重开时停掉旧面板仍在跑的计算,避免浪费 I/O。
        if let Some(previous) = self.checksum.take() {
            previous.cancel_token.cancel();
        }

        let mut state = ChecksumState::new(files);
        let task = state.begin_active_computation();
        self.checksum = Some(state);
        task
    }

    fn select_checksum_file(&mut self, index: usize) -> Task<Message> {
        let Some(state) = &mut self.checksum else {
            return Task::none();
        };
        if index >= state.files.len() || index == state.active_index {
            return Task::none();
        }
        state.active_index = index;
        state.last_copied = None;
        state.begin_active_computation()
    }

    fn update_checksum_progress(&mut self, generation: u64, bytes_done: u64, total_bytes: u64) {
        let Some(state) = &mut self.checksum else {
            return;
        };
        if state.generation != generation {
            return;
        }
        if let ChecksumComputation::Computing {
            bytes_done: current_done,
            total_bytes: current_total,
        } = &mut state.computation
        {
            *current_done = bytes_done;
            *current_total = total_bytes;
        }
    }

    fn finish_checksum_computation(
        &mut self,
        generation: u64,
        result: Result<FileChecksums, ChecksumFailure>,
    ) -> Task<Message> {
        let Some(state) = &mut self.checksum else {
            return Task::none();
        };
        if state.generation != generation {
            return Task::none();
        }
        state.computation = match result {
            Ok(digests) => ChecksumComputation::Completed(digests),
            Err(ChecksumFailure::Canceled) => ChecksumComputation::Canceled,
            Err(ChecksumFailure::Message(message)) => ChecksumComputation::Failed(message),
        };
        Task::none()
    }

    fn copy_checksum_digest(&mut self, algorithm: ChecksumAlgorithm) -> Task<Message> {
        let Some(state) = &mut self.checksum else {
            return Task::none();
        };
        let ChecksumComputation::Completed(digests) = &state.computation else {
            return Task::none();
        };
        let digest = digests.digest(algorithm).to_owned();
        state.last_copied = Some(algorithm);
        write_desktop_clipboard_text_command(digest)
    }

    fn accept_checksum_file_load(&mut self, load: ChecksumFileLoad) {
        match load {
            ChecksumFileLoad::Canceled => {}
            ChecksumFileLoad::Loaded { path, content } => {
                if let Some(state) = &mut self.checksum {
                    state.checksum_file = Some(ChecksumFileVerification { path, content });
                }
            }
            ChecksumFileLoad::Failed(error) => self.show_global_error(error),
        }
    }
}

/// 关闭面板时停掉仍在跑的计算,由 dismiss_floating 调用。
pub(super) fn cancel_running_checksum(state: &ChecksumState) {
    state.cancel_token.cancel();
}

fn checksum_compute_command(
    path: PathBuf,
    generation: u64,
    cancel_token: CancellationToken,
) -> Task<Message> {
    let (progress_sender, mut progress_receiver) = tokio::sync::mpsc::channel(8);
    let compute_handle = tokio::spawn(compute_file_checksums(
        path.clone(),
        progress_sender,
        cancel_token,
    ));
    Task::stream(stream::channel(16, async move |mut output| {
        while let Some(progress) = progress_receiver.recv().await {
            if output
                .send(Message::Checksum(ChecksumMessage::Progress {
                    generation,
                    bytes_done: progress.bytes_done,
                    total_bytes: progress.total_bytes,
                }))
                .await
                .is_err()
            {
                return;
            }
        }
        // 进度通道关闭说明计算任务已结束;取消在结果里如实上报。
        let result = match compute_handle.await {
            Ok(Ok(digests)) => Ok(digests),
            Ok(Err(file_core::FileError::Cancelled)) => Err(ChecksumFailure::Canceled),
            Ok(Err(error)) => Err(ChecksumFailure::Message(format!(
                "{}: {error}",
                crate::localization::translate_current("Failed to compute checksum")
            ))),
            Err(join_error) => Err(ChecksumFailure::Message(join_error.to_string())),
        };
        let _ = output
            .send(Message::Checksum(ChecksumMessage::Completed {
                generation,
                result,
            }))
            .await;
    }))
}

fn write_desktop_clipboard_text_command(text: String) -> Task<Message> {
    Task::perform(
        desktop_linux::write_desktop_clipboard_text(text),
        |result| {
            Message::Checksum(ChecksumMessage::HashCopied(
                result.map_err(|error| error.to_string()),
            ))
        },
    )
}

fn checksum_file_pick_command() -> Task<Message> {
    Task::perform(choose_checksum_file(), Message::Checksum)
}

async fn choose_checksum_file() -> ChecksumMessage {
    let load = async {
        let title = crate::localization::translate_current("Load checksum file");
        let accept_label = crate::localization::translate_current("Load");
        let request = ashpd::desktop::file_chooser::SelectedFiles::open_file()
            .title(title.as_str())
            .accept_label(accept_label.as_str())
            .modal(true)
            .multiple(false)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let selected = match request.response() {
            Ok(selected) => selected,
            Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)) => {
                return Ok(ChecksumFileLoad::Canceled)
            }
            Err(error) => return Err(error.to_string()),
        };
        let path = selected
            .uris()
            .first()
            .and_then(|uri| uri.to_file_path().ok())
            .ok_or_else(|| "the selected checksum file is not a local file".to_owned())?;
        let text = tokio::fs::read_to_string(&path)
            .await
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        let content = parse_checksum_file(&text)
            .map_err(|message| format!("{}: {message}", path.display()))?;
        Ok(ChecksumFileLoad::Loaded { path, content })
    }
    .await;
    ChecksumMessage::ChecksumFileLoaded(match load {
        Ok(load) => load,
        Err(error) => ChecksumFileLoad::Failed(error),
    })
}
