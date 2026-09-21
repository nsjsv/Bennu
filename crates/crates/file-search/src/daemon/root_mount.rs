use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use file_core::mount_table::{parse_mount_table, PROC_SELF_MOUNTINFO};

use crate::database::SearchRootMount;
use crate::error::{SearchError, SearchResult};

pub(super) fn observe_root_mounts(roots: &[PathBuf]) -> SearchResult<Vec<SearchRootMount>> {
    let mount_points = read_mount_points()?;
    roots
        .iter()
        .map(|root| observe_root_mount(root, &mount_points))
        .collect()
}

fn read_mount_points() -> SearchResult<Vec<PathBuf>> {
    let bytes = fs::read(PROC_SELF_MOUNTINFO).map_err(|source| SearchError::Io {
        path: PathBuf::from(PROC_SELF_MOUNTINFO),
        source,
    })?;
    mount_points_from(&bytes)
}

fn mount_points_from(content: &[u8]) -> SearchResult<Vec<PathBuf>> {
    let snapshot = parse_mount_table(content);
    if let Some(warning) = snapshot.warnings.first() {
        return Err(SearchError::InvalidConfiguration(format!(
            "invalid mount table at line {}: {}",
            warning.line_number, warning.message
        )));
    }
    if snapshot.mounts.is_empty() {
        return Err(SearchError::InvalidConfiguration(
            "could not resolve any mount points from /proc/self/mountinfo".to_owned(),
        ));
    }
    Ok(snapshot
        .mounts
        .into_iter()
        .map(|mount| mount.mount_point)
        .collect())
}

fn observe_root_mount(root: &Path, mount_points: &[PathBuf]) -> SearchResult<SearchRootMount> {
    let mount_point = mount_points
        .iter()
        .filter(|mount_point| root.starts_with(mount_point))
        .max_by_key(|mount_point| mount_point.components().count())
        .cloned()
        .ok_or_else(|| {
            SearchError::InvalidConfiguration(format!(
                "could not resolve mount point for {}",
                root.display()
            ))
        })?;
    let device = fs::metadata(&mount_point)
        .map_err(|source| SearchError::Io {
            path: mount_point.clone(),
            source,
        })?
        .dev();
    Ok(SearchRootMount {
        root_path: root.to_path_buf(),
        mount_point,
        device,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_partially_malformed_mount_table() {
        let error = mount_points_from(
            b"broken\n42 35 8:1 / /media/My\\040Disk rw,nosuid - ext4 /dev/sda1 rw\n",
        )
        .unwrap_err();
        assert!(error.to_string().contains("line 1"));
    }

    #[test]
    fn selects_the_deepest_containing_mount() {
        let directory = tempfile::tempdir().unwrap();
        let nested = directory.path().join("nested");
        fs::create_dir(&nested).unwrap();
        let observed = observe_root_mount(
            &nested.join("missing"),
            &[PathBuf::from("/"), directory.path().to_path_buf()],
        )
        .unwrap();
        assert_eq!(observed.mount_point, directory.path());
    }
}
