//! 测试用的 `EntryStepPacer`：在指定的一步之前返回取消，并记录上报的进度。

use std::future::Future;

use super::entry_gathering::EntryStepPacer;
use crate::FileError;

/// 第 `cancel_before` 步（从 0 数）开始前返回取消；记录每次上报的进度。
#[derive(Default)]
pub(super) struct ScriptedPacer {
    steps_started: usize,
    cancel_before: Option<usize>,
    pub(super) progress: Vec<(usize, usize)>,
}

impl ScriptedPacer {
    pub(super) fn cancelling_before(step: usize) -> Self {
        Self {
            cancel_before: Some(step),
            ..Self::default()
        }
    }
}

impl EntryStepPacer for ScriptedPacer {
    fn before_step(&mut self) -> impl Future<Output = Result<(), FileError>> + Send {
        let cancel = self.cancel_before == Some(self.steps_started);
        self.steps_started += 1;
        async move {
            if cancel {
                Err(FileError::Cancelled)
            } else {
                Ok(())
            }
        }
    }

    fn step_finished(&mut self, completed: usize, total: usize) -> impl Future<Output = ()> + Send {
        self.progress.push((completed, total));
        async {}
    }
}
