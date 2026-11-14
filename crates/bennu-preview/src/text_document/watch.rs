//! 预览中的文本文件有没有变：本地文件用 inotify 监视父目录（编辑器「写临时文件再改名」
//! 替换文件也收得到），网络文件系统上 inotify 收不到别的机器的改动，改为每秒看一次
//! （与 KDE KDirWatch 对网络挂载的做法相同）。这里只报「可能变了」，变成了什么由引擎
//! 拿文件戳比对决定。

use std::path::{Path, PathBuf};
use std::time::Duration;

use iced::futures::{SinkExt, Stream};
use iced::Subscription;
use notify::{RecursiveMode, Watcher};

use crate::preview_message::PreviewMessage;

/// 写日志常是一阵一阵的：收到事件后再等这么久合并成一次。
const CHANGE_SETTLE: Duration = Duration::from_millis(100);
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// 网络/用户态文件系统的 `statfs` 魔数：NFS、SMB、CIFS、SMB2、FUSE、Ceph、AFS、Coda、9P、NCP。
const NETWORK_FILESYSTEM_MAGICS: [i64; 10] = [
    0x6969,
    0x517b,
    0xff53_4d42,
    0xfe53_4d42,
    0x6573_5546,
    0x00c3_6400,
    0x5346_414f,
    0x7375_7245,
    0x0102_1997,
    0x564c,
];

/// 文档 `document` 的文件变化订阅；文档换了，订阅随键一起换掉。
pub fn text_file_watch_subscription(document: u64, path: &Path) -> Subscription<PreviewMessage> {
    Subscription::run_with((document, path.to_path_buf()), watch_text_file)
}

fn watch_text_file(key: &(u64, PathBuf)) -> impl Stream<Item = PreviewMessage> + use<> {
    let (document, path) = key.clone();
    iced::stream::channel(1, async move |mut output| {
        let (sender, mut events) = tokio::sync::mpsc::unbounded_channel::<()>();
        let watcher = (!is_network_filesystem(&path))
            .then(|| start_watcher(&path, sender))
            .flatten();
        loop {
            match &watcher {
                Some(_) => {
                    if events.recv().await.is_none() {
                        return;
                    }
                    tokio::time::sleep(CHANGE_SETTLE).await;
                    while events.try_recv().is_ok() {}
                }
                None => tokio::time::sleep(POLL_INTERVAL).await,
            }
            if output
                .send(PreviewMessage::TextFileChanged { document })
                .await
                .is_err()
            {
                return;
            }
        }
    })
}

fn start_watcher(
    path: &Path,
    sender: tokio::sync::mpsc::UnboundedSender<()>,
) -> Option<notify::RecommendedWatcher> {
    let parent = path.parent()?.to_path_buf();
    let target = path.to_path_buf();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let concerns_target = match &event {
            Ok(event) => event.need_rescan() || event.paths.contains(&target),
            Err(_) => true,
        };
        if concerns_target {
            let _ = sender.send(());
        }
    })
    .ok()?;
    watcher.watch(&parent, RecursiveMode::NonRecursive).ok()?;
    Some(watcher)
}

fn is_network_filesystem(path: &Path) -> bool {
    let probe = path.parent().unwrap_or(path);
    // f_type 的整数类型随架构不同，统一换成 i64 比较。
    #[allow(clippy::unnecessary_cast)]
    rustix::fs::statfs(probe)
        .map(|stat| NETWORK_FILESYSTEM_MAGICS.contains(&(stat.f_type as i64)))
        .unwrap_or(false)
}
