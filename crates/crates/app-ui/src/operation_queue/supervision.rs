//! 驱动者监督:不变量是"非终态 journal 记录在租约存活期内必须有且仅有一个
//! 活驱动者"。驱动者现在是 `Task::stream`(见 queued_file_operations),它的
//! 死亡不再被框架记账,只能由本模块的信号超时发现;发现后换代重启,新驱动者
//! 从 journal 断点续跑。跨实例的同类问题(其他进程退出后遗留的非终态任务)
//! 由同一 tick 按 runner 租约认领,复用启动恢复路径。

use std::time::{Duration, Instant};

use super::*;

/// Running 任务的驱动者超过该时长没有任何生命信号即判定死亡。
/// 取值须宽容于最长静默阶段(目录指纹校验等纯元数据工作),
/// 误判的代价只是重复一次幂等的断点校验,漏判的代价是任务永久挂起。
pub(crate) const DRIVER_SIGNAL_TIMEOUT: Duration = Duration::from_secs(60);
/// 监督 tick 周期(app.rs 订阅按此频发 `OperationSupervisionTick`)。
pub(crate) const SUPERVISION_INTERVAL: Duration = Duration::from_secs(5);
/// 外来任务认领的 store 扫描周期(本地 sqlite 全表读,不必每 tick 都扫)。
const STORE_SCAN_INTERVAL: Duration = Duration::from_secs(30);

impl FileOperationQueue {
    /// update 出口统一签发驱动者:第一个 Running/Canceling 且尚未持有驱动者的任务。
    /// 恢复路径重建的 Canceling 任务没有驱动者,取消协议只能靠驱动者收敛到终态,
    /// 不签发就会永远停在取消中并经 active_subscription 堵死整个队列。
    /// 序列性由 start_next 保证(同一时刻至多一个任务处于非终态推进中)。
    pub(crate) fn poll_driver_launch(&mut self) -> Option<RunningFileOperation> {
        let position = self.tasks.iter().position(|task| {
            !task.driver_running
                && matches!(
                    task.status,
                    FileOperationStatus::Running | FileOperationStatus::Canceling
                )
                && (!task.operation.uses_recovery_journal() || task.stored_id.is_some())
        })?;
        let task = &mut self.tasks[position];
        task.driver_running = true;
        task.last_driver_signal = Some(Instant::now());
        Some(RunningFileOperation {
            id: task.id,
            stored_id: task.stored_id,
            operation: task.operation.clone(),
            controls: FileOperationControls::new(
                task.cancel.clone(),
                task.run_state_receiver.clone(),
            ),
            store: self.store.clone(),
            generation: task.driver_generation,
        })
    }

    /// 驱动者生命信号:驱动者发出的任何任务级消息都刷新计时。
    pub(crate) fn note_driver_signal(&mut self, task_id: u64) {
        if let Some(task) = self.tasks.iter_mut().find(|task| task.id == task_id) {
            task.last_driver_signal = Some(Instant::now());
        }
    }

    /// 过期驱动者的终态消息必须被拒,否则会终结已被换代重启的任务。
    pub(crate) fn driver_generation_is_current(&self, task_id: u64, generation: u64) -> bool {
        self.tasks
            .iter()
            .find(|task| task.id == task_id)
            .is_some_and(|task| task.driver_generation == generation)
    }

    /// 监督一个 tick:先做进程内静默重启,再做跨实例认领。
    pub(crate) fn supervise(&mut self, now: Instant) -> Option<String> {
        let storage_error = self.resume_silent_driver(now);
        combine_storage_errors(storage_error, self.adopt_abandoned_tasks(now))
    }

    /// Running/Canceling 任务信号超时 → 判定驱动者死亡,换代重启。仅恢复式(journal)
    /// 任务可安全重放;瞬时任务(重命名/回收站等)不可重放,不重启。
    fn resume_silent_driver(&mut self, now: Instant) -> Option<String> {
        let stale = self.tasks.iter().position(|task| {
            matches!(
                task.status,
                FileOperationStatus::Running | FileOperationStatus::Canceling
            ) && task.driver_running
                && task.operation.uses_recovery_journal()
                && task
                    .last_driver_signal
                    .is_some_and(|signal| now.duration_since(signal) >= DRIVER_SIGNAL_TIMEOUT)
        });
        let Some(position) = stale else {
            return None;
        };
        let task = &mut self.tasks[position];
        task.driver_generation += 1;
        task.driver_running = false;
        task.last_driver_signal = Some(now);
        // 旧驱动者可能只是假死仍在 I/O 中:作废旧 token,它会在下一个检查点
        // (checkpoint_now/wait_until_running)以 Cancelled 退出,防止双驱动。
        // 取消中任务换代不得丢失取消意图:新 token 保持已取消,重启的驱动者直接收敛取消。
        let was_canceling = task.status == FileOperationStatus::Canceling;
        task.cancel = CancellationToken::new();
        if was_canceling {
            task.cancel.cancel();
        }
        let (run_state_sender, run_state_receiver) = watch::channel(FileOperationRunState::Running);
        task.run_state_sender = run_state_sender;
        task.run_state_receiver = run_state_receiver;
        tracing::warn!(
            target: "app_ui::operation_supervision",
            task_id = task.id,
            generation = task.driver_generation,
            operation = task.operation.title(),
            "driver showed no signal; restarting from recovery journal"
        );
        None
    }

    /// 认领其他实例遗留的非终态任务:不在内存、租约空闲者,走启动恢复同一条
    /// restore_stored_task 路径(flock try_acquire 天然仲裁多实例竞争)。
    fn adopt_abandoned_tasks(&mut self, now: Instant) -> Option<String> {
        let Some(store) = self.store.clone() else {
            return None;
        };
        if self
            .next_store_scan
            .is_some_and(|scheduled| now < scheduled)
        {
            return None;
        }
        self.next_store_scan = Some(now + STORE_SCAN_INTERVAL);
        let stored_tasks = match store.read_tasks() {
            Ok(stored_tasks) => stored_tasks,
            Err(error) => return Some(storage_error(error)),
        };
        let mut storage_error = None;
        for stored_task in stored_tasks {
            let known = self
                .tasks
                .iter()
                .any(|task| task.stored_id == Some(stored_task.id));
            if known || stored_status_is_terminal(stored_task.status) {
                continue;
            }
            storage_error = combine_storage_errors(
                storage_error,
                self.restore_stored_task(stored_task.clone()),
            );
        }
        // 认领的任务推入为 Pending,无活动任务时推进到 Running,
        // 驱动者在本次 update 出口由 poll_driver_launch 统一签发。
        storage_error = combine_storage_errors(storage_error, self.start_next());
        storage_error
    }
}
