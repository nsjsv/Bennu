use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::error::{SearchError, SearchResult};

pub const SEARCH_RUNTIME_IDENTITY_ENV: &str = "FILE_MANAGER_SEARCH_RUNTIME_IDENTITY";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SearchRuntimeIdentity {
    #[default]
    Release,
    Development,
}

impl SearchRuntimeIdentity {
    pub fn from_environment() -> SearchResult<Self> {
        Self::from_configured_value(std::env::var_os(SEARCH_RUNTIME_IDENTITY_ENV))
    }

    pub fn from_configured_value(configured_value: Option<OsString>) -> SearchResult<Self> {
        match configured_value {
            None => Ok(Self::Release),
            Some(configured_value) if configured_value == "development" => Ok(Self::Development),
            Some(_) => Err(SearchError::InvalidConfiguration(format!(
                "{SEARCH_RUNTIME_IDENTITY_ENV} must be unset for release or exactly 'development'"
            ))),
        }
    }

    pub const fn systemd_unit(self) -> &'static str {
        match self {
            Self::Release => "bennu-search.service",
            Self::Development => "bennu-search-dev.service",
        }
    }

    pub const fn socket_name(self) -> &'static str {
        match self {
            Self::Release => "bennu-search.sock",
            Self::Development => "bennu-search-dev.sock",
        }
    }

    pub fn socket_path_in(self, runtime_directory: &Path) -> PathBuf {
        runtime_directory.join(self.socket_name())
    }

    pub fn socket_path(self) -> PathBuf {
        let runtime_directory = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        self.socket_path_in(&runtime_directory)
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::{OsStr, OsString};
    use std::path::Path;

    use super::{SearchRuntimeIdentity, SEARCH_RUNTIME_IDENTITY_ENV};

    #[test]
    fn runtime_identity_defaults_to_release_and_accepts_development() {
        assert_eq!(
            SearchRuntimeIdentity::from_configured_value(None).unwrap(),
            SearchRuntimeIdentity::Release
        );
        assert_eq!(
            SearchRuntimeIdentity::from_configured_value(Some(OsString::from("development")))
                .unwrap(),
            SearchRuntimeIdentity::Development
        );
    }

    #[test]
    fn runtime_identity_rejects_every_other_configured_value() {
        let invalid_configuration =
            SearchRuntimeIdentity::from_configured_value(Some(OsString::from("release")))
                .unwrap_err()
                .to_string();

        assert!(invalid_configuration.contains(SEARCH_RUNTIME_IDENTITY_ENV));
        assert!(!invalid_configuration.contains("got"));
    }

    #[cfg(unix)]
    #[test]
    fn runtime_identity_rejects_non_utf8_configuration() {
        use std::os::unix::ffi::OsStringExt;

        assert!(
            SearchRuntimeIdentity::from_configured_value(Some(OsString::from_vec(vec![0xff])))
                .is_err()
        );
    }

    #[test]
    fn release_and_development_derive_disjoint_units_and_sockets() {
        let runtime_directory = Path::new("/run/user/1000");

        assert_eq!(
            SearchRuntimeIdentity::Release.systemd_unit(),
            "bennu-search.service"
        );
        assert_eq!(
            SearchRuntimeIdentity::Development.systemd_unit(),
            "bennu-search-dev.service"
        );
        assert_eq!(
            SearchRuntimeIdentity::Release.socket_path_in(runtime_directory),
            runtime_directory.join(OsStr::new("bennu-search.sock"))
        );
        assert_eq!(
            SearchRuntimeIdentity::Development.socket_path_in(runtime_directory),
            runtime_directory.join(OsStr::new("bennu-search-dev.sock"))
        );
    }
}
