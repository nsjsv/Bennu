use std::path::{Path, PathBuf};

use file_core::DirectoryEntry;
use iced::Task;

use super::{icon_grid_load_context, loading_icon_grid_directory, FileBrowser};
use crate::commands::load_expanded_directory_command;
use crate::model::{
    IconGridExpansionAnchor, IconGridExpansionContext, IconGridExpansionFollowAdvance,
    IconGridExpansionState, Message,
};

impl FileBrowser {
    pub(in crate::app) fn start_icon_grid_expansion_follow(
        &mut self,
        mut chain: Vec<PathBuf>,
        target_selection: PathBuf,
    ) -> Task<Message> {
        let Some(root_path) = chain.first().cloned() else {
            return Task::none();
        };
        let deepest_directory = chain
            .last()
            .cloned()
            .expect("non-empty icon expansion follow chain");
        let Some((root_index, _)) = self
            .entries
            .iter()
            .enumerate()
            .find(|(_, entry)| root_entry_matches(entry, &root_path, &self.current_dir))
        else {
            return Task::none();
        };
        let anchor = IconGridExpansionAnchor {
            parent_directory: self.current_dir.clone(),
            path: root_path.clone(),
            index: root_index,
        };
        let context = IconGridExpansionContext {
            pane_id: self.active_pane_id(),
            current_dir: self.current_dir.clone(),
            session_id: self.next_icon_grid_expansion_session_id(),
        };
        let mut expanded = loading_icon_grid_directory();
        let (request, cancellation) = Self::next_expanded_directory_load_request(
            icon_grid_load_context(&context),
            root_path,
            &mut expanded,
        );
        chain.remove(0);
        self.icon_grid_expansion = Some(IconGridExpansionState::following_directory_chain(
            context,
            anchor,
            expanded,
            chain,
            deepest_directory,
            target_selection,
        ));
        load_expanded_directory_command(request, self.options.clone(), cancellation)
    }

    pub(super) fn advance_icon_grid_expansion_follow(&mut self) -> Task<Message> {
        let advance = self
            .icon_grid_expansion
            .as_mut()
            .map(IconGridExpansionState::advance_follow_plan)
            .unwrap_or(IconGridExpansionFollowAdvance::Waiting);
        match advance {
            IconGridExpansionFollowAdvance::Waiting | IconGridExpansionFollowAdvance::Invalid => {
                Task::none()
            }
            IconGridExpansionFollowAdvance::StartChild(anchor) => {
                self.start_icon_grid_child(anchor)
            }
            IconGridExpansionFollowAdvance::RestoreSelection(path) => {
                if let Some(parent) = path.parent() {
                    if let Some(state) = self.icon_grid_expansion.as_mut() {
                        state.set_selection_directory(parent);
                    }
                }
                self.select_path(path.clone());
                match self.view_switch_reveal_scroll(&path) {
                    Some(task) => task,
                    None => {
                        self.pending_view_switch_reveal = Some((self.active_pane_id(), path));
                        Task::none()
                    }
                }
            }
        }
    }
}

fn root_entry_matches(entry: &DirectoryEntry, path: &Path, current_dir: &Path) -> bool {
    entry.path == *path && entry.expands_as_directory() && entry.path.parent() == Some(current_dir)
}
