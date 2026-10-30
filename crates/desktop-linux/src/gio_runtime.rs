//! 所有 GIO 操作共用的一个常驻线程。GIO 对象只能在拥有它的线程与 thread-default
//! `MainContext` 上使用；进程级 `VolumeMonitor` 要由一直在跑的主循环驱动，设备拔出等信号
//! 才会继续更新（见 gvfs-device-guidelines）。调用方的闭包在该线程上构造 future，作为主循环
//! 上的本地任务执行，彼此不阻塞——一个慢的网络挂载不会挡住设备刷新。跨线程只返回纯 Rust 值。

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, LazyLock};
use std::thread;

use gio::glib::MainContext;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

#[derive(Debug, Error)]
pub enum GioRuntimeStartError {
    #[error("could not start the GIO thread: {0}")]
    Thread(#[source] io::Error),
    #[error("the GIO thread stopped during initialization")]
    InitializationStopped,
}

#[derive(Debug, Error)]
pub enum GioRuntimeError {
    #[error("could not start the GIO runtime: {0}")]
    Start(#[source] Arc<GioRuntimeStartError>),
    #[error("the GIO runtime stopped before replying")]
    Stopped,
}

type GioJob = Box<dyn FnOnce(gio::VolumeMonitor) -> Pin<Box<dyn Future<Output = ()>>> + Send>;

struct GioRuntime {
    job_sender: mpsc::UnboundedSender<GioJob>,
}

static GIO_RUNTIME: LazyLock<Result<GioRuntime, Arc<GioRuntimeStartError>>> =
    LazyLock::new(|| GioRuntime::start().map_err(Arc::new));

impl GioRuntime {
    fn start() -> Result<Self, GioRuntimeStartError> {
        let (job_sender, mut job_receiver) = mpsc::unbounded_channel::<GioJob>();
        let (startup_sender, startup_receiver) = std::sync::mpsc::sync_channel(1);
        thread::Builder::new()
            .name("bennu-gio".to_owned())
            .spawn(move || {
                let context = MainContext::new();
                let spawner = context.clone();
                context.block_on(async move {
                    let monitor = gio::VolumeMonitor::get();
                    if startup_sender.send(()).is_err() {
                        return;
                    }
                    while let Some(job) = job_receiver.recv().await {
                        spawner.spawn_local(job(monitor.clone()));
                    }
                });
            })
            .map_err(GioRuntimeStartError::Thread)?;
        startup_receiver
            .recv()
            .map_err(|_| GioRuntimeStartError::InitializationStopped)?;
        Ok(Self { job_sender })
    }
}

/// 在 GIO 线程上执行 `job` 构造的 future，等它的结果。
pub(crate) async fn run_on_gio_thread<T, Job, Work>(job: Job) -> Result<T, GioRuntimeError>
where
    T: Send + 'static,
    Job: FnOnce(gio::VolumeMonitor) -> Work + Send + 'static,
    Work: Future<Output = T> + 'static,
{
    let runtime = match &*GIO_RUNTIME {
        Ok(runtime) => runtime,
        Err(source) => return Err(GioRuntimeError::Start(source.clone())),
    };
    let (response_sender, response_receiver) = oneshot::channel();
    let job: GioJob = Box::new(move |monitor| {
        Box::pin(async move {
            let _ = response_sender.send(job(monitor).await);
        })
    });
    runtime
        .job_sender
        .send(job)
        .map_err(|_| GioRuntimeError::Stopped)?;
    response_receiver
        .await
        .map_err(|_| GioRuntimeError::Stopped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn gio_requests_reuse_one_thread_default_context() {
        let probe = || {
            run_on_gio_thread(|_| async {
                (
                    thread::current().id(),
                    MainContext::thread_default().is_some(),
                )
            })
        };
        let first = probe().await.unwrap();
        let second = probe().await.unwrap();

        assert!(first.1 && second.1);
        assert_eq!(first.0, second.0);
        assert_ne!(first.0, thread::current().id());
    }

    #[tokio::test]
    async fn a_pending_request_does_not_block_later_requests() {
        let (started_sender, started_receiver) = oneshot::channel();
        let (release_sender, release_receiver) = oneshot::channel::<()>();
        let first = tokio::spawn(run_on_gio_thread(move |_| async move {
            let _ = started_sender.send(());
            let _ = release_receiver.await;
            "first"
        }));
        started_receiver.await.unwrap();

        let second = run_on_gio_thread(|_| async { "second" }).await.unwrap();

        assert_eq!(second, "second");
        release_sender.send(()).unwrap();
        assert_eq!(first.await.unwrap().unwrap(), "first");
    }
}
