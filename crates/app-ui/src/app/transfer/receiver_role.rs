//! 同一台电脑同一个用户只能有一个 Bennu 进程开 LocalSend 接收：端口、设备身份都只有一份，
//! 而二次启动默认是另起一个进程开新窗口。拿到锁的进程收；其它进程只发不收、不露面，一直等
//! 着锁——收的那个进程退出（崩溃也一样，内核会放锁）时马上接手。
//!
//! 不用"端口被占了就当别人在收"：被别的程序占了要照样报错。也不用桌面激活的 D-Bus 名字：
//! 它只在启动时争一次，主实例退出后剩下的窗口不会接手。

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::futures::SinkExt;
use iced::{Subscription, Task};

use super::super::FileBrowser;
use super::TransferMessage;
use crate::model::Message;

const RECEIVER_LOCK_FILE_NAME: &str = "localsend-receiver.lock";
/// 等锁的线程只阻塞在一个系统调用上，用不着默认的 2 MiB 栈。
const WAITER_STACK_SIZE: usize = 64 * 1024;
/// 接手时最多等前一个进程关掉接收端口多久：它退出时锁和端口先后放掉，正常只差几毫秒。
const PORT_RELEASE_WAIT: Duration = Duration::from_secs(2);
const PORT_RELEASE_POLL: Duration = Duration::from_millis(50);

/// 拿着就是这个进程负责接收；文件关掉（进程退出）锁就放了。
#[derive(Debug)]
pub(crate) struct ReceiverLock {
    _file: File,
}

/// 这个进程在 LocalSend 接收上的角色。
#[derive(Debug)]
pub(crate) enum ReceiverRole {
    /// 还没争过（服务还没启动过）。
    Unclaimed,
    /// 这个进程负责接收：拿着锁直到进程退出。锁文件打不开时没有锁，照旧自己收（同时开两个
    /// 进程会报端口被占用）。
    Receiving { _lock: Option<Arc<ReceiverLock>> },
    /// 别的 Bennu 进程在收：只发不收、不露面，等那个进程退出后接手。
    Standby,
}

impl ReceiverRole {
    /// 不等：锁在别人手里就先只发不收，由 `receiver_lock_subscription` 接着等。
    fn claim() -> Self {
        match open_lock_file().and_then(|file| match file.try_lock() {
            Ok(()) => Ok(Some(file)),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(error)) => Err(error),
        }) {
            Ok(Some(file)) => Self::Receiving {
                _lock: Some(Arc::new(ReceiverLock { _file: file })),
            },
            Ok(None) => Self::Standby,
            Err(error) => {
                tracing::warn!(
                    target: "app_ui::transfer",
                    event = "localsend_receiver_lock_unavailable",
                    %error,
                    "could not use the LocalSend receiver lock; receiving without it"
                );
                Self::Receiving { _lock: None }
            }
        }
    }

    pub(crate) fn is_receiving(&self) -> bool {
        matches!(self, Self::Receiving { .. })
    }
}

async fn port_released(port: u16) {
    let deadline = Instant::now() + PORT_RELEASE_WAIT;
    while tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .is_err()
        && Instant::now() < deadline
    {
        tokio::time::sleep(PORT_RELEASE_POLL).await;
    }
}

fn receiver_lock_path() -> PathBuf {
    file_core::app_dirs::app_data_dir().join(RECEIVER_LOCK_FILE_NAME)
}

fn open_lock_file() -> io::Result<File> {
    let path = receiver_lock_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    open_existing_or_new(&path)
}

/// 锁文件只当锁用，里面不写东西；不能截断，别的进程可能正锁着它。
fn open_existing_or_new(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

impl FileBrowser {
    /// 服务第一次启动前定下这个进程收不收。
    pub(super) fn claim_receiver_role(&mut self) {
        if matches!(self.transfer_receiver_role, ReceiverRole::Unclaimed) {
            self.transfer_receiver_role = ReceiverRole::claim();
        }
    }

    /// 等到了锁：这个进程接手接收。前一个进程退出时先放锁、后关端口（谁先谁后由不得
    /// 它），等端口能绑了再按新角色重启服务；端口被别的程序占着就照常重启，由接收端报错。
    pub(super) fn accept_receiver_lock(&mut self, lock: Arc<ReceiverLock>) -> Task<Message> {
        self.transfer_receiver_role = ReceiverRole::Receiving { _lock: Some(lock) };
        Task::perform(port_released(self.user_config.transfer_port), |()| {
            Message::Transfer(TransferMessage::ReceiverPortReleased)
        })
    }

    pub(crate) fn waits_for_receiver_lock(&self) -> bool {
        matches!(self.transfer_receiver_role, ReceiverRole::Standby)
    }
}

/// 只发不收时挂着：等到锁就发 `ReceiverLockAcquired`。
pub(crate) fn receiver_lock_subscription() -> Subscription<Message> {
    Subscription::run(wait_for_receiver_lock)
}

/// 阻塞等锁放在单独的线程：tokio 的阻塞线程池在运行时关闭时要等它跑完，那样等着锁的
/// 窗口就关不掉了。订阅没了线程还在等，等到时没人收，锁随文件关掉立刻放回去。
fn wait_for_receiver_lock() -> impl iced::futures::Stream<Item = Message> {
    iced::stream::channel(1, async |mut output| {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let spawned = std::thread::Builder::new()
            .name("localsend-receiver-lock".to_owned())
            .stack_size(WAITER_STACK_SIZE)
            .spawn(move || {
                let _ = sender.send(open_lock_file().and_then(|file| file.lock().map(|()| file)));
            });
        let acquired = match spawned {
            Ok(_) => receiver
                .await
                .unwrap_or_else(|_| Err(io::Error::other("the receiver lock waiter stopped"))),
            Err(error) => Err(error),
        };
        match acquired {
            Ok(file) => {
                let lock = Arc::new(ReceiverLock { _file: file });
                let _ = output
                    .send(Message::Transfer(TransferMessage::ReceiverLockAcquired(
                        lock,
                    )))
                    .await;
            }
            Err(error) => tracing::warn!(
                target: "app_ui::transfer",
                event = "localsend_receiver_lock_wait_failed",
                %error,
                "stopped waiting for the LocalSend receiver lock; this window keeps only sending"
            ),
        }
        std::future::pending::<()>().await;
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_holder_at_a_time_and_the_next_one_gets_it_when_the_holder_closes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(RECEIVER_LOCK_FILE_NAME);

        let holder = open_existing_or_new(&path).unwrap();
        holder.try_lock().unwrap();
        // 另一次打开（另一个进程同理）拿不到。
        let second = open_existing_or_new(&path).unwrap();
        assert!(matches!(second.try_lock(), Err(TryLockError::WouldBlock)));

        drop(holder);
        second.try_lock().unwrap();
    }
}
