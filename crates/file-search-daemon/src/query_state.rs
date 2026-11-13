use std::sync::Arc;

use crate::ranked_snapshots::RankedSnapshots;
use crate::recent_files::RecentFiles;

/// 所有搜索连接共享的查询状态。界面每翻一页都重新连接服务，所以它挂在 socket 服务上，
/// 而不是挂在连接上。
#[derive(Default)]
pub(crate) struct QueryState {
    pub(crate) ranked_snapshots: Arc<RankedSnapshots>,
    pub(crate) recent_files: RecentFiles,
}
