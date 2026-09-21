use std::collections::HashSet;
use std::path::{Path, PathBuf};

const CONSERVATIVE_PATH_ALLOCATION_BYTES: usize = 192;

#[derive(Debug)]
pub(super) struct BoundedPathSet {
    paths: HashSet<Box<Path>>,
    estimated_bytes: usize,
    max_entries: usize,
    max_estimated_bytes: usize,
}

impl BoundedPathSet {
    pub(super) fn new(max_entries: usize, max_estimated_bytes: usize) -> Self {
        Self {
            paths: HashSet::new(),
            estimated_bytes: 0,
            max_entries,
            max_estimated_bytes,
        }
    }

    pub(super) fn insert(&mut self, path: PathBuf) -> Result<(), ()> {
        if self.paths.contains(path.as_path()) {
            return Ok(());
        }

        let path_estimated_bytes = estimated_path_bytes(&path);
        let next_estimated_bytes = self.estimated_bytes.saturating_add(path_estimated_bytes);
        if self.paths.len() >= self.max_entries || next_estimated_bytes > self.max_estimated_bytes {
            return Err(());
        }

        self.paths.insert(path.into_boxed_path());
        self.estimated_bytes = next_estimated_bytes;
        Ok(())
    }

    pub(super) fn clear(&mut self) {
        self.paths.clear();
        self.estimated_bytes = 0;
    }

    pub(super) fn contains(&self, path: &Path) -> bool {
        self.paths.contains(path)
    }

    pub(super) fn remove(&mut self, path: &Path) -> bool {
        if !self.paths.remove(path) {
            return false;
        }
        self.estimated_bytes = self
            .estimated_bytes
            .saturating_sub(estimated_path_bytes(path));
        true
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = &Path> {
        self.paths.iter().map(Box::as_ref)
    }

    pub(super) fn retain(&mut self, mut keep: impl FnMut(&Path) -> bool) {
        let removed = self
            .paths
            .iter()
            .filter(|path| !keep(path))
            .map(|path| path.to_path_buf())
            .collect::<Vec<_>>();
        for path in removed {
            self.remove(&path);
        }
    }

    pub(super) fn take_paths(&mut self) -> Vec<PathBuf> {
        self.estimated_bytes = 0;
        std::mem::take(&mut self.paths)
            .into_iter()
            .map(PathBuf::from)
            .collect()
    }

    pub(super) fn len(&self) -> usize {
        self.paths.len()
    }

    pub(super) fn estimated_bytes(&self) -> usize {
        self.estimated_bytes
    }
}

pub(super) fn estimated_path_bytes(path: &Path) -> usize {
    path.as_os_str()
        .as_encoded_bytes()
        .len()
        .saturating_add(CONSERVATIVE_PATH_ALLOCATION_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_paths_do_not_consume_capacity_twice() {
        let path = PathBuf::from("/tmp/repeated");
        let mut paths = BoundedPathSet::new(1, estimated_path_bytes(&path));

        paths.insert(path.clone()).unwrap();
        paths.insert(path).unwrap();

        assert_eq!(paths.len(), 1);
    }

    #[test]
    fn count_and_byte_limits_reject_without_growing_state() {
        let first = PathBuf::from("/tmp/first");
        let second = PathBuf::from("/tmp/second");
        let mut count_limited = BoundedPathSet::new(1, usize::MAX);
        count_limited.insert(first.clone()).unwrap();
        assert!(count_limited.insert(second.clone()).is_err());
        assert_eq!(count_limited.len(), 1);

        let mut byte_limited = BoundedPathSet::new(2, estimated_path_bytes(&first));
        byte_limited.insert(first).unwrap();
        assert!(byte_limited.insert(second).is_err());
        assert_eq!(byte_limited.len(), 1);
    }

    #[test]
    fn observed_home_scale_fits_the_realtime_path_budget() {
        let mut paths = BoundedPathSet::new(96_000, 32_000_000);
        for position in 0..78_710 {
            paths
                .insert(PathBuf::from(format!(
                    "/home/yuanming/work/project-{position:05}/target/debug/build/dependency-{position:05}/out/generated"
                )))
                .unwrap();
        }

        assert_eq!(paths.len(), 78_710);
        assert!(paths.estimated_bytes() <= 32_000_000);
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_paths_keep_raw_identity() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let first = PathBuf::from(OsString::from_vec(b"/tmp/\x80".to_vec()));
        let second = PathBuf::from(OsString::from_vec(b"/tmp/\x81".to_vec()));
        let mut paths = BoundedPathSet::new(2, usize::MAX);

        paths.insert(first.clone()).unwrap();
        paths.insert(second.clone()).unwrap();

        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&first));
        assert!(paths.contains(&second));
        assert!(paths.remove(&first));
        assert!(!paths.contains(&first));
        assert!(paths.contains(&second));
    }

    #[test]
    fn removal_releases_the_exact_estimated_capacity() {
        let first = PathBuf::from("/tmp/first");
        let second = PathBuf::from("/tmp/second");
        let capacity = estimated_path_bytes(&first).max(estimated_path_bytes(&second));
        let mut paths = BoundedPathSet::new(1, capacity);

        paths.insert(first.clone()).unwrap();
        assert!(paths.contains(&first));
        assert!(paths.remove(&first));
        paths.insert(second.clone()).unwrap();

        assert!(!paths.contains(&first));
        assert!(paths.contains(&second));
    }
}
