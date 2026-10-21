//! 测试按正式版的顺序驱动持久化:取请求 → 执行 → 交回结果。
//!
//! 正式版由界面层取出请求(`continue_file_operation_persistence`),在后台线程执行,
//! 稍后经 `Message::FileOperationPersistenceFinished` 交回 `accept_persistence_outcome`。
//! 这里调用的是同样三个函数,只是两步之间不插入其它操作,等价于"持久化先于下一个
//! 用户动作完成"。需要观察"请求已排队、尚未完成"中间态的测试,直接手动调用这三步。

use file_operation_store::TaskQueueStore;

use super::{
    execute_file_operation_persistence, FileOperationEnqueueOutcome, FileOperationQueue,
    QueuedFileOperation,
};

/// 给队列装上存储:与启动时同一入口。新建的库没有可恢复任务,恢复不应报错。
pub(crate) fn attach_store(queue: &mut FileOperationQueue, store: TaskQueueStore) {
    assert_eq!(queue.set_store_and_restore(store), None);
}

/// 入队必须被接受,返回本地任务 id。
pub(crate) fn queued_task_id(outcome: FileOperationEnqueueOutcome) -> u64 {
    match outcome {
        FileOperationEnqueueOutcome::Queued { task_id } => task_id,
        FileOperationEnqueueOutcome::Rejected { error } => panic!("enqueue rejected: {error}"),
    }
}

/// 驱动到队列里没有待执行的持久化请求为止,返回回执里的存储错误(按出现顺序)。
pub(crate) fn drain_persistence_errors(queue: &mut FileOperationQueue) -> Vec<String> {
    let mut errors = Vec::new();
    while let Some(request) = queue.take_next_persistence_request() {
        let outcome = execute_file_operation_persistence(request);
        errors.extend(queue.accept_persistence_outcome(outcome).error);
    }
    errors
}

/// 驱动全部持久化并断言回执里没有存储错误。
pub(crate) fn assert_persisted(queue: &mut FileOperationQueue) {
    let errors = drain_persistence_errors(queue);
    assert!(errors.is_empty(), "unexpected storage errors: {errors:?}");
}

/// 入队并按正式版步骤完成写库,返回本地任务 id。
pub(crate) fn enqueue_persisted(
    queue: &mut FileOperationQueue,
    operation: QueuedFileOperation,
) -> u64 {
    let task_id = queued_task_id(queue.enqueue(operation));
    assert_persisted(queue);
    task_id
}
