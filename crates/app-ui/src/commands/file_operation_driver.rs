use iced::futures::SinkExt;

use crate::app::archive_member_password::ArchiveMemberPasswordAction;
use crate::model::Message;
use file_operations::{
    run_file_operation, FileOperationEvent, RunningFileOperation, FILE_OPERATION_EVENT_CAPACITY,
};

/// 驱动者是 `Task::stream`:一次性托付给执行器跑到流结束,不进订阅 hash 名册,
/// 不会被 subscription() 重评估静默回收。驱动者死亡由队列监督检测并换代重启,
/// 见 `file_operations::FileOperationQueue` 的监督。
pub(crate) fn file_operation_driver_task(task: RunningFileOperation) -> iced::Task<Message> {
    iced::Task::stream(iced::stream::channel(
        FILE_OPERATION_EVENT_CAPACITY,
        async move |mut output| {
            let (task_id, generation) = (task.id, task.generation);
            let (events, mut received) = tokio::sync::mpsc::channel(FILE_OPERATION_EVENT_CAPACITY);
            // 转发与执行并行:界面不取走时事件通道填满,执行器随之停在发送上。
            // 执行器返回即关闭发送端,转发读空余下事件后结束,终态因此总排在事件之后。
            let forward_events = async {
                while let Some(event) = received.recv().await {
                    let _ = output.send(file_operation_event_message(event)).await;
                }
            };
            let (completion, ()) = tokio::join!(run_file_operation(task, events), forward_events);
            let _ = output
                .send(Message::FileOperationFinished(
                    task_id, generation, completion,
                ))
                .await;
        },
    ))
}

fn file_operation_event_message(event: FileOperationEvent) -> Message {
    match event {
        FileOperationEvent::Progressed {
            task_id,
            update,
            entries,
        } => Message::FileOperationProgressed(task_id, update, entries),
        FileOperationEvent::MovesRenamed { task_id, moves } => {
            Message::FileOperationMovesRenamed { task_id, moves }
        }
        FileOperationEvent::DirectMovesCommitted { task_id, commits } => {
            Message::FileOperationDirectMovesCommitted { task_id, commits }
        }
        FileOperationEvent::ArchiveMemberPasswordRequested {
            sources,
            destination,
            invalid_retry,
        } => Message::ArchiveMemberPasswordRequested {
            action: ArchiveMemberPasswordAction::Extract {
                sources,
                destination,
            },
            invalid_retry,
        },
    }
}

#[cfg(test)]
mod tests {
    use iced::futures::StreamExt;

    use super::*;
    use file_operations::{FileOperationCompletion, FileOperationQueue, QueuedFileOperation};

    #[tokio::test]
    async fn completion_is_the_last_message_after_every_event() {
        let directory = tempfile::tempdir().unwrap();
        let mut queue = FileOperationQueue::new();
        queue.enqueue(QueuedFileOperation::CreateDirectory {
            parent: directory.path().to_path_buf(),
        });
        let running = queue.active_subscription().unwrap();
        let (task_id, generation) = (running.id, running.generation);

        let mut stream =
            iced_runtime::task::into_stream(file_operation_driver_task(running)).unwrap();
        let mut messages = Vec::new();
        while let Some(action) = stream.next().await {
            if let iced_runtime::Action::Output(message) = action {
                messages.push(message);
            }
        }

        assert!(matches!(
            messages.as_slice(),
            [
                Message::FileOperationProgressed(progressed_id, _, _),
                Message::FileOperationFinished(
                    finished_id,
                    finished_generation,
                    FileOperationCompletion::Succeeded(_),
                ),
            ] if *progressed_id == task_id
                && *finished_id == task_id
                && *finished_generation == generation
        ));
    }
}
