use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};
#[cfg(windows)]
use std::os::windows::ffi::{OsStrExt, OsStringExt};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "encoding", rename_all = "snake_case")]
pub enum StoredPath {
    #[cfg(unix)]
    UnixBytes {
        bytes: Vec<u8>,
    },
    #[cfg(windows)]
    WindowsWide {
        units: Vec<u16>,
    },
    Utf8 {
        value: String,
    },
}

impl StoredPath {
    pub fn from_path(path: &Path) -> Self {
        #[cfg(unix)]
        {
            Self::UnixBytes {
                bytes: path.as_os_str().as_bytes().to_vec(),
            }
        }

        #[cfg(windows)]
        {
            return Self::WindowsWide {
                units: path.as_os_str().encode_wide().collect(),
            };
        }

        #[cfg(not(any(unix, windows)))]
        {
            Self::Utf8 {
                value: path.to_string_lossy().into_owned(),
            }
        }
    }

    pub fn to_path_buf(&self) -> PathBuf {
        match self {
            #[cfg(unix)]
            Self::UnixBytes { bytes } => PathBuf::from(OsString::from_vec(bytes.clone())),
            #[cfg(windows)]
            Self::WindowsWide { units } => PathBuf::from(OsString::from_wide(units)),
            Self::Utf8 { value } => PathBuf::from(value),
        }
    }
}

/// 文件名的持久化形态：与 `StoredPath` 一样按字节保存，非 UTF-8 名字不丢字节。
/// 旧版本把重命名的新名字存成纯字符串，读取时两种形态都接受；写入只用字节编码。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct StoredFileName(StoredPath);

impl StoredFileName {
    pub fn from_os_str(name: &std::ffi::OsStr) -> Self {
        Self(StoredPath::from_path(Path::new(name)))
    }

    pub fn to_os_string(&self) -> OsString {
        self.0.to_path_buf().into_os_string()
    }
}

impl<'de> Deserialize<'de> for StoredFileName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum StoredFileNameForm {
            Encoded(StoredPath),
            LegacyText(String),
        }

        Ok(match StoredFileNameForm::deserialize(deserializer)? {
            StoredFileNameForm::Encoded(path) => Self(path),
            StoredFileNameForm::LegacyText(value) => Self(StoredPath::Utf8 { value }),
        })
    }
}
