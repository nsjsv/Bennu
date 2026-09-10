use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use desktop_linux::{LocalRequestError, LocalWorkspaceRequest};

use crate::model::{
    BrowserPaneId, BrowserPaneLayout, BrowserPaneSession, BrowserSessionSnapshot,
    BrowserTabSession, BrowserViewMode, ColumnBrowserViewport,
};

pub(crate) const HELP_TEXT: &str = "Usage: bennu [OPTIONS] [PATH]...\n\nOpen local directories or reveal local files.\n\nArguments:\n  [PATH]...        Local directories or files to open\n\nOptions:\n  -h, --help       Print help\n  -V, --version    Print version\n";
pub(crate) const VERSION_TEXT: &str = concat!("bennu ", env!("CARGO_PKG_VERSION"), "\n");
pub(crate) const RENDERER_PROBE_ARGUMENT: &str = "--renderer-probe";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CommandLineAction {
    Launch(ApplicationLaunchRequest),
    ActivationService,
    RendererProbe,
    PrintHelp,
    PrintVersion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ApplicationLaunchRequest {
    ConfiguredStartup,
    ExplicitWorkspace(ExplicitWorkspace),
}

impl ApplicationLaunchRequest {
    pub(crate) fn allows_browser_session_persistence(&self) -> bool {
        matches!(self, Self::ConfiguredStartup)
    }

    pub(crate) fn activation_paths(&self) -> Vec<PathBuf> {
        match self {
            Self::ConfiguredStartup => Vec::new(),
            Self::ExplicitWorkspace(workspace) => workspace.activation_paths.clone(),
        }
    }

    pub(crate) fn explicit_browser_session(
        &self,
        view_mode: BrowserViewMode,
    ) -> Option<BrowserSessionSnapshot> {
        let Self::ExplicitWorkspace(workspace) = self else {
            return None;
        };
        Some(workspace.browser_session(view_mode))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExplicitWorkspace {
    workspace: LocalWorkspaceRequest,
    activation_paths: Vec<PathBuf>,
}

impl ExplicitWorkspace {
    pub(crate) fn from_desktop_workspace(workspace: LocalWorkspaceRequest) -> Self {
        Self {
            workspace,
            activation_paths: Vec::new(),
        }
    }

    fn browser_session(&self, view_mode: BrowserViewMode) -> BrowserSessionSnapshot {
        let tabs = self
            .workspace
            .tabs()
            .iter()
            .enumerate()
            .map(|(id, tab)| BrowserTabSession {
                id,
                directory: tab.directory().to_path_buf(),
                is_trash_view: false,
                selected: tab.selected_paths().first().cloned(),
                selected_paths: tab.selected_paths().iter().cloned().collect(),
                deepest_open_column_directory: None,
                expanded_directories: Vec::new(),
                view_mode,
                back_stack: Vec::new(),
                forward_stack: Vec::new(),
            })
            .collect();
        BrowserSessionSnapshot {
            panes: vec![BrowserPaneSession {
                id: BrowserPaneId::PRIMARY,
                tabs,
                active_tab_id: 0,
                column_browser_viewport: ColumnBrowserViewport::default(),
                column_viewports: HashMap::new(),
            }],
            layout: BrowserPaneLayout::Single {
                active: BrowserPaneId::PRIMARY,
            },
        }
    }
}

#[derive(Debug)]
pub(crate) enum CommandLineError {
    Arguments(String),
    CurrentDirectory(io::Error),
    PathUnavailable { path: PathBuf, source: io::Error },
    UnsupportedPath { path: PathBuf },
    MissingParent { path: PathBuf },
    DirectoryUnreadable { path: PathBuf, source: io::Error },
}

impl fmt::Display for CommandLineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Arguments(message) => write!(formatter, "{message}"),
            Self::CurrentDirectory(error) => {
                write!(
                    formatter,
                    "could not determine the current directory: {error}"
                )
            }
            Self::PathUnavailable { path, source } => {
                write!(formatter, "cannot open '{}': {source}", path.display())
            }
            Self::UnsupportedPath { path } => write!(
                formatter,
                "cannot open '{}': only directories and regular files are supported",
                path.display()
            ),
            Self::MissingParent { path } => {
                write!(
                    formatter,
                    "cannot reveal '{}': no parent directory",
                    path.display()
                )
            }
            Self::DirectoryUnreadable { path, source } => {
                write!(
                    formatter,
                    "cannot read directory '{}': {source}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for CommandLineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CurrentDirectory(error)
            | Self::PathUnavailable { source: error, .. }
            | Self::DirectoryUnreadable { source: error, .. } => Some(error),
            Self::Arguments(_) | Self::UnsupportedPath { .. } | Self::MissingParent { .. } => None,
        }
    }
}

pub(crate) fn parse_process_arguments() -> Result<CommandLineAction, CommandLineError> {
    let current_directory = std::env::current_dir().map_err(CommandLineError::CurrentDirectory)?;
    parse_arguments(std::env::args_os().skip(1), &current_directory)
}

pub(crate) fn parse_arguments(
    arguments: impl IntoIterator<Item = OsString>,
    current_directory: &Path,
) -> Result<CommandLineAction, CommandLineError> {
    use lexopt::Arg::{Long, Short, Value};

    let mut parser = lexopt::Parser::from_args(arguments);
    let mut path_arguments = Vec::new();
    let mut activation_service = false;
    let mut renderer_probe = false;
    while let Some(argument) = parser
        .next()
        .map_err(|error| CommandLineError::Arguments(error.to_string()))?
    {
        match argument {
            Short('h') | Long("help") => return Ok(CommandLineAction::PrintHelp),
            Short('V') | Long("version") => return Ok(CommandLineAction::PrintVersion),
            Long("activation-service") => activation_service = true,
            Long("renderer-probe") => renderer_probe = true,
            Value(path) => path_arguments.push(path),
            unexpected => {
                return Err(CommandLineError::Arguments(
                    unexpected.unexpected().to_string(),
                ));
            }
        }
    }

    if activation_service && renderer_probe {
        return Err(CommandLineError::Arguments(
            "--activation-service and --renderer-probe cannot be used together".to_owned(),
        ));
    }

    if renderer_probe {
        if path_arguments.is_empty() {
            return Ok(CommandLineAction::RendererProbe);
        }
        return Err(CommandLineError::Arguments(
            "--renderer-probe does not accept path arguments".to_owned(),
        ));
    }

    if activation_service {
        if path_arguments.is_empty() {
            return Ok(CommandLineAction::ActivationService);
        }
        return Err(CommandLineError::Arguments(
            "--activation-service does not accept path arguments".to_owned(),
        ));
    }

    if path_arguments.is_empty() {
        return Ok(CommandLineAction::Launch(
            ApplicationLaunchRequest::ConfiguredStartup,
        ));
    }

    let workspace = classify_explicit_workspace(path_arguments, current_directory)?;
    Ok(CommandLineAction::Launch(
        ApplicationLaunchRequest::ExplicitWorkspace(workspace),
    ))
}

fn classify_explicit_workspace(
    path_arguments: Vec<OsString>,
    current_directory: &Path,
) -> Result<ExplicitWorkspace, CommandLineError> {
    let mut activation_paths = Vec::with_capacity(path_arguments.len());
    for argument in path_arguments {
        let argument_path = PathBuf::from(argument);
        let path = if argument_path.is_absolute() {
            argument_path
        } else {
            current_directory.join(argument_path)
        };
        activation_paths.push(path);
    }
    let workspace = LocalWorkspaceRequest::from_cli_paths(activation_paths.clone())
        .map_err(command_line_error_from_local_request)?;
    Ok(ExplicitWorkspace {
        workspace,
        activation_paths,
    })
}

fn command_line_error_from_local_request(error: LocalRequestError) -> CommandLineError {
    match error {
        LocalRequestError::PathUnavailable { path, source } => {
            CommandLineError::PathUnavailable { path, source }
        }
        LocalRequestError::UnsupportedPath { path } => CommandLineError::UnsupportedPath { path },
        LocalRequestError::MissingParent { path } => CommandLineError::MissingParent { path },
        LocalRequestError::DirectoryUnreadable { path, source } => {
            CommandLineError::DirectoryUnreadable { path, source }
        }
        other => CommandLineError::Arguments(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;

    use tempfile::TempDir;

    use super::{
        parse_arguments, ApplicationLaunchRequest, CommandLineAction, CommandLineError,
        ExplicitWorkspace, HELP_TEXT,
    };
    use crate::model::{BrowserPaneId, BrowserPaneLayout, BrowserViewMode};

    fn parse(
        arguments: impl IntoIterator<Item = OsString>,
        current_directory: &std::path::Path,
    ) -> Result<CommandLineAction, CommandLineError> {
        parse_arguments(arguments, current_directory)
    }

    fn explicit_workspace(action: CommandLineAction) -> ExplicitWorkspace {
        let CommandLineAction::Launch(ApplicationLaunchRequest::ExplicitWorkspace(workspace)) =
            action
        else {
            panic!("expected explicit workspace launch");
        };
        workspace
    }

    #[test]
    fn no_arguments_use_configured_startup() {
        let root = TempDir::new().expect("create temp directory");

        let action = parse(Vec::new(), root.path()).expect("parse no arguments");

        assert_eq!(
            action,
            CommandLineAction::Launch(ApplicationLaunchRequest::ConfiguredStartup)
        );
    }

    #[test]
    fn hidden_activation_service_rejects_path_operands() {
        let root = TempDir::new().expect("create temp directory");

        assert_eq!(
            parse([OsString::from("--activation-service")], root.path())
                .expect("parse activation service"),
            CommandLineAction::ActivationService
        );
        let error = parse(
            [
                OsString::from("--activation-service"),
                root.path().as_os_str().to_owned(),
            ],
            root.path(),
        )
        .expect_err("activation service must reject paths");
        assert!(matches!(error, CommandLineError::Arguments(_)));
    }

    #[test]
    fn hidden_renderer_probe_rejects_extra_operands_and_other_hidden_modes() {
        let root = TempDir::new().expect("create temp directory");

        assert_eq!(
            parse([OsString::from("--renderer-probe")], root.path()).expect("parse renderer probe"),
            CommandLineAction::RendererProbe
        );
        let path_error = parse(
            [
                OsString::from("--renderer-probe"),
                root.path().as_os_str().to_owned(),
            ],
            root.path(),
        )
        .expect_err("renderer probe must reject paths");
        assert!(matches!(path_error, CommandLineError::Arguments(_)));

        let mixed_mode_error = parse(
            [
                OsString::from("--renderer-probe"),
                OsString::from("--activation-service"),
            ],
            root.path(),
        )
        .expect_err("renderer probe must reject the activation service mode");
        assert!(matches!(mixed_mode_error, CommandLineError::Arguments(_)));
        assert!(!HELP_TEXT.contains("renderer-probe"));
    }

    #[test]
    fn help_and_version_exit_without_launch() {
        let root = TempDir::new().expect("create temp directory");

        assert_eq!(
            parse([OsString::from("--help")], root.path()).expect("parse help"),
            CommandLineAction::PrintHelp
        );
        assert_eq!(
            parse([OsString::from("-h")], root.path()).expect("parse short help"),
            CommandLineAction::PrintHelp
        );
        assert_eq!(
            parse([OsString::from("--version")], root.path()).expect("parse version"),
            CommandLineAction::PrintVersion
        );
        assert_eq!(
            parse([OsString::from("-V")], root.path()).expect("parse short version"),
            CommandLineAction::PrintVersion
        );
    }

    #[test]
    fn unknown_option_is_rejected() {
        let root = TempDir::new().expect("create temp directory");

        let error = parse([OsString::from("--unknown")], root.path())
            .expect_err("unknown option must fail");

        assert!(matches!(error, CommandLineError::Arguments(_)));
    }

    #[test]
    fn directories_and_files_form_ordered_tabs() {
        let root = TempDir::new().expect("create temp directory");
        let first = root.path().join("first");
        let second = root.path().join("second");
        fs::create_dir_all(&first).expect("create first directory");
        fs::create_dir_all(&second).expect("create second directory");
        let first_file = first.join("one.txt");
        let second_file = first.join("two.txt");
        fs::write(&first_file, "one").expect("write first file");
        fs::write(&second_file, "two").expect("write second file");

        let workspace = explicit_workspace(
            parse(
                [
                    first_file.clone().into_os_string(),
                    second.clone().into_os_string(),
                    first.clone().into_os_string(),
                    second_file.clone().into_os_string(),
                ],
                root.path(),
            )
            .expect("parse workspace"),
        );

        assert_eq!(workspace.workspace.tabs().len(), 2);
        assert_eq!(workspace.workspace.tabs()[0].directory(), first);
        assert_eq!(
            workspace.workspace.tabs()[0].selected_paths(),
            &[first_file, second_file]
        );
        assert_eq!(workspace.workspace.tabs()[1].directory(), second);
        assert!(workspace.workspace.tabs()[1].selected_paths().is_empty());
    }

    #[test]
    fn relative_paths_use_supplied_working_directory() {
        let root = TempDir::new().expect("create temp directory");
        let directory = root.path().join("relative");
        fs::create_dir(&directory).expect("create relative directory");

        let workspace = explicit_workspace(
            parse([OsString::from("relative")], root.path()).expect("parse relative directory"),
        );

        assert_eq!(workspace.workspace.tabs()[0].directory(), directory);
    }

    #[test]
    fn invalid_path_rejects_entire_workspace() {
        let root = TempDir::new().expect("create temp directory");
        let valid = root.path().join("valid");
        fs::create_dir(&valid).expect("create valid directory");
        let missing = root.path().join("missing");

        let error = parse(
            [valid.into_os_string(), missing.clone().into_os_string()],
            root.path(),
        )
        .expect_err("one invalid path rejects all paths");

        assert!(matches!(
            error,
            CommandLineError::PathUnavailable { path, .. } if path == missing
        ));
    }

    #[test]
    fn double_dash_accepts_dash_prefixed_path() {
        let root = TempDir::new().expect("create temp directory");
        let directory = root.path().join("-workspace");
        fs::create_dir(&directory).expect("create dash-prefixed directory");

        let workspace = explicit_workspace(
            parse(
                [OsString::from("--"), OsString::from("-workspace")],
                root.path(),
            )
            .expect("parse dash-prefixed path"),
        );

        assert_eq!(workspace.workspace.tabs()[0].directory(), directory);
    }

    #[test]
    fn explicit_workspace_builds_single_pane_session() {
        let root = TempDir::new().expect("create temp directory");
        let directory = root.path().join("workspace");
        fs::create_dir(&directory).expect("create workspace");
        let file = directory.join("selected.txt");
        fs::write(&file, "selected").expect("write selected file");
        let request = match parse([file.clone().into_os_string()], root.path())
            .expect("parse selected file")
        {
            CommandLineAction::Launch(request) => request,
            _ => panic!("expected launch request"),
        };

        for view_mode in [
            BrowserViewMode::List,
            BrowserViewMode::Icons,
            BrowserViewMode::Columns,
        ] {
            let session = request
                .explicit_browser_session(view_mode)
                .expect("build explicit browser session");

            assert_eq!(
                session.layout,
                BrowserPaneLayout::Single {
                    active: BrowserPaneId::PRIMARY
                }
            );
            assert_eq!(session.panes.len(), 1);
            assert_eq!(session.panes[0].active_tab_id, 0);
            assert_eq!(session.panes[0].tabs.len(), 1);
            assert_eq!(session.panes[0].tabs[0].directory, directory);
            assert_eq!(session.panes[0].tabs[0].selected.as_ref(), Some(&file));
            assert!(session.panes[0].tabs[0].selected_paths.contains(&file));
            assert_eq!(session.panes[0].tabs[0].view_mode, view_mode);
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlink_targets_follow_navigation_semantics() {
        use std::os::unix::fs::symlink;

        let root = TempDir::new().expect("create temp directory");
        let directory = root.path().join("directory");
        fs::create_dir(&directory).expect("create directory");
        let file = root.path().join("file.txt");
        fs::write(&file, "file").expect("write file");
        let directory_link = root.path().join("directory-link");
        let file_link = root.path().join("file-link");
        symlink(&directory, &directory_link).expect("link directory");
        symlink(&file, &file_link).expect("link file");

        let workspace = explicit_workspace(
            parse(
                [
                    directory_link.clone().into_os_string(),
                    file_link.clone().into_os_string(),
                ],
                root.path(),
            )
            .expect("parse symlink paths"),
        );

        assert_eq!(workspace.workspace.tabs()[0].directory(), directory_link);
        assert_eq!(workspace.workspace.tabs()[1].directory(), root.path());
        assert_eq!(workspace.workspace.tabs()[1].selected_paths(), &[file_link]);
    }

    #[cfg(unix)]
    #[test]
    fn special_file_is_rejected() {
        use std::os::unix::net::UnixListener;

        let root = TempDir::new().expect("create temp directory");
        let socket = root.path().join("socket");
        let _listener = UnixListener::bind(&socket).expect("bind unix socket");

        let error = parse([socket.clone().into_os_string()], root.path())
            .expect_err("special file must fail");

        assert!(matches!(
            error,
            CommandLineError::UnsupportedPath { path } if path == socket
        ));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_paths_are_preserved() {
        use std::os::unix::ffi::OsStringExt;

        let root = TempDir::new().expect("create temp directory");
        let name = OsString::from_vec(vec![b'w', b'o', b'r', b'k', 0xff]);
        let directory = root.path().join(&name);
        fs::create_dir(&directory).expect("create non-UTF-8 directory");
        let file_name = OsString::from_vec(vec![b'f', b'i', b'l', b'e', 0xfe]);
        let file = directory.join(&file_name);
        fs::write(&file, "file").expect("write non-UTF-8 file");

        let workspace = explicit_workspace(
            parse([file.clone().into_os_string()], root.path()).expect("parse non-UTF-8 file"),
        );

        assert_eq!(workspace.workspace.tabs()[0].directory(), directory);
        assert_eq!(workspace.workspace.tabs()[0].selected_paths(), &[file]);
    }
}
