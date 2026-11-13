use std::path::{Path, PathBuf};

/// 浏览窗口正在看的位置。回收站不是目录、没有路径：需要真实目录的代码只能经
/// [`BrowserLocation::directory`] 拿到，回收站在类型上拿不到伪路径。主程序与文件选择
/// 对话框共用这一个类型。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BrowserLocation {
    Directory(PathBuf),
    Trash,
}

impl BrowserLocation {
    pub fn directory(&self) -> Option<&Path> {
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
            BrowserLocation::Directory(PathBuf::from("/srv")).directory(),
            Some(Path::new("/srv"))
        );
        assert_eq!(BrowserLocation::Trash.directory(), None);
    }
}
