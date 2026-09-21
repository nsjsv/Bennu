use std::ffi::OsStr;
use std::fs::{self, Metadata, ReadDir};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio_util::sync::CancellationToken;

use crate::config::{SearchExcludeRules, SearchTraversalDecision};
use crate::error::{SearchError, SearchResult};
use crate::model::SearchFileKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TraversalDepth {
    DirectChildren,
    Recursive,
}

#[derive(Debug)]
pub(crate) struct FilesystemEntry {
    path: PathBuf,
    metadata: Metadata,
    kind: SearchFileKind,
}

#[derive(Debug)]
pub(crate) enum FilesystemObservation<T> {
    Complete(T),
    Inaccessible { scope: PathBuf },
    Missing { scope: PathBuf },
    PolicyExcluded { scope: PathBuf },
    DelegatedToNestedRoot { scope: PathBuf },
}

#[derive(Debug)]
pub(crate) enum TraversalEvent {
    Entry(FilesystemEntry),
    Observation(FilesystemObservation<PathBuf>),
}

impl FilesystemEntry {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    pub(crate) fn kind(&self) -> SearchFileKind {
        self.kind
    }
}

#[derive(Debug, Clone)]
pub(crate) struct LocalFilesystemBoundary {
    root: PathBuf,
    root_device: u64,
    rules: SearchExcludeRules,
}

impl LocalFilesystemBoundary {
    #[cfg(test)]
    pub(crate) fn open(root: &Path, rules: &SearchExcludeRules) -> SearchResult<Self> {
        match Self::observe(root, rules)? {
            FilesystemObservation::Complete(boundary) => Ok(boundary),
            FilesystemObservation::Inaccessible { scope } => Err(SearchError::Inaccessible {
                path: scope,
                source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            }),
            FilesystemObservation::Missing { scope } => Err(io_error(
                &scope,
                std::io::Error::from(std::io::ErrorKind::NotFound),
            )),
            FilesystemObservation::PolicyExcluded { scope } => Err(io_error(
                &scope,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "search root is excluded by filesystem policy",
                ),
            )),
            FilesystemObservation::DelegatedToNestedRoot { scope } => Err(io_error(
                &scope,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "search root is owned by another configured root",
                ),
            )),
        }
    }

    pub(crate) fn observe(
        root: &Path,
        rules: &SearchExcludeRules,
    ) -> SearchResult<FilesystemObservation<Self>> {
        let metadata = match fs::symlink_metadata(root) {
            Ok(metadata) => metadata,
            Err(source) => return classify_path_error(root, source),
        };
        if entry_kind(&metadata) != SearchFileKind::Directory {
            return Err(io_error(
                root,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "search root must be a directory and cannot be a symlink",
                ),
            ));
        }

        Ok(FilesystemObservation::Complete(Self {
            root: root.to_path_buf(),
            root_device: metadata.dev(),
            rules: rules.clone(),
        }))
    }

    pub(crate) fn inspect_path(
        &self,
        path: &Path,
    ) -> SearchResult<FilesystemObservation<FilesystemEntry>> {
        let Some(relative_path) = safe_relative_path(path, &self.root) else {
            return Ok(FilesystemObservation::PolicyExcluded {
                scope: path.to_path_buf(),
            });
        };

        if relative_path.as_os_str().is_empty() {
            let metadata = match fs::symlink_metadata(path) {
                Ok(metadata) => metadata,
                Err(source) => return classify_path_error(path, source),
            };
            return Ok(FilesystemObservation::Complete(FilesystemEntry {
                path: path.to_path_buf(),
                kind: entry_kind(&metadata),
                metadata,
            }));
        }

        let mut current_path = self.root.clone();
        let component_count = relative_path.components().count();
        for (index, component) in relative_path.components().enumerate() {
            let Component::Normal(name) = component else {
                return Ok(FilesystemObservation::PolicyExcluded {
                    scope: path.to_path_buf(),
                });
            };
            current_path.push(name);

            let metadata = match fs::symlink_metadata(&current_path) {
                Ok(metadata) => metadata,
                Err(source) => return classify_path_error(&current_path, source),
            };
            let kind = entry_kind(&metadata);
            let is_target = index + 1 == component_count;
            let traversal_decision = self.entry_traversal_decision(&current_path, kind);
            if is_target {
                match traversal_decision {
                    SearchTraversalDecision::Excluded => {
                        return Ok(FilesystemObservation::PolicyExcluded {
                            scope: current_path,
                        })
                    }
                    SearchTraversalDecision::DelegatedToNestedRoot => {
                        return Ok(FilesystemObservation::DelegatedToNestedRoot {
                            scope: current_path,
                        })
                    }
                    SearchTraversalDecision::Included => {}
                }
                if kind != SearchFileKind::File && kind != SearchFileKind::Directory {
                    return Ok(FilesystemObservation::PolicyExcluded {
                        scope: current_path,
                    });
                }
                return Ok(FilesystemObservation::Complete(FilesystemEntry {
                    path: current_path,
                    metadata,
                    kind,
                }));
            }

            if kind != SearchFileKind::Directory {
                return Ok(FilesystemObservation::PolicyExcluded {
                    scope: current_path,
                });
            }
            match traversal_decision {
                SearchTraversalDecision::Excluded => {
                    return Ok(FilesystemObservation::PolicyExcluded {
                        scope: current_path,
                    })
                }
                SearchTraversalDecision::DelegatedToNestedRoot => {
                    return Ok(FilesystemObservation::DelegatedToNestedRoot {
                        scope: current_path,
                    })
                }
                SearchTraversalDecision::Included if metadata.dev() != self.root_device => {
                    return Ok(FilesystemObservation::PolicyExcluded {
                        scope: current_path,
                    })
                }
                SearchTraversalDecision::Included => {}
            }
        }

        Ok(FilesystemObservation::PolicyExcluded {
            scope: path.to_path_buf(),
        })
    }

    pub(crate) fn walk_root(
        &self,
        depth: TraversalDepth,
        cancellation: &CancellationToken,
    ) -> SearchResult<FilesystemObservation<LocalTreeWalker>> {
        ensure_not_cancelled(cancellation)?;
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(source) => return classify_path_error(&self.root, source),
        };
        Ok(FilesystemObservation::Complete(LocalTreeWalker::new(
            self.clone(),
            depth,
            cancellation.clone(),
            vec![OpenDirectory {
                path: self.root.clone(),
                entries,
                observation_complete: true,
            }],
        )))
    }

    pub(crate) fn walk_directory(
        &self,
        directory: &Path,
        depth: TraversalDepth,
        cancellation: &CancellationToken,
    ) -> SearchResult<FilesystemObservation<LocalTreeWalker>> {
        if directory == self.root {
            return self.walk_root(depth, cancellation);
        }

        ensure_not_cancelled(cancellation)?;
        let entry = match self.inspect_path(directory)? {
            FilesystemObservation::Complete(entry) => entry,
            FilesystemObservation::Inaccessible { scope } => {
                return Ok(FilesystemObservation::Inaccessible { scope })
            }
            FilesystemObservation::Missing { scope } => {
                return Ok(FilesystemObservation::Missing { scope })
            }
            FilesystemObservation::PolicyExcluded { scope } => {
                return Ok(FilesystemObservation::PolicyExcluded { scope })
            }
            FilesystemObservation::DelegatedToNestedRoot { scope } => {
                return Ok(FilesystemObservation::DelegatedToNestedRoot { scope })
            }
        };
        if entry.kind != SearchFileKind::Directory || entry.metadata.dev() != self.root_device {
            return Ok(FilesystemObservation::PolicyExcluded {
                scope: directory.to_path_buf(),
            });
        }

        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(source) => return classify_path_error(directory, source),
        };
        Ok(FilesystemObservation::Complete(LocalTreeWalker::new(
            self.clone(),
            depth,
            cancellation.clone(),
            vec![OpenDirectory {
                path: directory.to_path_buf(),
                entries,
                observation_complete: true,
            }],
        )))
    }

    fn entry_traversal_decision(
        &self,
        path: &Path,
        kind: SearchFileKind,
    ) -> SearchTraversalDecision {
        if kind == SearchFileKind::Directory {
            self.rules.directory_traversal_decision(&self.root, path)
        } else {
            self.rules.traversal_decision(&self.root, path)
        }
    }
}

pub(crate) fn observe_path(path: &Path) -> SearchResult<FilesystemObservation<FilesystemEntry>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) => return classify_path_error(path, source),
    };
    Ok(FilesystemObservation::Complete(FilesystemEntry {
        path: path.to_path_buf(),
        kind: entry_kind(&metadata),
        metadata,
    }))
}

pub(crate) struct LocalTreeWalker {
    boundary: LocalFilesystemBoundary,
    depth: TraversalDepth,
    cancellation: CancellationToken,
    directories: Vec<OpenDirectory>,
    pending_event: Option<TraversalEvent>,
}

struct OpenDirectory {
    path: PathBuf,
    entries: ReadDir,
    observation_complete: bool,
}

impl LocalTreeWalker {
    fn new(
        boundary: LocalFilesystemBoundary,
        depth: TraversalDepth,
        cancellation: CancellationToken,
        directories: Vec<OpenDirectory>,
    ) -> Self {
        Self {
            boundary,
            depth,
            cancellation,
            directories,
            pending_event: None,
        }
    }

    pub(crate) fn next_event(&mut self) -> SearchResult<Option<TraversalEvent>> {
        if let Some(pending_event) = self.pending_event.take() {
            return Ok(Some(pending_event));
        }
        loop {
            ensure_not_cancelled(&self.cancellation)?;
            let (directory_path, next_entry) = match self.directories.last_mut() {
                Some(directory) => (directory.path.clone(), directory.entries.next()),
                None => return Ok(None),
            };
            let Some(entry) = next_entry else {
                let directory = self
                    .directories
                    .pop()
                    .expect("directory stack is not empty");
                if directory.observation_complete {
                    return Ok(Some(TraversalEvent::Observation(
                        FilesystemObservation::Complete(directory.path),
                    )));
                }
                continue;
            };
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    if let Some(directory) = self.directories.last_mut() {
                        directory.observation_complete = false;
                    }
                    // ReadDir 条目不携带可靠的子路径，只能保守保护整个所在目录。
                    return Ok(Some(TraversalEvent::Observation(
                        FilesystemObservation::Inaccessible {
                            scope: directory_path,
                        },
                    )));
                }
            };

            ensure_not_cancelled(&self.cancellation)?;
            let path = entry.path();
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(source) => {
                    return Ok(Some(TraversalEvent::Observation(classify_path_error(
                        &path, source,
                    )?)));
                }
            };
            let kind = entry_kind(&metadata);
            match self.boundary.entry_traversal_decision(&path, kind) {
                SearchTraversalDecision::Excluded => {
                    return Ok(Some(TraversalEvent::Observation(
                        FilesystemObservation::PolicyExcluded { scope: path },
                    )))
                }
                SearchTraversalDecision::DelegatedToNestedRoot => {
                    return Ok(Some(TraversalEvent::Observation(
                        FilesystemObservation::DelegatedToNestedRoot { scope: path },
                    )))
                }
                SearchTraversalDecision::Included => {}
            }

            if can_descend(kind, metadata.dev(), self.boundary.root_device, self.depth) {
                match fs::read_dir(&path) {
                    Ok(entries) => self.directories.push(OpenDirectory {
                        path: path.clone(),
                        entries,
                        observation_complete: true,
                    }),
                    Err(source) => {
                        self.pending_event = Some(TraversalEvent::Entry(FilesystemEntry {
                            path: path.clone(),
                            metadata,
                            kind,
                        }));
                        return Ok(Some(TraversalEvent::Observation(classify_path_error(
                            &path, source,
                        )?)));
                    }
                }
            } else if self.depth == TraversalDepth::Recursive && kind != SearchFileKind::File {
                self.pending_event = Some(TraversalEvent::Entry(FilesystemEntry {
                    path: path.clone(),
                    metadata,
                    kind,
                }));
                return Ok(Some(TraversalEvent::Observation(
                    FilesystemObservation::PolicyExcluded { scope: path },
                )));
            }

            return Ok(Some(TraversalEvent::Entry(FilesystemEntry {
                path,
                metadata,
                kind,
            })));
        }
    }
}

pub(crate) fn ensure_not_cancelled(cancellation: &CancellationToken) -> SearchResult<()> {
    if cancellation.is_cancelled() {
        Err(SearchError::Cancelled)
    } else {
        Ok(())
    }
}

pub(crate) fn display_name(path: &Path) -> String {
    path.file_name()
        .and_then(OsStr::to_str)
        .map(str::to_owned)
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

pub(crate) fn file_time_ms(time: Option<SystemTime>) -> Option<i64> {
    time.and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
}

pub(crate) fn mime_type_for_path(path: &Path) -> Option<String> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    let mime_type = match extension.as_str() {
        "txt" | "rs" | "toml" | "json" | "yaml" | "yml" | "log" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" => "text/javascript",
        "rtf" => "application/rtf",
        "pdf" => "application/pdf",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "odp" => "application/vnd.oasis.opendocument.presentation",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "ico" => "image/vnd.microsoft.icon",
        "svg" => "image/svg+xml",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "ogg" | "opus" => "audio/ogg",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "mp4" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        "mov" => "video/quicktime",
        "m4v" => "video/x-m4v",
        "mpg" | "mpeg" => "video/mpeg",
        "zip" => "application/zip",
        "7z" => "application/x-7z-compressed",
        "rar" => "application/vnd.rar",
        "tar" => "application/x-tar",
        "gz" => "application/gzip",
        "bz2" => "application/x-bzip2",
        "xz" => "application/x-xz",
        "zst" => "application/zstd",
        _ => return None,
    };
    Some(mime_type.to_owned())
}

fn entry_kind(metadata: &Metadata) -> SearchFileKind {
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        SearchFileKind::Symlink
    } else if file_type.is_dir() {
        SearchFileKind::Directory
    } else if file_type.is_file() {
        SearchFileKind::File
    } else {
        SearchFileKind::Other
    }
}

fn can_descend(
    kind: SearchFileKind,
    entry_device: u64,
    root_device: u64,
    depth: TraversalDepth,
) -> bool {
    kind == SearchFileKind::Directory
        && entry_device == root_device
        && depth == TraversalDepth::Recursive
}

fn safe_relative_path<'a>(path: &'a Path, root: &Path) -> Option<&'a Path> {
    let relative_path = path.strip_prefix(root).ok()?;
    relative_path
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
        .then_some(relative_path)
}

fn io_error(path: &Path, source: std::io::Error) -> SearchError {
    SearchError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn classify_path_error<T>(
    scope: &Path,
    source: std::io::Error,
) -> SearchResult<FilesystemObservation<T>> {
    match source.kind() {
        std::io::ErrorKind::PermissionDenied => Ok(FilesystemObservation::Inaccessible {
            scope: scope.to_path_buf(),
        }),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory => {
            Ok(FilesystemObservation::Missing {
                scope: scope.to_path_buf(),
            })
        }
        _ => Err(io_error(scope, source)),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::Path;

    use tempfile::tempdir;
    use tokio_util::sync::CancellationToken;

    use crate::config::{SearchExcludeRules, SearchIndexConfig};
    use crate::error::SearchError;
    use crate::model::SearchFileKind;

    use super::{
        can_descend, classify_path_error, mime_type_for_path, FilesystemObservation,
        LocalFilesystemBoundary, TraversalDepth,
    };

    #[test]
    fn permission_denied_is_classified_as_inaccessible() {
        let scope = Path::new("/tmp/private");
        let observation = classify_path_error::<()>(
            scope,
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        )
        .unwrap();

        assert!(matches!(
            observation,
            FilesystemObservation::Inaccessible { scope: observed_scope }
                if observed_scope == scope
        ));
    }

    #[test]
    fn common_search_categories_share_one_extension_to_mime_boundary() {
        let expected_mime_types = [
            ("notes.md", "text/markdown"),
            ("photo.png", "image/png"),
            ("recording.flac", "audio/flac"),
            ("movie.mkv", "video/x-matroska"),
            ("bundle.7z", "application/x-7z-compressed"),
        ];

        for (file_name, expected_mime_type) in expected_mime_types {
            assert_eq!(
                mime_type_for_path(Path::new(file_name)).as_deref(),
                Some(expected_mime_type),
                "unexpected MIME type for {file_name}"
            );
        }
    }

    #[test]
    fn office_formats_keep_mime_metadata_when_content_is_not_extracted() {
        let expected_mime_types = [
            ("report.doc", "application/msword"),
            (
                "report.docx",
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            ),
            ("workbook.xls", "application/vnd.ms-excel"),
            (
                "workbook.xlsx",
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            ),
            ("slides.ppt", "application/vnd.ms-powerpoint"),
            (
                "slides.pptx",
                "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            ),
            ("document.odt", "application/vnd.oasis.opendocument.text"),
            (
                "spreadsheet.ods",
                "application/vnd.oasis.opendocument.spreadsheet",
            ),
            (
                "presentation.odp",
                "application/vnd.oasis.opendocument.presentation",
            ),
        ];

        for (file_name, expected_mime_type) in expected_mime_types {
            assert_eq!(
                mime_type_for_path(Path::new(file_name)).as_deref(),
                Some(expected_mime_type),
                "unexpected MIME type for {file_name}"
            );
        }
    }

    #[test]
    fn symlink_is_an_entry_but_its_target_is_not_walked() {
        let content = tempdir().unwrap();
        let external = tempdir().unwrap();
        fs::write(external.path().join("outside.txt"), "outside").unwrap();
        symlink(external.path(), content.path().join("linked-directory")).unwrap();

        let boundary =
            LocalFilesystemBoundary::open(content.path(), &SearchExcludeRules::new(Vec::new()))
                .unwrap();
        let FilesystemObservation::Complete(mut walker) = boundary
            .walk_root(TraversalDepth::Recursive, &CancellationToken::new())
            .unwrap()
        else {
            panic!("temporary directory root must be observable");
        };
        let mut entries = Vec::new();
        while let Some(event) = walker.next_event().unwrap() {
            if let super::TraversalEvent::Entry(entry) = event {
                entries.push((entry.path().to_path_buf(), entry.kind()));
            }
        }

        assert!(entries.contains(&(
            content.path().join("linked-directory"),
            SearchFileKind::Symlink
        )));
        assert!(!entries
            .iter()
            .any(|(path, _)| path.ends_with("outside.txt")));
    }

    #[test]
    fn recursive_descent_requires_a_real_directory_on_the_root_device() {
        assert!(can_descend(
            SearchFileKind::Directory,
            7,
            7,
            TraversalDepth::Recursive
        ));
        assert!(!can_descend(
            SearchFileKind::Directory,
            8,
            7,
            TraversalDepth::Recursive
        ));
        assert!(!can_descend(
            SearchFileKind::Symlink,
            7,
            7,
            TraversalDepth::Recursive
        ));
        assert!(!can_descend(
            SearchFileKind::Directory,
            7,
            7,
            TraversalDepth::DirectChildren
        ));
    }

    #[test]
    fn hidden_nomedia_and_configured_subtrees_share_one_boundary() {
        let content = tempdir().unwrap();
        let hidden = content.path().join(".hidden");
        let media = content.path().join("media");
        let excluded = content.path().join("excluded");
        fs::create_dir(&hidden).unwrap();
        fs::create_dir(&media).unwrap();
        fs::create_dir(&excluded).unwrap();
        fs::write(hidden.join("hidden.txt"), "hidden").unwrap();
        fs::write(media.join(".nomedia"), "").unwrap();
        fs::write(media.join("media.txt"), "media").unwrap();
        fs::write(excluded.join("excluded.txt"), "excluded").unwrap();
        fs::write(content.path().join("visible.txt"), "visible").unwrap();

        let rules = SearchExcludeRules::new(vec![excluded]);
        let boundary = LocalFilesystemBoundary::open(content.path(), &rules).unwrap();
        let FilesystemObservation::Complete(mut walker) = boundary
            .walk_root(TraversalDepth::Recursive, &CancellationToken::new())
            .unwrap()
        else {
            panic!("temporary directory root must be observable");
        };
        let mut names = Vec::new();
        while let Some(event) = walker.next_event().unwrap() {
            if let super::TraversalEvent::Entry(entry) = event {
                names.push(super::display_name(entry.path()));
            }
        }

        assert_eq!(names, vec!["visible.txt"]);
    }

    #[test]
    fn local_entry_cannot_bypass_an_ignored_ancestor() {
        let content = tempdir().unwrap();
        let hidden = content.path().join(".hidden");
        fs::create_dir(&hidden).unwrap();
        let path = hidden.join("secret.txt");
        fs::write(&path, "secret").unwrap();

        let boundary =
            LocalFilesystemBoundary::open(content.path(), &SearchExcludeRules::new(Vec::new()))
                .unwrap();

        assert!(matches!(
            boundary.inspect_path(&path).unwrap(),
            FilesystemObservation::PolicyExcluded { .. }
        ));
    }

    #[test]
    fn nested_root_is_delegated_without_deleting_or_duplicate_traversal() {
        let content = tempdir().unwrap();
        let nested_root = content.path().join("nested-root");
        let nested_file = nested_root.join("owned-by-nested-root.txt");
        fs::create_dir(&nested_root).unwrap();
        fs::write(&nested_file, "nested").unwrap();

        let config = SearchIndexConfig {
            roots: vec![content.path().to_path_buf(), nested_root.clone()],
            ..SearchIndexConfig::default()
        };
        let rules = SearchExcludeRules::from_index_config(&config).unwrap();
        let home_boundary = LocalFilesystemBoundary::open(content.path(), &rules).unwrap();
        let FilesystemObservation::Complete(mut home_walker) = home_boundary
            .walk_root(TraversalDepth::Recursive, &CancellationToken::new())
            .unwrap()
        else {
            panic!("temporary directory root must be observable");
        };
        let mut delegated = false;
        let mut home_saw_nested_file = false;
        while let Some(event) = home_walker.next_event().unwrap() {
            match event {
                super::TraversalEvent::Observation(
                    FilesystemObservation::DelegatedToNestedRoot { scope },
                ) if scope == nested_root => delegated = true,
                super::TraversalEvent::Entry(entry) if entry.path() == nested_file => {
                    home_saw_nested_file = true
                }
                _ => {}
            }
        }

        let nested_boundary = LocalFilesystemBoundary::open(&nested_root, &rules).unwrap();
        let FilesystemObservation::Complete(mut nested_walker) = nested_boundary
            .walk_root(TraversalDepth::Recursive, &CancellationToken::new())
            .unwrap()
        else {
            panic!("nested temporary directory root must be observable");
        };
        let mut nested_saw_file = false;
        while let Some(event) = nested_walker.next_event().unwrap() {
            if let super::TraversalEvent::Entry(entry) = event {
                nested_saw_file |= entry.path() == nested_file;
            }
        }

        assert!(delegated);
        assert!(!home_saw_nested_file);
        assert!(nested_saw_file);
    }

    #[test]
    fn missing_root_reports_the_root_path() {
        let content = tempdir().unwrap();
        let missing = content.path().join("missing");

        let error = LocalFilesystemBoundary::open(&missing, &SearchExcludeRules::new(Vec::new()))
            .unwrap_err();

        assert!(matches!(error, SearchError::Io { path, .. } if path == missing));
    }

    #[test]
    fn symlink_root_is_rejected_without_following_its_target() {
        let target = tempdir().unwrap();
        let parent = tempdir().unwrap();
        let linked_root = parent.path().join("linked-root");
        symlink(target.path(), &linked_root).unwrap();

        let error =
            LocalFilesystemBoundary::open(&linked_root, &SearchExcludeRules::new(Vec::new()))
                .unwrap_err();

        assert!(matches!(error, SearchError::Io { path, .. } if path == linked_root));
    }
}
