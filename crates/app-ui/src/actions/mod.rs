//! 自定义动作：`~/.config/bennu/actions/` 里一个 `.toml` 文件一个动作。这里是纯模型：
//! 读文件、校验、按类型匹配、展开模板，不碰界面状态；怎么跑在 `app/actions/`。
//!
//! 文件只在读进来的时候校验一次（`action_file`），之后运行时信任 `ActionDefinition`：
//! 名字不空、格式认识、路径模板开头正确、命令能切开，这些都不再检查。

mod action_file;
mod convert_format;
mod definition;
mod library;
pub(crate) mod notice;
pub(crate) mod template;
pub(crate) mod type_filter;

pub(crate) use convert_format::ConvertFormat;
pub(crate) use definition::{ActionDefinition, ActionId, ActionStep, TrashScope};
pub(crate) use library::{action_folder, ActionFileProblem, ActionLibrary};
pub(crate) use notice::{ActionNotice, NoticeValues};
pub(crate) use template::{PathTemplate, RunValues};
