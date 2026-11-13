use std::path::{Path, PathBuf};

/// 选择窗口正在看的位置。回收站不是目录、没有路径：需要真实目录的代码只能经
/// [`PickerLocation::directory`] 拿到，回收站在类型上拿不到伪路径。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum PickerLocation {
    Directory(PathBuf),
    Trash,
}

impl PickerLocation {
    pub(crate) fn directory(&self) -> Option<&Path> {
        match self {
            Self::Directory(directory) => Some(directory),
            Self::Trash => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_directory_location_has_a_path() {
        assert_eq!(
            PickerLocation::Directory(PathBuf::from("/srv")).directory(),
            Some(Path::new("/srv"))
        );
        assert_eq!(PickerLocation::Trash.directory(), None);
    }
}
