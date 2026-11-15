//! 「播下一个」：引擎说这个放完了，就在当前列表的顺序里找下一个同类文件（音频接音频、视频接
//! 视频），把选中移过去。独立窗口重新打开预览；右侧面板由更新出口的收敛点
//! （sync_right_preview_panel）自然跟上。引擎记着排进来的这个，打开时右侧面板也直接播、
//! 全屏的接着全屏。
//!
//! 图片的 ←/→ 用同一个列表顺序换上一张/下一张图片（静态、动图都算），到头停下；独立窗口
//! 不关窗，在原窗口里换内容。

use std::path::{Path, PathBuf};

use bennu_preview::engine::PreviewLoadSurface;
use bennu_preview::preview_loading::{classify_preview_path, PreviewPathKind};
use bennu_preview::preview_message::ImageStep;
use file_core::FileKind;
use iced::Task;

use crate::app::FileBrowser;
use crate::model::{BrowserViewMode, Message};

impl FileBrowser {
    pub(in crate::app) fn play_next_media(
        &mut self,
        finished: PathBuf,
        kind: PreviewPathKind,
    ) -> Task<Message> {
        // 这个放完前用户已经选了别的：不替他换。
        if self.tab().selected.as_deref() != Some(finished.as_path()) {
            return Task::none();
        }
        let rules = &self.user_config.preview_extension_rules;
        let next = next_media_after(&self.media_queue_order(&finished), &finished, |path| {
            self.entry_kind(path) == Some(FileKind::File)
                && classify_preview_path(path, rules) == Some(kind)
        });
        // 最后一个放完就停（用户定的：不回到第一个）。
        let Some(next) = next else {
            return Task::none();
        };
        self.preview_engine.queue_next_media(next.clone());
        let select = self.select_path_from_keyboard(next);
        let reopen = match self.preview_engine.preview_load_surface {
            PreviewLoadSurface::StandaloneWindow => self.open_preview(),
            PreviewLoadSurface::RightDockedPanel => Task::none(),
        };
        Task::batch([select, reopen])
    }

    pub(in crate::app) fn step_preview_image(&mut self, step: ImageStep) -> Task<Message> {
        let Some(shown) = self.preview_engine.preview_shown_path.clone() else {
            return Task::none();
        };
        let rules = &self.user_config.preview_extension_rules;
        let is_image = |path: &Path| {
            self.entry_kind(path) == Some(FileKind::File)
                && matches!(
                    classify_preview_path(path, rules),
                    Some(PreviewPathKind::Image | PreviewPathKind::AnimatedImage)
                )
        };
        let mut order = self.media_queue_order(&shown);
        if step == ImageStep::Previous {
            order.reverse();
        }
        let Some(neighbour) = next_media_after(&order, &shown, is_image) else {
            return Task::none();
        };
        let select = self.select_path_from_keyboard(neighbour.clone());
        let show = match self.preview_engine.preview_load_surface {
            PreviewLoadSurface::StandaloneWindow => {
                let pinned = self.preview_engine.preview_window_pinned;
                self.present_preview_target(neighbour, pinned)
            }
            PreviewLoadSurface::RightDockedPanel => Task::none(),
        };
        Task::batch([select, show])
    }

    /// 「当前列表的顺序」：搜索结果按结果顺序；多栏按这首所在的那一栏；列表、图标按当前可见顺序。
    fn media_queue_order(&self, finished: &Path) -> Vec<PathBuf> {
        if self.active_search().is_none() && self.tab().view_mode() == BrowserViewMode::Columns {
            return finished
                .parent()
                .map(|directory| self.entry_paths_in_directory(directory))
                .unwrap_or_default();
        }
        self.visible_entry_paths()
    }
}

fn next_media_after(
    paths: &[PathBuf],
    finished: &Path,
    is_audio: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let index = paths.iter().position(|path| path == finished)?;
    paths[index + 1..]
        .iter()
        .find(|path| is_audio(path))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_audio_skips_other_files_and_stops_after_the_last() {
        let paths: Vec<PathBuf> = ["a.mp3", "cover.jpg", "notes.txt", "b.flac", "c.opus"]
            .into_iter()
            .map(PathBuf::from)
            .collect();
        let is_audio = |path: &Path| {
            path.extension().is_some_and(|extension| {
                ["mp3", "flac", "opus"].contains(&extension.to_str().unwrap())
            })
        };
        assert_eq!(
            next_media_after(&paths, Path::new("a.mp3"), is_audio),
            Some(PathBuf::from("b.flac"))
        );
        assert_eq!(
            next_media_after(&paths, Path::new("c.opus"), is_audio),
            None
        );
        // 这个不在列表里（被删了、列表换了）：不乱跳。
        assert_eq!(
            next_media_after(&paths, Path::new("gone.mp3"), is_audio),
            None
        );
    }

    #[test]
    fn neighbouring_images_skip_other_files_both_ways() {
        let rules = bennu_preview::preview_config::PreviewExtensionRules::default_rules();
        let mut paths: Vec<PathBuf> = ["a.jpg", "song.mp3", "b.gif", "notes.txt", "c.heic"]
            .into_iter()
            .map(PathBuf::from)
            .collect();
        let is_image = |path: &Path| {
            matches!(
                classify_preview_path(path, &rules),
                Some(PreviewPathKind::Image | PreviewPathKind::AnimatedImage)
            )
        };
        assert_eq!(
            next_media_after(&paths, Path::new("a.jpg"), is_image),
            Some(PathBuf::from("b.gif"))
        );
        assert_eq!(
            next_media_after(&paths, Path::new("c.heic"), is_image),
            None
        );
        // 往前找就是倒过来的列表往后找；第一张再往前停下。
        paths.reverse();
        assert_eq!(
            next_media_after(&paths, Path::new("c.heic"), is_image),
            Some(PathBuf::from("b.gif"))
        );
        assert_eq!(next_media_after(&paths, Path::new("a.jpg"), is_image), None);
    }

    #[test]
    fn next_video_skips_audio_and_other_files() {
        let rules = bennu_preview::preview_config::PreviewExtensionRules::default_rules();
        let paths: Vec<PathBuf> = ["a.mkv", "b.mp3", "notes.txt", "c.mp4", "d.flac"]
            .into_iter()
            .map(PathBuf::from)
            .collect();
        let rules = &rules;
        let same_kind = |kind| move |path: &Path| classify_preview_path(path, rules) == Some(kind);
        assert_eq!(
            next_media_after(
                &paths,
                Path::new("a.mkv"),
                same_kind(PreviewPathKind::Video)
            ),
            Some(PathBuf::from("c.mp4"))
        );
        assert_eq!(
            next_media_after(
                &paths,
                Path::new("c.mp4"),
                same_kind(PreviewPathKind::Video)
            ),
            None
        );
        assert_eq!(
            next_media_after(
                &paths,
                Path::new("b.mp3"),
                same_kind(PreviewPathKind::Audio)
            ),
            Some(PathBuf::from("d.flac"))
        );
    }
}
