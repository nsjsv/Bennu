//! 条目不覆盖地改名或移动：多步操作失败时回滚到操作前，回不去的条目逐个报告。

use std::path::PathBuf;

use crate::FileError;

/// 回滚没能把条目放回原处：原位置、条目此刻实际所在位置、放不回去的原因。
/// 用户要靠这三项自己找回文件，所以回滚的每一步失败都必须落到这里。
#[derive(Debug)]
pub struct UnrestoredEntry {
    pub original: PathBuf,
    pub current: PathBuf,
    pub error: FileError,
}
