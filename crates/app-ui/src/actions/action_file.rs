//! 一个动作文件 → `ActionDefinition`。所有校验都在这里做一次，错误带行号；写错的文件整份不用，
//! 不会只执行一半。

use std::path::Path;

use file_core::mime::MimeDatabase;
use file_core::{ArchiveExtractionPlacement, ArchiveFormat};
use serde::Deserialize;
use toml::Spanned;

use super::convert_format::ConvertFormat;
use super::definition::{ActionDefinition, ActionId, ActionStep, TrashScope};
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
    Ok(ActionDefinition {
        id,
        name: trimmed_name.to_owned(),
        file: file.to_path_buf(),
        types: parse_types(&source, raw.types, types)?,
        steps: parse_steps(&source, raw.steps)?,
        notice: parse_notice(&source, raw.notify)?,
    })
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
    let present = [
        raw.convert.is_some(),
        raw.compress.is_some(),
        raw.extract.is_some(),
        raw.copy_to.is_some(),
        raw.move_to.is_some(),
        raw.trash.is_some(),
        raw.run.is_some(),
        raw.copy_paths.is_some(),
    ];
    if present.iter().filter(|present| **present).count() != 1 {
        return Err(source.mistake(step, tr!("Each step needs exactly one of: %1", STEP_KEYS)));
    }
    let invalid = |at: &Spanned<String>, reason: String| source.mistake(at, reason);
    if let Some(format) = &raw.convert {
        return ConvertFormat::named(format.get_ref())
            .map(ActionStep::Convert)
            .ok_or_else(|| {
                invalid(
                    format,
                    tr!(
                        "Unknown format “%1”. You can use: %2",
                        format.get_ref(),
                        ConvertFormat::names()
                    ),
                )
            });
    }
    if let Some(format) = &raw.compress {
        return COMPRESS_FORMATS
            .into_iter()
            .find(|known| known.extension() == format.get_ref())
            .map(ActionStep::Compress)
            .ok_or_else(|| {
                invalid(
                    format,
                    tr!(
                        "Unknown archive format “%1”. You can use: zip 7z tar.gz",
                        format.get_ref()
                    ),
                )
            });
    }
    if let Some(placement) = &raw.extract {
        return spelled(&EXTRACT_PLACEMENTS, placement.get_ref())
            .map(ActionStep::Extract)
            .ok_or_else(|| invalid(placement, tr!("extract is “here” or “folder”")));
    }
    if let Some(folder) = &raw.copy_to {
        return PathTemplate::parse(folder.get_ref())
            .map(ActionStep::CopyTo)
            .map_err(|error| invalid(folder, error.0));
    }
    if let Some(folder) = &raw.move_to {
        return PathTemplate::parse(folder.get_ref())
            .map(ActionStep::MoveTo)
            .map_err(|error| invalid(folder, error.0));
    }
    if let Some(scope) = &raw.trash {
        return spelled(&TRASH_SCOPES, scope.get_ref())
            .map(ActionStep::Trash)
            .ok_or_else(|| invalid(scope, tr!("trash is “current” or “selected”")));
    }
    if let Some(command) = &raw.run {
        return CommandTemplate::parse(command.get_ref())
            .map(ActionStep::Run)
            .map_err(|error| invalid(command, error.0));
    }
    match &raw.copy_paths {
        Some(copy_paths) if *copy_paths.get_ref() => Ok(ActionStep::CopyPaths),
        _ => Err(source.mistake(step, tr!("copy_paths only takes true"))),
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
