use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serializer};

pub fn serialize<S>(path: &Path, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        serializer.serialize_bytes(path.as_os_str().as_bytes())
    }
    #[cfg(not(unix))]
    {
        serializer.serialize_str(&path.to_string_lossy())
    }
}

pub fn deserialize<'de, D>(deserializer: D) -> Result<PathBuf, D::Error>
where
    D: Deserializer<'de>,
{
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let bytes = Vec::<u8>::deserialize(deserializer)?;
        Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
    }
    #[cfg(not(unix))]
    {
        String::deserialize(deserializer).map(PathBuf::from)
    }
}

pub mod optional {
    use super::*;

    pub fn serialize<S>(path: &Option<PathBuf>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            match path {
                Some(path) => serializer.serialize_some(path.as_os_str().as_bytes()),
                None => serializer.serialize_none(),
            }
        }
        #[cfg(not(unix))]
        {
            match path {
                Some(path) => serializer.serialize_some(&path.to_string_lossy()),
                None => serializer.serialize_none(),
            }
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<PathBuf>, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            Option::<Vec<u8>>::deserialize(deserializer)
                .map(|bytes| bytes.map(std::ffi::OsString::from_vec).map(PathBuf::from))
        }
        #[cfg(not(unix))]
        {
            Option::<String>::deserialize(deserializer).map(|path| path.map(PathBuf::from))
        }
    }
}
