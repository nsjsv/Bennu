use std::sync::Arc;

use crate::recent_files::RecentFiles;
use crate::result_snapshots::ResultSnapshots;

/// 所有搜索连接共享的查询状态。界面每翻一页都重新连接服务，所以它挂在 socket 服务上，
/// 而不是挂在连接上。
#[derive(Default)]
pub(crate) struct QueryState {
    pub(crate) result_snapshots: Arc<ResultSnapshots>,
    pub(crate) recent_files: RecentFiles,
}
