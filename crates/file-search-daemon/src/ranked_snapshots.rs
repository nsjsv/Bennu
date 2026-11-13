use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::Notify;

use crate::model::SearchQuery;

/// 同时保留的定格份数，与服务同时接纳的客户端数一致。
const MAX_RANKED_SNAPSHOTS: usize = 8;
/// 这么久没被翻页就释放，服务空闲时不留着大结果集占内存。
const RANKED_SNAPSHOT_IDLE_LIFETIME: Duration = Duration::from_secs(5 * 60);

/// 定格结果中的一条命中：文件行身份与第一页时算出的相关度（越大越相关）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RankedRow {
    pub(crate) rowid: i64,
    pub(crate) rank: f64,
}

/// 按相关度排序的查询在第一页定格的全部命中顺序。界面每翻一页都重新连接服务，
/// 所以定格结果不挂在连接上，由所有连接共享，凭游标里的编号取回。
pub(crate) struct RankedSnapshots {
    snapshots: Mutex<Vec<RankedSnapshot>>,
    next_id: AtomicU64,
    inserted: Notify,
}

struct RankedSnapshot {
    id: u64,
    /// 去掉 query_id/limit/cursor 的查询。编号必须配同一个查询才算数，
    /// 防止编号碰撞或游标被拿去配别的查询时返回错的结果。
    query: SearchQuery,
    rows: Arc<[RankedRow]>,
    last_used: Instant,
}

impl Default for RankedSnapshots {
    fn default() -> Self {
        Self {
            snapshots: Mutex::new(Vec::new()),
            // 编号从每个进程随机的起点开始：服务重启后，界面手里的旧编号不会撞上新的定格。
            next_id: AtomicU64::new(RandomState::new().hash_one(std::process::id())),
            inserted: Notify::new(),
        }
    }
}

impl RankedSnapshots {
    pub(crate) fn insert(&self, query: &SearchQuery, rows: Arc<[RankedRow]>) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut snapshots = self.lock();
        if snapshots.len() == MAX_RANKED_SNAPSHOTS {
            let least_recently_used = snapshots
                .iter()
                .enumerate()
                .min_by_key(|(_, snapshot)| snapshot.last_used)
                .map(|(index, _)| index)
                .expect("a full snapshot list is not empty");
            snapshots.swap_remove(least_recently_used);
        }
        snapshots.push(RankedSnapshot {
            id,
            query: snapshot_identity(query),
            rows,
            last_used: Instant::now(),
        });
        drop(snapshots);
        self.inserted.notify_one();
        id
    }

    pub(crate) fn get(&self, id: u64, query: &SearchQuery) -> Option<Arc<[RankedRow]>> {
        let identity = snapshot_identity(query);
        let mut snapshots = self.lock();
        let snapshot = snapshots
            .iter_mut()
            .find(|snapshot| snapshot.id == id && snapshot.query == identity)?;
        snapshot.last_used = Instant::now();
        Some(Arc::clone(&snapshot.rows))
    }

    /// 释放到期的定格，返回剩下的最早到期时刻。
    fn release_expired(&self, now: Instant) -> Option<Instant> {
        let mut snapshots = self.lock();
        snapshots.retain(|snapshot| now < snapshot.last_used + RANKED_SNAPSHOT_IDLE_LIFETIME);
        snapshots
            .iter()
            .map(|snapshot| snapshot.last_used + RANKED_SNAPSHOT_IDLE_LIFETIME)
            .min()
    }

    /// 随服务运行的到期释放任务。没有定格时只等新的插入，不产生周期唤醒。
    pub(crate) async fn release_idle_snapshots(self: Arc<Self>) {
        loop {
            match self.release_expired(Instant::now()) {
                Some(next_expiry) => {
                    tokio::select! {
                        () = tokio::time::sleep_until(next_expiry.into()) => {}
                        () = self.inserted.notified() => {}
                    }
                }
                None => self.inserted.notified().await,
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<RankedSnapshot>> {
        self.snapshots
            .lock()
            .expect("ranked search snapshot mutex poisoned")
    }
}

fn snapshot_identity(query: &SearchQuery) -> SearchQuery {
    SearchQuery {
        query_id: 0,
        limit: 0,
        cursor: None,
        ..query.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(count: i64) -> Arc<[RankedRow]> {
        (0..count)
            .map(|rowid| RankedRow { rowid, rank: -1.0 })
            .collect()
    }

    #[test]
    fn a_snapshot_is_found_only_by_its_id_and_its_own_query() {
        let snapshots = RankedSnapshots::default();
        let query = SearchQuery::global(1, "needle");
        let id = snapshots.insert(&query, rows(3));

        let mut next_page = query.clone();
        next_page.query_id = 2;
        next_page.limit = 7;
        assert_eq!(snapshots.get(id, &next_page).unwrap().len(), 3);
        assert!(snapshots.get(id + 1, &query).is_none());
        assert!(snapshots
            .get(id, &SearchQuery::global(1, "other"))
            .is_none());
    }

    #[test]
    fn the_least_recently_used_snapshot_makes_room_for_a_new_one() {
        let snapshots = RankedSnapshots::default();
        let queries: Vec<_> = (0..=MAX_RANKED_SNAPSHOTS)
            .map(|index| SearchQuery::global(1, format!("needle-{index}")))
            .collect();
        let ids: Vec<_> = queries[..MAX_RANKED_SNAPSHOTS]
            .iter()
            .map(|query| snapshots.insert(query, rows(1)))
            .collect();
        // 第一份刚被翻过页，最久没用的变成第二份。
        assert!(snapshots.get(ids[0], &queries[0]).is_some());

        let newest = snapshots.insert(&queries[MAX_RANKED_SNAPSHOTS], rows(1));

        assert!(snapshots.get(ids[0], &queries[0]).is_some());
        assert!(snapshots.get(ids[1], &queries[1]).is_none());
        assert!(snapshots
            .get(newest, &queries[MAX_RANKED_SNAPSHOTS])
            .is_some());
    }

    #[test]
    fn idle_snapshots_are_released_after_their_lifetime() {
        let snapshots = RankedSnapshots::default();
        let query = SearchQuery::global(1, "needle");
        let id = snapshots.insert(&query, rows(1));
        let inserted_at = Instant::now();

        assert!(snapshots.release_expired(inserted_at).is_some());
        assert!(snapshots.get(id, &query).is_some());
        assert_eq!(
            snapshots.release_expired(Instant::now() + RANKED_SNAPSHOT_IDLE_LIFETIME),
            None
        );
        assert!(snapshots.get(id, &query).is_none());
    }
}
