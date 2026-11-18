//! 正在补的文件信息：按（目录, 要补哪种信息）分组记条目下标，同一条不重复请求。
//!
//! 原来每个下标一个键、每个键各拷一份目录路径。按大小排序、状态栏总大小都要补整个目录，
//! 10 万个条目就是 10 万份路径，请求落地时再逐个拷一遍去删。

use std::collections::{HashMap, HashSet};

use file_core::DirectoryMetadataRequirement;

use crate::model::{BrowserPaneId, DirectoryMetadataLoadContext};

#[derive(Debug, Default)]
pub(super) struct DirectoryMetadataInFlight {
    targets: HashMap<(DirectoryMetadataLoadContext, DirectoryMetadataRequirement), HashSet<usize>>,
}

impl DirectoryMetadataInFlight {
    /// 记下要补的下标，返回之前没在补的那些（同一批里重复的只留一个）。
    pub(super) fn start(
        &mut self,
        context: &DirectoryMetadataLoadContext,
        requirement: DirectoryMetadataRequirement,
        indices: impl IntoIterator<Item = usize>,
    ) -> Vec<usize> {
        let key = (context.clone(), requirement);
        let in_flight = self.targets.entry(key.clone()).or_default();
        let started = indices
            .into_iter()
            .filter(|index| in_flight.insert(*index))
            .collect();
        if in_flight.is_empty() {
            self.targets.remove(&key);
        }
        started
    }

    pub(super) fn finish(
        &mut self,
        context: &DirectoryMetadataLoadContext,
        requirement: DirectoryMetadataRequirement,
        indices: &[usize],
    ) {
        let key = (context.clone(), requirement);
        let Some(in_flight) = self.targets.get_mut(&key) else {
            return;
        };
        for index in indices {
            in_flight.remove(index);
        }
        if in_flight.is_empty() {
            self.targets.remove(&key);
        }
    }

    pub(super) fn forget_pane(&mut self, pane_id: BrowserPaneId) {
        self.targets
            .retain(|(context, _), _| context.pane_id() != pane_id);
    }

    pub(super) fn clear(&mut self) {
        self.targets.clear();
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.targets.values().map(HashSet::len).sum()
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    #[cfg(test)]
    pub(super) fn iter(&self) -> impl Iterator<Item = InFlightTarget<'_>> {
        self.targets
            .iter()
            .flat_map(|((context, requirement), indices)| {
                indices.iter().map(move |index| InFlightTarget {
                    context,
                    requirement: *requirement,
                    index: *index,
                })
            })
    }
}

/// 一个正在补的条目（测试检查用）。
#[cfg(test)]
pub(super) struct InFlightTarget<'a> {
    pub(super) context: &'a DirectoryMetadataLoadContext,
    pub(super) requirement: DirectoryMetadataRequirement,
    pub(super) index: usize,
}
