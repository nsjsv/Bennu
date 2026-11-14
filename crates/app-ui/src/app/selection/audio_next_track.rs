//! 「播下一首」：引擎说这首放完了，就在当前列表的顺序里找下一个音频、把选中移过去。
//! 独立窗口重新打开预览；右侧面板由更新出口的收敛点（sync_right_preview_panel）自然跟上。
//! 引擎记着排进来的这首，打开时右侧面板也直接播。

use std::path::{Path, PathBuf};

use bennu_preview::engine::PreviewLoadSurface;
use bennu_preview::preview_loading::{classify_preview_path, PreviewPathKind};
use file_core::FileKind;
use iced::Task;

use crate::app::FileBrowser;
use crate::model::{BrowserViewMode, Message};

impl FileBrowser {
    pub(in crate::app) fn play_next_audio_track(&mut self, finished: PathBuf) -> Task<Message> {
        // 这首放完前用户已经选了别的：不替他换。
        if self.tab().selected.as_deref() != Some(finished.as_path()) {
            return Task::none();
        }
        let rules = &self.user_config.preview_extension_rules;
        let next = next_audio_after(&self.audio_queue_order(&finished), &finished, |path| {
            self.entry_kind(path) == Some(FileKind::File)
                && classify_preview_path(path, rules) == Some(PreviewPathKind::Audio)
        });
        // 最后一首放完就停（用户定的：不回到第一首）。
        let Some(next) = next else {
            return Task::none();
        };
        self.preview_engine.queue_next_audio_track(next.clone());
        let select = self.select_path_from_keyboard(next);
        let reopen = match self.preview_engine.preview_load_surface {
            PreviewLoadSurface::StandaloneWindow => self.open_preview(),
            PreviewLoadSurface::RightDockedPanel => Task::none(),
        };
        Task::batch([select, reopen])
    }

    /// 「当前列表的顺序」：搜索结果按结果顺序；多栏按这首所在的那一栏；列表、图标按当前可见顺序。
    fn audio_queue_order(&self, finished: &Path) -> Vec<PathBuf> {
        if self.active_search().is_none() && self.tab().view_mode() == BrowserViewMode::Columns {
            return finished
                .parent()
                .map(|directory| self.entry_paths_in_directory(directory))
                .unwrap_or_default();
        }
        self.visible_entry_paths()
    }
}

fn next_audio_after(
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
            next_audio_after(&paths, Path::new("a.mp3"), is_audio),
            Some(PathBuf::from("b.flac"))
        );
        assert_eq!(
            next_audio_after(&paths, Path::new("c.opus"), is_audio),
            None
        );
        // 这首不在列表里（被删了、列表换了）：不乱跳。
        assert_eq!(
            next_audio_after(&paths, Path::new("gone.mp3"), is_audio),
            None
        );
    }
}
