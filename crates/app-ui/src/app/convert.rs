use std::path::{Path, PathBuf};

use file_core::{available_conversion_encoders, ConversionEncoderAvailability};
use iced::Task;

use super::FileBrowser;
use crate::model::{
    BrowserLocation, ConvertEdit, ConvertSource, ConvertState, ConvertibleMediaKind, Message,
};
use file_operations::QueuedFileOperation;

#[derive(Debug, Clone)]
pub(crate) enum ConvertMessage {
    OpenSelected,
    SourcesClassified(Vec<ConvertSource>),
    EncodersProbed(ConversionEncoderAvailability),
    Edit(ConvertEdit),
    OutputDirectoryBrowse,
    OutputDirectoryChosen(Result<Option<PathBuf>, String>),
    Submitted,
}

impl FileBrowser {
    pub(super) fn handle_convert_message(&mut self, message: ConvertMessage) -> Task<Message> {
        match message {
            ConvertMessage::OpenSelected => self.open_convert(),
            ConvertMessage::SourcesClassified(sources) => self.show_convert(sources),
            // 探测结果和选文件夹结果是异步回来的，对话框可能已经关了。
            ConvertMessage::EncodersProbed(encoders) => {
                if let Some(state) = &mut self.convert {
                    state.accept_encoder_probe(encoders);
                }
                Task::none()
            }
            ConvertMessage::Edit(edit) => {
                if let Some(state) = &mut self.convert {
                    state.apply(edit);
                }
                Task::none()
            }
            ConvertMessage::OutputDirectoryBrowse => Task::perform(
                crate::commands::choose_directory(tr!("Choose output folder")),
                |outcome| Message::Convert(ConvertMessage::OutputDirectoryChosen(outcome)),
            ),
            ConvertMessage::OutputDirectoryChosen(Ok(Some(directory))) => {
                Task::done(Message::Convert(ConvertMessage::Edit(
                    ConvertEdit::OutputDirectory(directory.display().to_string()),
                )))
            }
            ConvertMessage::OutputDirectoryChosen(Ok(None)) => Task::none(),
            ConvertMessage::OutputDirectoryChosen(Err(error)) => {
                self.show_global_error(error);
                Task::none()
            }
            ConvertMessage::Submitted => self.submit_convert(),
        }
    }

    fn open_convert(&mut self) -> Task<Message> {
        if self.tab().location == BrowserLocation::Trash {
            return Task::none();
        }
        Task::perform(
            classify_convert_sources(self.selected_paths_for_operation()),
            |sources| Message::Convert(ConvertMessage::SourcesClassified(sources)),
        )
    }

    fn show_convert(&mut self, sources: Vec<ConvertSource>) -> Task<Message> {
        let Some(state) = ConvertState::new(sources, &self.user_config.convert_defaults) else {
            return Task::none();
        };

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

        self.convert = Some(state);
        Task::perform(available_conversion_encoders(), |encoders| {
            Message::Convert(ConvertMessage::EncodersProbed(encoders))
        })
    }

    /// 提交按钮只在 `requests()` 成功时可点，这里失败说明对话框已经变了，不提交。
    fn submit_convert(&mut self) -> Task<Message> {
        let Some(state) = &self.convert else {
            return Task::none();
        };
        let Ok(requests) = state.requests() else {
            return Task::none();
        };
        let remembered = state.remembered(&self.user_config.convert_defaults);
        self.convert = None;
        let enqueue = self.enqueue_file_operation(QueuedFileOperation::Convert { requests });
        if remembered == self.user_config.convert_defaults {
            return enqueue;
        }
        self.user_config.convert_defaults = remembered;
        Task::batch([enqueue, self.persist_user_preferences_command()])
    }
}

/// 名字定不下来的视频后缀（`.ts`）要读开头内容，放到后台线程：网络挂载卡住也不冻界面。
/// 大小顺手在同一趟里读，给分区小标题用；读不到按 0 算，不挡转换。
async fn classify_convert_sources(paths: Vec<PathBuf>) -> Vec<ConvertSource> {
    tokio::task::spawn_blocking(move || {
        paths
            .into_iter()
            .filter_map(|path| {
                let kind = convertible_media_kind(&path)?;
                let bytes = std::fs::metadata(&path).map_or(0, |metadata| metadata.len());
                Some(ConvertSource { path, kind, bytes })
            })
            .collect()
    })
    .await
    .unwrap_or_else(|error| {
        tracing::warn!(
            target: "app_ui::convert",
            error = %error,
            "convert source classification worker failed"
        );
        Vec::new()
    })
}

/// 转换源按文件类型分类。SVG 是矢量、相机 RAW 只有自带的预览图，引擎都没法当原片转，
/// 不进入对话框。
fn convertible_media_kind(path: &Path) -> Option<ConvertibleMediaKind> {
    match file_core::media_by_name(path) {
        file_core::MediaByName::Image(file_core::ImageKind::Raster(
            file_core::RasterSource::Picture,
        )) => Some(ConvertibleMediaKind::Image),
        file_core::MediaByName::Video => Some(ConvertibleMediaKind::Video),
        file_core::MediaByName::MaybeVideo => {
            file_core::is_video_file(path).then_some(ConvertibleMediaKind::Video)
        }
        file_core::MediaByName::Audio => Some(ConvertibleMediaKind::Audio),
        file_core::MediaByName::Image(_) | file_core::MediaByName::Other => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convertible_media_kind_skips_svg_and_camera_raw() {
        for path in ["/tmp/photo.png", "/tmp/phone.heic", "/tmp/layers.psd"] {
            assert_eq!(
                convertible_media_kind(Path::new(path)),
                Some(ConvertibleMediaKind::Image),
                "{path}"
            );
        }
        assert_eq!(convertible_media_kind(Path::new("/tmp/logo.svg")), None);
        assert_eq!(convertible_media_kind(Path::new("/tmp/shot.cr2")), None);
        assert_eq!(convertible_media_kind(Path::new("/tmp/notes.txt")), None);
    }

    #[test]
    fn convertible_media_kind_follows_the_shared_video_rule() {
        assert_eq!(
            convertible_media_kind(Path::new("/tmp/old.rmvb")),
            Some(ConvertibleMediaKind::Video)
        );
        // .ts 看内容：电视录像能转，代码不进对话框。
        let directory = tempfile::tempdir().expect("temp dir");
        let recording = directory.path().join("recording.ts");
        let mut transport_stream = vec![0; 188 * 5];
        for packet in transport_stream.chunks_mut(188) {
            packet[0] = 0x47;
        }
        std::fs::write(&recording, transport_stream).expect("write recording");
        let code = directory.path().join("component.ts");
        std::fs::write(&code, "export const answer: number = 42;\n").expect("write code");
        assert_eq!(
            convertible_media_kind(&recording),
            Some(ConvertibleMediaKind::Video)
        );
        assert_eq!(convertible_media_kind(&code), None);
    }

    #[tokio::test]
    async fn classification_reads_sizes_and_drops_unconvertible_files() {
        let directory = tempfile::tempdir().expect("temp dir");
        let photo = directory.path().join("photo.png");
        std::fs::write(&photo, [0u8; 42]).expect("write photo");
        let notes = directory.path().join("notes.txt");
        std::fs::write(&notes, "hi").expect("write notes");

        let sources = classify_convert_sources(vec![photo.clone(), notes]).await;
        assert_eq!(
            sources,
            vec![ConvertSource {
                path: photo,
                kind: ConvertibleMediaKind::Image,
                bytes: 42,
            }]
        );
    }
}
