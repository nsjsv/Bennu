use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::error::{SearchError, SearchResult};
use crate::VersionedSearchPathPreferences;

const SEARCH_PATH_FORMAT_VERSION: u32 = 1;
const SEARCH_PATH_SIDECAR_MAX_BYTES: u64 = 1_048_576;
const SEARCH_PATH_CONFIG_DIRECTORY: &str = "bennu";
const SEARCH_PATH_CONFIG_FILE: &str = "search-paths.json";
static TEMPORARY_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// 搜索路径偏好文件在用户配置目录下的位置。配置目录属于外部环境,由守护进程
/// 入口解析一次后经 `SearchDaemonCore::new` 显式传入,核心不自己读环境。
pub fn search_path_file_in(config_directory: &Path) -> PathBuf {
    config_directory
        .join(SEARCH_PATH_CONFIG_DIRECTORY)
        .join(SEARCH_PATH_CONFIG_FILE)
}

/// 本地目录回退用：直接读服务同一份路径偏好文件，服务没运行也能读。文件不存在 = 默认
/// 偏好；读不了或读不懂时返回错误，调用方据此拒绝回退，免得扫进用户排除的目录。
pub fn load_search_path_preferences(
    config_directory: &Path,
) -> SearchResult<crate::SearchPathPreferences> {
    Ok(SearchPathStore::at(search_path_file_in(config_directory))
        .load()?
        .map(|versioned| versioned.preferences)
        .unwrap_or_default())
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredSearchPathPreferences {
    format_version: u32,
    revision: u64,
    preferences: crate::SearchPathPreferences,
}

#[derive(Debug, Clone)]
pub(crate) struct SearchPathStore {
    path: PathBuf,
}

impl SearchPathStore {
    pub(crate) fn at(path: PathBuf) -> Self {
        Self { path }
    }

    pub(crate) fn load(&self) -> SearchResult<Option<VersionedSearchPathPreferences>> {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let file = match options.open(&self.path) {
            Ok(file) => file,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(SearchError::Io {
                    path: self.path.clone(),
                    source,
                });
            }
        };
        let metadata = file.metadata().map_err(|source| SearchError::Io {
            path: self.path.clone(),
            source,
        })?;
        if !metadata.is_file() {
            return Err(SearchError::InvalidConfiguration(format!(
                "search path configuration is not a regular file: {}",
                self.path.display()
            )));
        }
        if metadata.len() > SEARCH_PATH_SIDECAR_MAX_BYTES {
            return Err(sidecar_too_large(metadata.len() as usize));
        }

        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(SEARCH_PATH_SIDECAR_MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|source| SearchError::Io {
                path: self.path.clone(),
                source,
            })?;
        if bytes.len() as u64 > SEARCH_PATH_SIDECAR_MAX_BYTES {
            return Err(sidecar_too_large(bytes.len()));
        }
        let stored: StoredSearchPathPreferences = serde_json::from_slice(&bytes)?;
        if stored.format_version != SEARCH_PATH_FORMAT_VERSION {
            return Err(SearchError::InvalidConfiguration(format!(
                "unsupported search path configuration format {}",
                stored.format_version
            )));
        }
        Ok(Some(VersionedSearchPathPreferences {
            revision: stored.revision,
            preferences: stored.preferences,
        }))
    }

    pub(crate) fn replace(&self, versioned: &VersionedSearchPathPreferences) -> SearchResult<()> {
        let stored = StoredSearchPathPreferences {
            format_version: SEARCH_PATH_FORMAT_VERSION,
            revision: versioned.revision,
            preferences: versioned.preferences.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&stored)?;
        if bytes.len() as u64 > SEARCH_PATH_SIDECAR_MAX_BYTES {
            return Err(sidecar_too_large(bytes.len()));
        }
        let parent = self.path.parent().ok_or_else(|| SearchError::Io {
            path: self.path.clone(),
            source: io::Error::new(
                io::ErrorKind::InvalidInput,
                "search path configuration has no parent directory",
            ),
        })?;
        fs::create_dir_all(parent).map_err(|source| SearchError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        let mut directory_options = OpenOptions::new();
        directory_options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            directory_options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW);
        }
        let directory = directory_options
            .open(parent)
            .map_err(|source| SearchError::Io {
                path: parent.to_path_buf(),
                source,
            })?;

        let (temporary_path, mut temporary_file) = self.create_temporary_file()?;
        if let Err(source) = temporary_file
            .write_all(&bytes)
            .and_then(|()| temporary_file.sync_all())
        {
            let _ = fs::remove_file(&temporary_path);
            return Err(SearchError::Io {
                path: temporary_path,
                source,
            });
        }
        drop(temporary_file);
        if let Err(source) = fs::rename(&temporary_path, &self.path) {
            let _ = fs::remove_file(&temporary_path);
            return Err(SearchError::Io {
                path: self.path.clone(),
                source,
            });
        }
        if let Err(source) = directory.sync_all() {
            // The rename is the visible commit boundary. Reporting failure now would leave the
            // daemon on the old desired state even though every reader sees the new sidecar.
            tracing::warn!(
                target: "file_search::search_path_store",
                path = %self.path.display(),
                error = %source,
                "search path sidecar committed but its directory could not be synced"
            );
        }
        Ok(())
    }

    fn create_temporary_file(&self) -> SearchResult<(PathBuf, File)> {
        for _ in 0..16 {
            let sequence = TEMPORARY_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let mut file_name = self
                .path
                .file_name()
                .unwrap_or_else(|| self.path.as_os_str())
                .to_os_string();
            file_name.push(format!(".tmp-{}-{sequence}", std::process::id()));
            let temporary_path = self.path.with_file_name(file_name);
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
            }
            match options.open(&temporary_path) {
                Ok(file) => return Ok((temporary_path, file)),
                Err(source) if source.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(source) => {
                    return Err(SearchError::Io {
                        path: temporary_path,
                        source,
                    });
                }
            }
        }
        Err(SearchError::Io {
            path: self.path.clone(),
            source: io::Error::new(
                io::ErrorKind::AlreadyExists,
                "could not reserve a search path configuration temporary file",
            ),
        })
    }
}

fn sidecar_too_large(actual_bytes: usize) -> SearchError {
    SearchError::PayloadTooLarge {
        boundary: "search path configuration sidecar",
        actual_bytes,
        max_bytes: SEARCH_PATH_SIDECAR_MAX_BYTES as usize,
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt;

    use tempfile::tempdir;

    use super::*;
    use crate::SearchPathPreferences;

    #[test]
    fn search_path_file_lives_in_the_bennu_config_directory() {
        assert_eq!(
            search_path_file_in(Path::new("/home/user/.config")),
            PathBuf::from("/home/user/.config/bennu/search-paths.json")
        );
    }

    #[test]
    fn fallback_reads_the_same_preferences_the_service_stores() {
        let config_directory = tempdir().unwrap();
        assert_eq!(
            load_search_path_preferences(config_directory.path()).unwrap(),
            SearchPathPreferences::default()
        );

        let preferences = SearchPathPreferences {
            custom_roots: Vec::new(),
            exclusions: vec![PathBuf::from("/home/user/private")],
        };
        SearchPathStore::at(search_path_file_in(config_directory.path()))
            .replace(&VersionedSearchPathPreferences {
                revision: 3,
                preferences: preferences.clone(),
            })
            .unwrap();
        assert_eq!(
            load_search_path_preferences(config_directory.path()).unwrap(),
            preferences
        );

        fs::write(search_path_file_in(config_directory.path()), b"{").unwrap();
        assert!(load_search_path_preferences(config_directory.path()).is_err());
    }

    #[test]
    fn missing_sidecar_is_distinct_from_a_damaged_sidecar() {
        let directory = tempdir().unwrap();
        let store = SearchPathStore::at(directory.path().join("search-paths.json"));
        assert_eq!(store.load().unwrap(), None);

        fs::write(&store.path, b"not-json").unwrap();
        assert!(store.load().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn sidecar_round_trip_preserves_non_utf8_paths_and_mode() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().unwrap();
        let store = SearchPathStore::at(directory.path().join("search-paths.json"));
        let versioned = VersionedSearchPathPreferences {
            revision: 7,
            preferences: SearchPathPreferences {
                custom_roots: vec![PathBuf::from(OsString::from_vec(
                    b"/mnt/root-\x80".to_vec(),
                ))],
                exclusions: Vec::new(),
            },
        };

        store.replace(&versioned).unwrap();

        assert_eq!(store.load().unwrap(), Some(versioned));
        assert_eq!(
            fs::metadata(&store.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn sidecar_reader_does_not_follow_symlinks() {
        use std::os::unix::fs::symlink;

        let directory = tempdir().unwrap();
        let target = directory.path().join("target.json");
        fs::write(&target, b"{}").unwrap();
        let store = SearchPathStore::at(directory.path().join("search-paths.json"));
        symlink(&target, &store.path).unwrap();

        assert!(store.load().is_err());
    }

    #[test]
    fn failed_replacement_preserves_the_previous_file() {
        let directory = tempdir().unwrap();
        let store = SearchPathStore::at(directory.path().join("search-paths.json"));
        let original = VersionedSearchPathPreferences {
            revision: 1,
            preferences: SearchPathPreferences::default(),
        };
        store.replace(&original).unwrap();
        let previous_bytes = fs::read(&store.path).unwrap();

        let blocked_parent = directory.path().join("blocked");
        fs::write(&blocked_parent, b"not-a-directory").unwrap();
        let blocked_store = SearchPathStore::at(blocked_parent.join("search-paths.json"));
        assert!(blocked_store
            .replace(&VersionedSearchPathPreferences {
                revision: 2,
                preferences: SearchPathPreferences::default(),
            })
            .is_err());

        assert_eq!(fs::read(&store.path).unwrap(), previous_bytes);
    }
}
