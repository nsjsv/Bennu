//! 每种模式下框里列什么。

use super::{calculate, complete, find, matching, mode, rename, targets};
use super::{BoxContext, BoxMode, BoxRow, FindStatus, SuperBox, FOUND_IN_SEARCH, MOST_ROWS};
use super::{PickedItems, Pipe, PipeSource, MOST_PICKED};

/// 框是空的、没有胶囊（默认模式）时列几条历史（各种模式混在一起）。
const HISTORY_MIXED: usize = 5;
/// 有胶囊时列几条这个模式的历史。
const HISTORY_IN_MODE: usize = 3;

impl SuperBox {
    pub(super) fn compute_rows(&self, context: &BoxContext<'_>) -> Vec<BoxRow> {
        let text = self.input.trim();
        if text.is_empty() {
            let history = self.history.iter();
            let history: Vec<_> = if self.shows_tag(self.mode) {
                history
                    .filter(|entry| mode::read_history(entry).0 == self.mode)
                    .take(HISTORY_IN_MODE)
                    .collect()
            } else {
                history.take(HISTORY_MIXED).collect()
            };
            return history
                .into_iter()
                .map(|entry| BoxRow::History(entry.clone()))
                .collect();
        }
        if let Some(pipe) = &self.pipe {
            return self.pipe_rows(pipe);
        }
        match self.mode {
            BoxMode::Search => matching::ranked(&self.entries, text)
                .into_iter()
                .map(BoxRow::Entry)
                .chain(self.found_rows(FOUND_IN_SEARCH))
                .collect(),
            BoxMode::Files => {
                let query = match find::read_find_query(text) {
                    Ok(query) => query,
                    Err(reason) => return vec![BoxRow::Problem(reason)],
                };
                if query.terms.is_empty() {
                    return vec![BoxRow::Problem(tr!(
                        "Type what to look for; filters only narrow it down"
                    ))];
                }
                if let FindStatus::Unavailable(reason) = &self.find_status {
                    return vec![BoxRow::Problem(reason.clone())];
                }
                self.found_rows(MOST_ROWS).collect()
            }
            BoxMode::Path => {
                let Some(target) = targets::jump_path(text, context.current_dir, context.home)
                else {
                    return vec![BoxRow::Problem(tr!(
                        "Relative paths need a folder; type a path starting with / or ~"
                    ))];
                };
                let name = complete::word(BoxMode::Path, &self.input)
                    .map(|word| word.name)
                    .unwrap_or_default();
                let suggestions = self
                    .listing
                    .folder
                    .iter()
                    .flat_map(|folder| {
                        complete::matching(&name, &self.listing.candidates)
                            .into_iter()
                            .filter(|candidate| candidate.is_dir)
                            .map(|candidate| folder.join(&candidate.name))
                    })
                    .filter(|suggestion| *suggestion != target)
                    .take(MOST_ROWS - 1)
                    .map(BoxRow::JumpSuggestion)
                    .collect::<Vec<_>>();
                std::iter::once(BoxRow::JumpTo(target))
                    .chain(suggestions)
                    .collect()
            }
            BoxMode::New => {
                if let Some(reason) = &context.cannot_create {
                    return vec![BoxRow::Problem(reason.clone())];
                }
                match targets::read_new_entry(text, context.taken) {
                    Ok(entry) => vec![BoxRow::Create(entry)],
                    Err(reason) => vec![BoxRow::Problem(reason)],
                }
            }
            BoxMode::Rename => {
                match rename::plan_renames(
                    text,
                    context.selected,
                    context.current_dir,
                    context.taken,
                ) {
                    Ok(renamed) => vec![BoxRow::Rename(renamed)],
                    Err(reason) => vec![BoxRow::Problem(reason)],
                }
            }
            BoxMode::Shell => match context.current_dir {
                Some(_) => vec![BoxRow::Run(text.to_owned())],
                None => vec![BoxRow::Problem(tr!(
                    "Commands run in a folder; open one first"
                ))],
            },
            BoxMode::Calculate => match calculate::calculate(text) {
                Some(answer) => vec![BoxRow::Answer(answer)],
                None => vec![BoxRow::Problem(tr!("This cannot be worked out"))],
            },
            BoxMode::Send => send_rows(text, context),
        }
    }

    /// 管道：写对了、挑好了、数量合适才有一行可执行的预览；否则说明为什么。
    fn pipe_rows(&self, pipe: &Result<Pipe, String>) -> Vec<BoxRow> {
        let picked = self
            .picked
            .as_ref()
            .map(|picked| (&picked.source, &picked.items));
        let reason = match (pipe, picked) {
            (Err(reason), _) => reason.clone(),
            (Ok(_), Some((PipeSource::Selection, PickedItems::Found(items))))
                if items.is_empty() =>
            {
                tr!("Select the items first, or write which ones before |")
            }
            (Ok(_), Some((_, PickedItems::Found(items)))) if items.is_empty() => {
                tr!("No item matches")
            }
            (Ok(_), Some((_, PickedItems::Found(items)))) if items.len() > MOST_PICKED => {
                tr!("More than %1 items; narrow it down", MOST_PICKED)
            }
            (Ok(_), Some((_, PickedItems::Found(_)))) => return vec![BoxRow::Pipe],
            (Ok(_), Some((_, PickedItems::Unavailable(reason)))) => reason.clone(),
            (Ok(_), Some((_, PickedItems::Waiting)) | None) => return Vec::new(),
        };
        vec![BoxRow::Problem(reason)]
    }

    fn found_rows(&self, most: usize) -> impl Iterator<Item = BoxRow> + '_ {
        (0..self.found.len().min(most)).map(BoxRow::Found)
    }
}

/// 发送模式：名字里有打的字的设备；没选中东西、没有能发的设备时说明。
fn send_rows(name: &str, context: &BoxContext<'_>) -> Vec<BoxRow> {
    if context.selected.is_empty() {
        return vec![BoxRow::Problem(tr!("Select what to send first"))];
    }
    let name = mode::fold(name.trim());
    let devices: Vec<_> = context
        .devices
        .iter()
        .filter(|device| mode::fold(&device.name).contains(&name))
        .take(MOST_ROWS)
        .cloned()
        .map(BoxRow::SendTo)
        .collect();
    if devices.is_empty() {
        return vec![BoxRow::Problem(tr!(
            "No device to send to: turn on transfers, or wait for a device nearby"
        ))];
    }
    devices
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::super_box::SendTarget;

    #[test]
    fn devices_are_found_by_name_and_need_something_selected() {
        let taken = |_: &str| false;
        let devices = [
            SendTarget {
                name: "我的手机".to_owned(),
                fingerprint: "a".to_owned(),
            },
            SendTarget {
                name: "Desktop".to_owned(),
                fingerprint: "b".to_owned(),
            },
        ];
        let selected = [PathBuf::from("/w/a.txt")];
        let with = BoxContext {
            current_dir: Some(Path::new("/w")),
            home: Path::new("/home/me"),
            cannot_create: None,
            taken: &taken,
            selected: &selected,
            devices: &devices,
        };
        assert_eq!(
            send_rows("手机", &with),
            [BoxRow::SendTo(devices[0].clone())]
        );
        assert_eq!(
            send_rows("desk", &with),
            [BoxRow::SendTo(devices[1].clone())]
        );
        assert_eq!(send_rows("", &with).len(), 2);
        assert!(matches!(send_rows("平板", &with)[..], [BoxRow::Problem(_)]));
        let nothing_selected = BoxContext {
            selected: &[],
            ..with
        };
        assert!(matches!(
            send_rows("手机", &nothing_selected)[..],
            [BoxRow::Problem(_)]
        ));
    }
}
