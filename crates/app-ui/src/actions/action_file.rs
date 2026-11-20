//! 一个动作文件 → `ActionDefinition`。所有校验都在这里做一次，错误带行号；写错的文件整份不用，
//! 不会只执行一半。

use std::path::{Path, PathBuf};

use file_core::mime::MimeDatabase;
use file_core::{ArchiveExtractionPlacement, ArchiveFormat};
use serde::Deserialize;
use toml::Spanned;

use super::convert_format::ConvertFormat;
use super::definition::{ActionDefinition, ActionId, ActionStep, ActionWatch, TrashScope};
use super::notice::{ActionNotice, NoticeTemplate};
use super::template::{CommandTemplate, PathTemplate};
use super::type_filter::{TypeFilter, TypePattern};

/// 文件哪一行错了、错在哪（已经是给用户看的话）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileMistake {
    pub(crate) line: Option<usize>,
    pub(crate) reason: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAction {
    name: Option<Spanned<String>>,
    types: Option<Spanned<Vec<Spanned<String>>>>,
    steps: Option<Vec<Spanned<RawStep>>>,
    /// `"none"` 或者 `[notify]` 表。两种写法类型不同，按值自己分，不交给 serde 猜。
    notify: Option<Spanned<toml::Value>>,
    watch: Option<Spanned<RawWatch>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWatch {
    query: Option<Spanned<String>>,
    folder: Option<Spanned<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStep {
    convert: Option<Spanned<String>>,
    compress: Option<Spanned<String>>,
    extract: Option<Spanned<String>>,
    copy_to: Option<Spanned<String>>,
    move_to: Option<Spanned<String>>,
    trash: Option<Spanned<String>>,
    run: Option<Spanned<String>>,
    copy_paths: Option<Spanned<bool>>,
}

const STEP_KEYS: &str = "convert compress extract copy_to move_to trash run copy_paths";

/// 文件里 `compress`、`extract`、`trash` 的写法。读（这里）和编辑器写共用，`compress` 写的是扩展名。
pub(crate) const COMPRESS_FORMATS: [ArchiveFormat; 3] = [
    ArchiveFormat::Zip,
    ArchiveFormat::SevenZip,
    ArchiveFormat::TarGz,
];
pub(crate) const EXTRACT_PLACEMENTS: [(&str, ArchiveExtractionPlacement); 2] = [
    ("here", ArchiveExtractionPlacement::Smart),
    ("folder", ArchiveExtractionPlacement::ArchiveFolder),
];
pub(crate) const TRASH_SCOPES: [(&str, TrashScope); 2] = [
    ("current", TrashScope::Current),
    ("selected", TrashScope::Selected),
];

/// 按写法查值。
pub(crate) fn spelled<T: Copy>(table: &[(&str, T)], text: &str) -> Option<T> {
    table
        .iter()
        .find(|(spelling, _)| *spelling == text)
        .map(|(_, value)| *value)
}

/// 值的写法。表里每个值都有。
pub(crate) fn spelling<T: Copy + PartialEq>(table: &[(&'static str, T)], value: T) -> &'static str {
    table
        .iter()
        .find(|(_, entry)| *entry == value)
        .map(|(spelling, _)| *spelling)
        .expect("every value has a spelling")
}

struct Source<'a> {
    text: &'a str,
}

impl Source<'_> {
    fn line_of<T>(&self, spanned: &Spanned<T>) -> Option<usize> {
        Some(self.line_at(spanned.span().start))
    }

    fn line_at(&self, offset: usize) -> usize {
        let end = offset.min(self.text.len());
        self.text.as_bytes()[..end]
            .iter()
            .filter(|byte| **byte == b'\n')
            .count()
            + 1
    }

    fn mistake<T>(&self, at: &Spanned<T>, reason: String) -> FileMistake {
        FileMistake {
            line: self.line_of(at),
            reason,
        }
    }
}

pub(super) fn parse_action_file(
    id: ActionId,
    file: &Path,
    text: &str,
    types: &MimeDatabase,
) -> Result<ActionDefinition, FileMistake> {
    let source = Source { text };
    let raw: RawAction = toml::from_str(text).map_err(|error| FileMistake {
        line: error.span().map(|span| source.line_at(span.start)),
        reason: error.message().trim().to_owned(),
    })?;
    let Some(name) = raw.name else {
        return Err(FileMistake {
            line: None,
            reason: tr!("The action needs a name (name = \"…\")"),
        });
    };
    let trimmed_name = name.get_ref().trim();
    if trimmed_name.is_empty() {
        return Err(source.mistake(&name, tr!("The name cannot be empty")));
    }
    let steps = parse_steps(&source, raw.steps)?;
    Ok(ActionDefinition {
        id,
        name: trimmed_name.to_owned(),
        file: file.to_path_buf(),
        types: parse_types(&source, raw.types, types)?,
        watch: parse_watch(&source, raw.watch, &steps)?,
        steps,
        notice: parse_notice(&source, raw.notify)?,
    })
}

/// `[watch]`：查询（`f:` 的写法，可以只有筛选；不写是什么都算）和监视的文件夹。自己跑的动作里不能
/// 有移到回收站、执行命令：没人看着时不能删东西、不能跑任意程序。
fn parse_watch(
    source: &Source<'_>,
    raw: Option<Spanned<RawWatch>>,
    steps: &[ActionStep],
) -> Result<Option<ActionWatch>, FileMistake> {
    let Some(watch) = raw else {
        return Ok(None);
    };
    let Some(folder) = &watch.get_ref().folder else {
        return Err(source.mistake(
            &watch,
            tr!("watch needs the folder to watch (folder = \"~/…\")"),
        ));
    };
    let folder_text = folder.get_ref().trim();
    let folder_path = match folder_text.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            file_core::app_dirs::home_dir().join(rest.trim_start_matches('/'))
        }
        _ if folder_text.starts_with('/') => PathBuf::from(folder_text),
        _ => {
            return Err(source.mistake(folder, tr!("The watched folder has to start with / or ~/")))
        }
    };
    let query = watch
        .get_ref()
        .query
        .as_ref()
        .map(|query| query.get_ref().trim().to_owned())
        .unwrap_or_default();
    if let (Some(at), Err(reason)) = (
        &watch.get_ref().query,
        crate::super_box::read_find_query(&query),
    ) {
        return Err(source.mistake(at, reason));
    }
    if steps
        .iter()
        .any(|step| matches!(step, ActionStep::Trash(_) | ActionStep::Run(_)))
    {
        return Err(source.mistake(
            &watch,
            tr!("An action that runs by itself cannot move to the trash or run commands"),
        ));
    }
    Ok(Some(ActionWatch {
        query,
        folder: folder_path,
    }))
}

fn parse_types(
    source: &Source<'_>,
    raw: Option<Spanned<Vec<Spanned<String>>>>,
    types: &MimeDatabase,
) -> Result<TypeFilter, FileMistake> {
    let Some(list) = raw else {
        return Ok(TypeFilter::Anything);
    };
    if list.get_ref().is_empty() {
        return Err(source.mistake(
            &list,
            tr!("types cannot be empty; leave it out to show the action for everything"),
        ));
    }
    list.get_ref()
        .iter()
        .map(|pattern| {
            parse_type_pattern(pattern.get_ref(), types)
                .map_err(|reason| source.mistake(pattern, reason))
        })
        .collect::<Result<_, _>>()
        .map(TypeFilter::Patterns)
}

/// `types` 里的一项。编辑器的“其他类型”也用它，说法和读文件时一样。
pub(crate) fn parse_type_pattern(text: &str, types: &MimeDatabase) -> Result<TypePattern, String> {
    TypePattern::parse(text, types).ok_or_else(|| {
        tr!(
            "“%1” is not a MIME type (for example video/*, image/png or inode/directory)",
            text
        )
    })
}

fn parse_steps(
    source: &Source<'_>,
    raw: Option<Vec<Spanned<RawStep>>>,
) -> Result<Vec<ActionStep>, FileMistake> {
    let raw = raw.unwrap_or_default();
    if raw.is_empty() {
        return Err(FileMistake {
            line: None,
            reason: tr!("The action needs at least one step ([[steps]])"),
        });
    }
    let last = raw.len() - 1;
    raw.iter()
        .enumerate()
        .map(|(index, step)| {
            let parsed = parse_step(source, step)?;
            if index != last && matches!(parsed, ActionStep::Trash(_)) {
                return Err(source.mistake(step, tr!("trash has to be the last step")));
            }
            Ok(parsed)
        })
        .collect()
}

fn parse_step(source: &Source<'_>, step: &Spanned<RawStep>) -> Result<ActionStep, FileMistake> {
    let raw = step.get_ref();
    let valued = [
        (StepKey::Convert, &raw.convert),
        (StepKey::Compress, &raw.compress),
        (StepKey::Extract, &raw.extract),
        (StepKey::CopyTo, &raw.copy_to),
        (StepKey::MoveTo, &raw.move_to),
        (StepKey::Trash, &raw.trash),
        (StepKey::Run, &raw.run),
    ];
    let present = valued.iter().filter(|(_, value)| value.is_some()).count()
        + usize::from(raw.copy_paths.is_some());
    if present != 1 {
        return Err(source.mistake(step, tr!("Each step needs exactly one of: %1", STEP_KEYS)));
    }
    if let Some((key, Some(value))) = valued.iter().find(|(_, value)| value.is_some()) {
        return read_step(*key, value.get_ref()).map_err(|reason| source.mistake(value, reason));
    }
    match &raw.copy_paths {
        Some(copy_paths) if *copy_paths.get_ref() => Ok(ActionStep::CopyPaths),
        _ => Err(source.mistake(step, tr!("copy_paths only takes true"))),
    }
}

/// 一步的键（`copy_paths` 没有值，不在这里）。动作文件和超级输入框的管道都按它读。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StepKey {
    Convert,
    Compress,
    Extract,
    CopyTo,
    MoveTo,
    Trash,
    Run,
}

/// 写在键后面的值读成一步；读不懂时是给用户看的原因。
pub(crate) fn read_step(key: StepKey, value: &str) -> Result<ActionStep, String> {
    match key {
        StepKey::Convert => ConvertFormat::named(value)
            .map(ActionStep::Convert)
            .ok_or_else(|| {
                tr!(
                    "Unknown format “%1”. You can use: %2",
                    value,
                    ConvertFormat::names()
                )
            }),
        StepKey::Compress => COMPRESS_FORMATS
            .into_iter()
            .find(|known| known.extension() == value)
            .map(ActionStep::Compress)
            .ok_or_else(|| {
                tr!(
                    "Unknown archive format “%1”. You can use: zip 7z tar.gz",
                    value
                )
            }),
        StepKey::Extract => spelled(&EXTRACT_PLACEMENTS, value)
            .map(ActionStep::Extract)
            .ok_or_else(|| tr!("extract is “here” or “folder”")),
        StepKey::CopyTo => PathTemplate::parse(value)
            .map(ActionStep::CopyTo)
            .map_err(|error| error.0),
        StepKey::MoveTo => PathTemplate::parse(value)
            .map(ActionStep::MoveTo)
            .map_err(|error| error.0),
        StepKey::Trash => spelled(&TRASH_SCOPES, value)
            .map(ActionStep::Trash)
            .ok_or_else(|| tr!("trash is “current” or “selected”")),
        StepKey::Run => CommandTemplate::parse(value)
            .map(ActionStep::Run)
            .map_err(|error| error.0),
    }
}

fn parse_notice(
    source: &Source<'_>,
    raw: Option<Spanned<toml::Value>>,
) -> Result<ActionNotice, FileMistake> {
    let Some(raw) = raw else {
        return Ok(ActionNotice::Standard);
    };
    let wrong = || {
        source.mistake(
            &raw,
            tr!("notify is “none”, or a [notify] table with a title and an optional body"),
        )
    };
    let table = match raw.get_ref() {
        toml::Value::String(choice) if choice == "none" => return Ok(ActionNotice::Silent),
        toml::Value::Table(table) => table,
        _ => return Err(wrong()),
    };
    if table.keys().any(|key| key != "title" && key != "body") {
        return Err(wrong());
    }
    let template = |key: &str| -> Result<Option<NoticeTemplate>, FileMistake> {
        match table.get(key) {
            None => Ok(None),
            Some(toml::Value::String(text)) if !text.trim().is_empty() => {
                NoticeTemplate::parse(text)
                    .map(Some)
                    .map_err(|error| source.mistake(&raw, error.0))
            }
            Some(_) => Err(wrong()),
        }
    };
    let Some(title) = template("title")? else {
        return Err(wrong());
    };
    Ok(ActionNotice::Custom {
        title,
        body: template("body")?,
    })
}

#[cfg(test)]
#[path = "action_file_tests.rs"]
mod tests;
