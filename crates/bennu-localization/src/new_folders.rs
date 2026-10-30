use crate::UiLanguage;

/// 撤销收纳后没能删掉的新建目录：路径与删不掉的原因。
pub struct KeptFolderText {
    pub path: String,
    pub reason: String,
}

pub(super) fn kept_folders_warning(language: UiLanguage, kept: &[KeptFolderText]) -> String {
    match language {
        UiLanguage::English => {
            let items = kept
                .iter()
                .map(|folder| format!("{} ({})", folder.path, folder.reason))
                .collect::<Vec<_>>()
                .join("; ");
            format!("Items were moved back, but these new folders could not be removed: {items}")
        }
        UiLanguage::Chinese => {
            let items = kept
                .iter()
                .map(|folder| format!("{}（{}）", folder.path, folder.reason))
                .collect::<Vec<_>>()
                .join("；");
            format!("条目已移回原处，但以下新建文件夹没能删除：{items}")
        }
    }
}
