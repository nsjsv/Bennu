use std::collections::HashMap;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct GvfsMountIdentity {
    pub(super) scheme: String,
    pub(super) host: String,
}

#[derive(Debug, Error)]
pub(super) enum GvfsMountPathError {
    #[error("invalid GVfs URI {uri:?}: {message}")]
    InvalidUri { uri: String, message: String },
    #[error("GVfs FUSE root is unavailable: {root:?}")]
    FuseRootUnavailable { root: PathBuf },
    #[error("GVfs mount path was not uniquely found for {uri:?} under {root:?}: {reason}")]
    MountPathUnavailable {
        uri: String,
        root: PathBuf,
        reason: &'static str,
    },
}

pub(super) fn default_gvfs_fuse_root() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("UID").map(|uid| PathBuf::from("/run/user").join(uid)))
        .unwrap_or_else(|| PathBuf::from("/run/user/0"))
        .join("gvfs")
}

pub(super) fn resolve_gvfs_mount_path(
    root_uri: &str,
    default_location_uri: &str,
    fuse_root: &Path,
) -> Result<PathBuf, GvfsMountPathError> {
    let root_identity = gvfs_mount_identity(root_uri)?;
    let default_identity = gvfs_mount_identity(default_location_uri)?;
    if root_identity != default_identity {
        return Err(GvfsMountPathError::InvalidUri {
            uri: default_location_uri.to_owned(),
            message: "default location belongs to a different GVfs mount".to_owned(),
        });
    }
    if !fuse_root.is_dir() {
        return Err(GvfsMountPathError::FuseRootUnavailable {
            root: fuse_root.to_path_buf(),
        });
    }

    let mut matching_paths = Vec::new();
    for entry in
        std::fs::read_dir(fuse_root).map_err(|_| GvfsMountPathError::FuseRootUnavailable {
            root: fuse_root.to_path_buf(),
        })?
    {
        let entry = entry.map_err(|_| GvfsMountPathError::MountPathUnavailable {
            uri: root_uri.to_owned(),
            root: fuse_root.to_path_buf(),
            reason: "could not read GVfs FUSE entry",
        })?;
        let Some(name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
            continue;
        };
        if gvfs_mount_directory_identity(&name).as_ref() == Some(&root_identity) {
            matching_paths.push(entry.path());
        }
    }

    let mount_path = match matching_paths.as_slice() {
        [path] => path.clone(),
        [] => {
            return Err(GvfsMountPathError::MountPathUnavailable {
                uri: root_uri.to_owned(),
                root: fuse_root.to_path_buf(),
                reason: "no matching mount directory",
            })
        }
        _ => {
            return Err(GvfsMountPathError::MountPathUnavailable {
                uri: root_uri.to_owned(),
                root: fuse_root.to_path_buf(),
                reason: "multiple matching mount directories",
            })
        }
    };

    let root_path = uri_path_segments(root_uri)?;
    let default_path = uri_path_segments(default_location_uri)?;
    let relative_segments = default_path
        .strip_prefix(root_path.as_slice())
        .ok_or_else(|| GvfsMountPathError::InvalidUri {
            uri: default_location_uri.to_owned(),
            message: "default location is outside the mount root".to_owned(),
        })?;
    Ok(relative_segments
        .iter()
        .fold(mount_path, |path, segment| path.join(segment)))
}

pub(super) fn gvfs_mount_identity(uri: &str) -> Result<GvfsMountIdentity, GvfsMountPathError> {
    parse_gvfs_uri(uri).map(|(identity, _)| identity)
}

fn gvfs_mount_directory_identity(name: &str) -> Option<GvfsMountIdentity> {
    let (scheme, values) = name.split_once(':')?;
    let values = parse_gvfs_mount_values(values);
    Some(GvfsMountIdentity {
        scheme: scheme.to_ascii_lowercase(),
        host: values.get("host")?.clone(),
    })
}

fn parse_gvfs_mount_values(value: &str) -> HashMap<String, String> {
    value
        .split(',')
        .filter_map(|item| item.split_once('='))
        .map(|(key, value)| (key.to_owned(), percent_decode(value)))
        .collect()
}

fn uri_path_segments(uri: &str) -> Result<Vec<OsString>, GvfsMountPathError> {
    parse_gvfs_uri(uri).map(|(_, segments)| segments)
}

fn parse_gvfs_uri(uri: &str) -> Result<(GvfsMountIdentity, Vec<OsString>), GvfsMountPathError> {
    let (scheme, remainder) =
        uri.trim()
            .split_once("://")
            .ok_or_else(|| GvfsMountPathError::InvalidUri {
                uri: uri.to_owned(),
                message: "GVfs mount URI has no scheme".to_owned(),
            })?;
    let (authority, path) = remainder.split_once('/').unwrap_or((remainder, ""));
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = percent_decode(authority);
    if host.is_empty() {
        return Err(GvfsMountPathError::InvalidUri {
            uri: uri.to_owned(),
            message: "GVfs mount URI has no host".to_owned(),
        });
    }
    let segments = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(|segment| OsString::from_vec(percent_decode_bytes(segment)))
        .collect();
    Ok((
        GvfsMountIdentity {
            scheme: scheme.to_ascii_lowercase(),
            host,
        },
        segments,
    ))
}

fn percent_decode(value: &str) -> String {
    String::from_utf8_lossy(&percent_decode_bytes(value)).into_owned()
}

fn percent_decode_bytes(value: &str) -> Vec<u8> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&value[index + 1..index + 3], 16) {
                decoded.push(byte);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    decoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn resolves_encoded_portable_mount_directory_and_default_child() {
        let root = tempdir().expect("temporary root");
        let mount = root.path().join("mtp:host=%5Busb%3A001%2C002%5D");
        std::fs::create_dir(&mount).expect("mount directory");

        let resolved = resolve_gvfs_mount_path(
            "mtp://[usb:001,002]/",
            "mtp://[usb:001,002]/DCIM/Camera",
            root.path(),
        )
        .expect("resolved mount");

        assert_eq!(resolved, mount.join("DCIM").join("Camera"));
    }

    #[test]
    fn preserves_non_utf8_default_location_segment_in_fuse_path() {
        let root = tempdir().expect("temporary root");
        let mount = root.path().join("mtp:host=phone");
        std::fs::create_dir(&mount).expect("mount directory");

        let resolved = resolve_gvfs_mount_path(
            "mtp://phone/storage/",
            "mtp://phone/storage/%80-photo",
            root.path(),
        )
        .expect("resolved mount");

        assert_eq!(
            resolved,
            mount.join(OsString::from_vec(b"\x80-photo".to_vec()))
        );
    }

    #[test]
    fn resolves_gphoto_and_afc_without_protocol_whitelist() {
        let root = tempdir().expect("temporary root");
        let gphoto = root.path().join("gphoto2:host=%5Busb%3A002%2C003%5D");
        let afc = root.path().join("afc:host=phone-serial");
        std::fs::create_dir(&gphoto).expect("gphoto mount directory");
        std::fs::create_dir(&afc).expect("afc mount directory");

        assert_eq!(
            resolve_gvfs_mount_path(
                "gphoto2://[usb:002,003]/",
                "gphoto2://[usb:002,003]/",
                root.path(),
            )
            .expect("gphoto path"),
            gphoto
        );
        assert_eq!(
            resolve_gvfs_mount_path("afc://phone-serial/", "afc://phone-serial/", root.path())
                .expect("afc path"),
            afc
        );
    }

    #[test]
    fn missing_fuse_root_is_a_structured_error() {
        let root = tempdir().expect("temporary root");
        let missing = root.path().join("missing");

        let error = resolve_gvfs_mount_path("mtp://phone/", "mtp://phone/", &missing)
            .expect_err("missing FUSE root");

        assert!(matches!(
            error,
            GvfsMountPathError::FuseRootUnavailable { root } if root == missing
        ));
    }

    #[test]
    fn no_matching_mount_directory_is_a_structured_error() {
        let root = tempdir().expect("temporary root");
        std::fs::create_dir(root.path().join("mtp:host=other-phone"))
            .expect("unrelated mount directory");

        let error = resolve_gvfs_mount_path("mtp://phone/", "mtp://phone/", root.path())
            .expect_err("missing mount directory");

        assert!(matches!(
            error,
            GvfsMountPathError::MountPathUnavailable {
                reason: "no matching mount directory",
                ..
            }
        ));
    }

    #[test]
    fn default_location_must_remain_inside_mount_root() {
        let root = tempdir().expect("temporary root");
        std::fs::create_dir(root.path().join("mtp:host=phone")).expect("mount directory");

        let error =
            resolve_gvfs_mount_path("mtp://phone/DCIM/", "mtp://phone/Documents/", root.path())
                .expect_err("default location outside mount root");

        assert!(matches!(error, GvfsMountPathError::InvalidUri { .. }));
    }

    #[test]
    fn rejects_multiple_matching_mount_directories() {
        let root = tempdir().expect("temporary root");
        std::fs::create_dir(root.path().join("mtp:host=phone")).expect("first mount");
        std::fs::create_dir(root.path().join("mtp:host=phone,volume=second"))
            .expect("second mount");

        let error = resolve_gvfs_mount_path("mtp://phone/", "mtp://phone/", root.path())
            .expect_err("ambiguous path");
        assert!(error.to_string().contains("multiple"));
    }
}
