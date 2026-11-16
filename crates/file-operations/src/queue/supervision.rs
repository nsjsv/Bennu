//! 驱动者监督:不变量是"非终态 journal 记录在租约存活期内必须有且仅有一个
//! 活驱动者"。驱动者现在是界面侧的 `Task::stream`(app-ui 的 `file_operation_driver_task`),它的
//! 死亡不再被框架记账,只能由本模块的信号超时发现;发现后换代重启,新驱动者
//! 从 journal 断点续跑。跨实例的同类问题(其他进程退出后遗留的非终态任务)
//! 由任务锁本身叫醒:拿不到锁就阻塞等它被放手,持有进程退出时内核放锁,
//! 醒来后复用启动恢复路径按锁仲裁认领。全程没有定时扫描。

use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use super::*;

/// Running 任务的驱动者超过该时长没有任何生命信号即判定死亡。
/// 取值须宽容于最长静默阶段(目录指纹校验等纯元数据工作),
/// 误判的代价只是重复一次幂等的断点校验,漏判的代价是任务永久挂起。
pub const DRIVER_SIGNAL_TIMEOUT: Duration = Duration::from_secs(60);
/// 监督 tick 周期;只在 [`FileOperationQueue::needs_driver_supervision`] 时订阅。
pub const SUPERVISION_INTERVAL: Duration = Duration::from_secs(5);

/// 一条被别的进程拿着锁的可恢复任务:等它放手再认领。
///
/// `generation` 每次重新登记都变,订阅身份随之变化:醒过一次的旧等待不会再醒,
/// 重新登记(比如醒来后锁又被第三个进程抢先)必须换一个新的等待。
#[derive(Debug, Clone)]
pub struct ForeignTaskWait {
    store: TaskQueueStore,
    pub stored_task_id: u64,
    pub generation: u64,
}

impl Hash for ForeignTaskWait {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.store.db_path().hash(state);
        self.stored_task_id.hash(state);
        self.generation.hash(state);
    }
}

impl ForeignTaskWait {
    /// 阻塞到持锁进程放手;调用方要放在不会拖住退出的线程上。
    pub fn wait_for_release(&self) -> Result<(), String> {
        self.store
            .wait_for_recoverable_task_runner_release(self.stored_task_id)
            .map_err(storage_error)
    }
}

impl FileOperationQueue {
    /// update 出口统一签发驱动者:第一个 Running/Canceling 且尚未持有驱动者的任务。
    /// 恢复路径重建的 Canceling 任务没有驱动者,取消协议只能靠驱动者收敛到终态,
    /// 不签发就会永远停在取消中并经 active_subscription 堵死整个队列。
    /// 序列性由 start_next 保证(同一时刻至多一个任务处于非终态推进中)。
    pub fn poll_driver_launch(&mut self) -> Option<RunningFileOperation> {
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
    pub fn note_driver_signal(&mut self, task_id: u64) {
        if let Some(task) = self.tasks.iter_mut().find(|task| task.id == task_id) {
            task.last_driver_signal = Some(Instant::now());
        }
    }

    /// 过期驱动者的终态消息必须被拒,否则会终结已被换代重启的任务。
    pub fn driver_generation_is_current(&self, task_id: u64, generation: u64) -> bool {
        self.tasks
            .iter()
            .find(|task| task.id == task_id)
            .is_some_and(|task| task.driver_generation == generation)
    }

    /// 有驱动者需要监督时才订阅 tick;空闲时没有任何定时器。
    pub fn needs_driver_supervision(&self) -> bool {
        self.tasks.iter().any(driver_is_supervised)
    }

    /// Running/Canceling 任务信号超时 → 判定驱动者死亡,换代重启。仅恢复式(journal)
    /// 任务可安全重放;瞬时任务(重命名/回收站等)不可重放,不重启。
    pub fn supervise_drivers(&mut self, now: Instant) {
        let stale = self.tasks.iter().position(|task| {
            driver_is_supervised(task)
                && task
                    .last_driver_signal
                    .is_some_and(|signal| now.duration_since(signal) >= DRIVER_SIGNAL_TIMEOUT)
        });
        let Some(position) = stale else {
            return;
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
            operation = %task.operation.title(),
            "driver showed no signal; restarting from recovery journal"
        );
    }

    /// 当前要等的外来任务;每条对应界面侧一个阻塞等待。
    pub fn foreign_task_waits(&self) -> Vec<ForeignTaskWait> {
        let Some(store) = &self.store else {
            return Vec::new();
        };
        self.foreign_tasks
            .iter()
            .map(|(&stored_task_id, &generation)| ForeignTaskWait {
                store: store.clone(),
                stored_task_id,
                generation,
            })
            .collect()
    }

    /// 任务库有写入(或刚开始监视):找新出现的、别的进程留下或正拿着的可恢复任务。
    ///
    /// 库被写得很频繁(本进程的进度、会话也写这里),所以先只读未终态任务的 id,
    /// 只有不认识的那几条才读内容。只认可恢复任务:它们有锁,锁能证明持有进程是否
    /// 还活着。瞬时任务没有锁,在这里看到未终态时可能正被另一个活进程执行,
    /// 不能当遗留判失败(那只在启动恢复时做)。
    pub fn adopt_foreign_tasks(&mut self) -> Option<String> {
        let store = self.store.clone()?;
        let unfinished = match store.read_unfinished_task_ids() {
            Ok(unfinished) => unfinished,
            Err(error) => return Some(storage_error(error)),
        };
        // 持有进程已把任务做完(或删掉)的,不再等。
        self.foreign_tasks.retain(|id, _| unfinished.contains(id));
        let mut errors = None;
        for id in unfinished {
            let known = self.foreign_tasks.contains_key(&id)
                || self.tasks.iter().any(|task| task.stored_id == Some(id));
            if known {
                continue;
            }
            match store.read_task(id) {
                Ok(Some(stored_task)) if stored_operation_is_resumable(&stored_task.operation) => {
                    errors = combine_storage_errors(errors, self.restore_stored_task(stored_task));
                }
                Ok(_) => {}
                Err(error) => {
                    errors = combine_storage_errors(errors, Some(storage_error(error)));
                }
            }
        }
        self.start_next();
        errors
    }

    /// 等待醒来:持锁进程放手了。认领仍按锁仲裁,被第三个进程抢先时会重新登记等待。
    pub fn accept_foreign_task_release(
        &mut self,
        stored_task_id: u64,
        generation: u64,
    ) -> Option<String> {
        if self.foreign_tasks.get(&stored_task_id) != Some(&generation) {
            return None;
        }
        self.foreign_tasks.remove(&stored_task_id);
        let store = self.store.clone()?;
        let stored_task = match store.read_task(stored_task_id) {
            Ok(stored_task) => stored_task?,
            Err(error) => return Some(storage_error(error)),
        };
        let error = self.restore_stored_task(stored_task);
        self.start_next();
        error
    }

    pub(super) fn wait_for_foreign_task(&mut self, stored_task_id: u64) {
        self.next_foreign_wait_generation += 1;
        self.foreign_tasks
            .insert(stored_task_id, self.next_foreign_wait_generation);
    }
}

fn driver_is_supervised(task: &FileOperationTask) -> bool {
    matches!(
        task.status,
        FileOperationStatus::Running | FileOperationStatus::Canceling
    ) && task.driver_running
        && task.operation.uses_recovery_journal()
}
