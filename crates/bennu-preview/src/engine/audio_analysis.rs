//! 当前这首的波形与声谱图：开始计算、接收结果、存进最近 3 首的缓存。网络挂载上的
//! 文件要不要自动算波形由宿主决定（只有宿主知道哪些路径在网络挂载上）：本地文件宿主
//! 在内容就绪后调 `request_audio_waveform`，网络上的等用户点按钮。

use std::path::Path;
use std::sync::Arc;

use iced::widget::image;
use iced::Task;

use crate::audio_preview::AudioWaveform;
use crate::commands::preview::{compute_audio_spectrogram_command, compute_audio_waveform_command};
use crate::preview::{
    AudioAnalysis, AudioAnalysisState, AudioTimelineView, PreviewContent, PreviewState,
};
use crate::preview_message::PreviewMessage;

impl super::PreviewEngine {
    /// 音频内容就绪时建这首的分析状态；缓存里有的直接就绪。替换掉的旧状态随 Drop 取消。
    pub(super) fn begin_audio_analysis(&mut self, path: &Path, len: u64) {
        let mut analysis = AudioAnalysis::new(path.to_path_buf(), len);
        if let Some(waveform) = self.audio_analysis_cache.waveform(path, len) {
            analysis.waveform = AudioAnalysisState::Ready(waveform);
        }
        if let Some(spectrogram) = self.audio_analysis_cache.spectrogram(path, len) {
            analysis.spectrogram = AudioAnalysisState::Ready(spectrogram);
        }
        self.audio_analysis = Some(analysis);
    }

    pub fn request_audio_waveform(&mut self) -> Task<PreviewMessage> {
        let Some(analysis) = self.audio_analysis.as_mut() else {
            return Task::none();
        };
        if !matches!(
            analysis.waveform,
            AudioAnalysisState::NotRequested | AudioAnalysisState::Failed(_)
        ) {
            return Task::none();
        }
        analysis.waveform = AudioAnalysisState::Computing;
        compute_audio_waveform_command(analysis.path.clone(), analysis.cancel_token())
    }

    pub(super) fn select_audio_timeline_view(
        &mut self,
        view: AudioTimelineView,
    ) -> Task<PreviewMessage> {
        let Some(analysis) = self.audio_analysis.as_mut() else {
            return Task::none();
        };
        analysis.view = view;
        if view == AudioTimelineView::Spectrogram
            && matches!(analysis.spectrogram, AudioAnalysisState::NotRequested)
        {
            analysis.spectrogram = AudioAnalysisState::Computing;
            return compute_audio_spectrogram_command(
                analysis.path.clone(),
                analysis.cancel_token(),
            );
        }
        Task::none()
    }

    pub(super) fn accept_audio_waveform(
        &mut self,
        path: &Path,
        outcome: Result<Arc<AudioWaveform>, String>,
    ) -> Task<PreviewMessage> {
        let Some(analysis) = self
            .audio_analysis
            .as_mut()
            .filter(|analysis| analysis.path == path)
        else {
            return Task::none();
        };
        let len = analysis.len;
        match outcome {
            Ok(waveform) => {
                self.audio_analysis_cache
                    .store_waveform(path, len, Arc::clone(&waveform));
                let duration = waveform.duration;
                analysis.waveform = AudioAnalysisState::Ready(waveform);
                // 整首解过一遍，长度是准的；ffprobe 只能估的文件拿它替换。
                if let Some(PreviewState::Ready(PreviewContent::Audio { details, .. })) =
                    &mut self.preview
                {
                    if details.probe.duration_is_estimate {
                        details.probe.duration = Some(duration);
                        details.probe.duration_is_estimate = false;
                    }
                }
            }
            Err(reason) => analysis.waveform = AudioAnalysisState::Failed(reason),
        }
        Task::none()
    }

    pub(super) fn accept_audio_spectrogram(
        &mut self,
        path: &Path,
        outcome: Result<image::Handle, String>,
    ) -> Task<PreviewMessage> {
        let Some(analysis) = self
            .audio_analysis
            .as_mut()
            .filter(|analysis| analysis.path == path)
        else {
            return Task::none();
        };
        match outcome {
            Ok(spectrogram) => {
                self.audio_analysis_cache.store_spectrogram(
                    path,
                    analysis.len,
                    spectrogram.clone(),
                );
                analysis.spectrogram = AudioAnalysisState::Ready(spectrogram);
            }
            Err(reason) => analysis.spectrogram = AudioAnalysisState::Failed(reason),
        }
        Task::none()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use super::*;
    use crate::preview::AudioAnalysisCache;

    fn waveform(seconds: u64) -> Arc<AudioWaveform> {
        Arc::new(AudioWaveform {
            peaks: vec![0.5; 4],
            duration: Duration::from_secs(seconds),
        })
    }

    #[test]
    fn cache_keeps_the_three_most_recent_songs_and_checks_the_size() {
        let mut cache = AudioAnalysisCache::default();
        for index in 0..4 {
            let path = PathBuf::from(format!("/m/{index}.flac"));
            cache.store_waveform(&path, 10, waveform(index));
        }
        // 最早那首被挤掉，其余三首还在。
        assert!(cache.waveform(Path::new("/m/0.flac"), 10).is_none());
        for index in 1..4 {
            assert!(cache
                .waveform(&PathBuf::from(format!("/m/{index}.flac")), 10)
                .is_some());
        }
        // 文件大小变了当作另一首。
        assert!(cache.waveform(Path::new("/m/3.flac"), 11).is_none());

        // 再碰一下最旧的，它就挪到最前，下一次挤掉的是别人。
        cache.store_spectrogram(
            Path::new("/m/1.flac"),
            10,
            image::Handle::from_rgba(1, 1, vec![0; 4]),
        );
        cache.store_waveform(Path::new("/m/9.flac"), 10, waveform(9));
        assert!(cache.waveform(Path::new("/m/1.flac"), 10).is_some());
        assert!(cache.waveform(Path::new("/m/2.flac"), 10).is_none());
    }

    #[test]
    fn dropping_the_analysis_cancels_its_computations() {
        let analysis = AudioAnalysis::new(PathBuf::from("/m/a.flac"), 1);
        let token = analysis.cancel_token();
        assert!(!token.is_cancelled());
        drop(analysis);
        assert!(token.is_cancelled());
    }
}
