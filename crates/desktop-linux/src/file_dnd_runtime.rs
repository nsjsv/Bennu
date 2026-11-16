use smithay_client_toolkit::reexports::calloop::channel::{self, Channel, Sender};

/// 拖放线程（Wayland、X11）的所有权句柄：drop 即让线程退出。
///
/// 线程平时阻塞在事件循环里，不再定时醒来看关闭标志，所以关闭本身必须是一个
/// 能叫醒它的事件：句柄里的发送端 drop 后，线程那头的通道收到 `Closed`。
#[derive(Debug)]
pub struct FileDndRuntime {
    _shutdown: Sender<()>,
}

pub(crate) fn file_dnd_runtime() -> (FileDndRuntime, Channel<()>) {
    let (shutdown, closed) = channel::channel();
    (
        FileDndRuntime {
            _shutdown: shutdown,
        },
        closed,
    )
}
