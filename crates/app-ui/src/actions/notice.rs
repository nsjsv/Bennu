//! 动作结束时的通知：默认 / 自己写 / 不通知（每个动作三选一）。

use std::path::Path;

use super::template::{parse_segments, Segment, TemplateError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ActionNotice {
    /// 不写 `notify`：软件的统一说法，和其他通知一样只在窗口不在前台时弹。
    Standard,
    /// `[notify]`：自己写的标题和正文，窗口在前台也弹。
    Custom {
        title: NoticeTemplate,
        body: Option<NoticeTemplate>,
    },
    /// `notify = "none"`：什么都不弹，只在任务面板里看。
    Silent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoticeVariable {
    Action,
    Count,
    Name,
    Dir,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoticeTemplate {
    segments: Vec<Segment<NoticeVariable>>,
}

/// 填进通知的值。
#[derive(Debug, Clone, Copy)]
pub(crate) struct NoticeValues<'a> {
    pub(crate) action: &'a str,
    /// 最后交出来的结果，第一个用来填 `{name}` 和 `{dir}`。
    pub(crate) results: &'a [std::path::PathBuf],
    pub(crate) failed: usize,
}

impl NoticeTemplate {
    pub(crate) fn parse(text: &str) -> Result<Self, TemplateError> {
        let segments = parse_segments(
            text,
            |name| {
                Some(match name {
                    "action" => NoticeVariable::Action,
                    "count" => NoticeVariable::Count,
                    "name" => NoticeVariable::Name,
                    "dir" => NoticeVariable::Dir,
                    "failed" => NoticeVariable::Failed,
                    _ => return None,
                })
            },
            "{action} {count} {name} {dir} {failed}",
        )?;
        Ok(Self { segments })
    }

    pub(crate) fn render(&self, values: NoticeValues<'_>) -> String {
        let first = values.results.first().map(|path| path.as_path());
        let display = |path: Option<&Path>| {
            path.map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let mut text = String::new();
        for segment in &self.segments {
            match segment {
                Segment::Text(literal) => text.push_str(literal),
                Segment::Variable(NoticeVariable::Action) => text.push_str(values.action),
                Segment::Variable(NoticeVariable::Count) => {
                    text.push_str(&values.results.len().to_string())
                }
                Segment::Variable(NoticeVariable::Name) => text.push_str(&display(
                    first.and_then(|path| path.file_name()).map(Path::new),
                )),
                Segment::Variable(NoticeVariable::Dir) => {
                    text.push_str(&display(first.and_then(Path::parent)))
                }
                Segment::Variable(NoticeVariable::Failed) => {
                    text.push_str(&values.failed.to_string())
                }
            }
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn variables_take_the_first_result_and_counts() {
        let template =
            NoticeTemplate::parse("{action}: {count} → {dir} ({name}), {failed} failed").unwrap();
        let results = [
            PathBuf::from("/music/音频/a.m4a"),
            PathBuf::from("/music/音频/b.m4a"),
        ];
        assert_eq!(
            template.render(NoticeValues {
                action: "提取音频",
                results: &results,
                failed: 1,
            }),
            "提取音频: 2 → /music/音频 (a.m4a), 1 failed"
        );
        assert_eq!(
            template.render(NoticeValues {
                action: "x",
                results: &[],
                failed: 0,
            }),
            "x: 0 →  (), 0 failed"
        );
    }

    #[test]
    fn unknown_variables_are_rejected() {
        assert!(NoticeTemplate::parse("{files}").is_err());
        assert!(NoticeTemplate::parse("{{count}}").is_ok());
    }
}
