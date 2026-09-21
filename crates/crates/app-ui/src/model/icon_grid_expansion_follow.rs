use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use file_core::FileKind;

use super::{
    IconGridExpandedDirectory, IconGridExpansionAnchor, IconGridExpansionContext,
    IconGridExpansionState,
};
use crate::model::ExpandedDirectory;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IconGridExpansionFollowAdvance {
    Waiting,
    StartChild(IconGridExpansionAnchor),
    RestoreSelection(PathBuf),
    Invalid,
}

#[derive(Debug, Clone)]
pub(super) struct IconGridExpansionFollowPlan {
    remaining_directories: VecDeque<PathBuf>,
    deepest_directory: PathBuf,
    target_selection: PathBuf,
}

impl IconGridExpansionState {
    pub(crate) fn following_directory_chain(
        context: IconGridExpansionContext,
        root_anchor: IconGridExpansionAnchor,
        root_contents: ExpandedDirectory,
        remaining_directories: Vec<PathBuf>,
        deepest_directory: PathBuf,
        target_selection: PathBuf,
    ) -> Self {
        let mut state = Self::new(context, root_anchor, root_contents);
        state.follow_plan = Some(IconGridExpansionFollowPlan {
            remaining_directories: remaining_directories.into(),
            deepest_directory,
            target_selection,
        });
        state
    }

    pub(crate) fn cancel_follow_plan(&mut self) {
        self.follow_plan = None;
    }

    #[cfg(test)]
    pub(crate) fn has_follow_plan(&self) -> bool {
        self.follow_plan.is_some()
    }

    pub(crate) fn advance_follow_plan(&mut self) -> IconGridExpansionFollowAdvance {
        let Some(plan) = self.follow_plan.as_ref() else {
            return IconGridExpansionFollowAdvance::Waiting;
        };

        if let Some(next_path) = plan.remaining_directories.front() {
            let Some(parent_directory) = next_path.parent() else {
                self.follow_plan = None;
                return IconGridExpansionFollowAdvance::Invalid;
            };
            let Some(entries) = self.entries_in_interactive_directory(parent_directory) else {
                return IconGridExpansionFollowAdvance::Waiting;
            };
            let Some((index, _)) = entries
                .iter()
                .enumerate()
                .find(|(_, entry)| entry.path == *next_path && entry.kind == FileKind::Directory)
            else {
                self.follow_plan = None;
                return IconGridExpansionFollowAdvance::Invalid;
            };
            let path = next_path.clone();
            let parent_directory = parent_directory.to_path_buf();
            self.follow_plan
                .as_mut()
                .expect("follow plan remains active")
                .remaining_directories
                .pop_front();
            return IconGridExpansionFollowAdvance::StartChild(IconGridExpansionAnchor {
                parent_directory,
                path,
                index,
            });
        }

        if !self.node_is_interactive(&plan.deepest_directory) {
            return IconGridExpansionFollowAdvance::Waiting;
        }
        let target_selection = plan.target_selection.clone();
        let target_is_interactive = target_selection == self.root_path
            || self.directories.iter().any(|(directory_path, directory)| {
                self.node_is_interactive(directory_path)
                    && directory
                        .contents
                        .entries
                        .iter()
                        .any(|entry| entry.path == target_selection)
            });
        self.follow_plan = None;
        if target_is_interactive {
            IconGridExpansionFollowAdvance::RestoreSelection(target_selection)
        } else {
            IconGridExpansionFollowAdvance::Invalid
        }
    }

    pub(crate) fn interactive_expansion_chain_for_selection(
        &self,
        selection: &Path,
    ) -> Option<Vec<PathBuf>> {
        let deepest_directory = if self
            .directories
            .get(selection)
            .is_some_and(IconGridExpandedDirectory::is_interactive)
            && self.node_is_interactive(selection)
        {
            selection.to_path_buf()
        } else {
            self.directories
                .iter()
                .find(|(directory_path, directory)| {
                    self.node_is_interactive(directory_path)
                        && directory
                            .contents
                            .entries
                            .iter()
                            .any(|entry| entry.path == selection)
                })
                .map(|(directory_path, _)| directory_path.clone())?
        };

        let mut chain = Vec::new();
        let mut current = deepest_directory;
        loop {
            let directory = self.directories.get(&current)?;
            if !self.node_is_interactive(&current) {
                return None;
            }
            chain.push(current.clone());
            if current == self.root_path {
                break;
            }
            current = directory.parent_directory.clone();
        }
        chain.reverse();
        Some(chain)
    }
}
