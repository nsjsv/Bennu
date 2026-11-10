//! 通过链接接收：LocalSend 窗口本机卡片里"从手机接收"，出二维码；手机网页发来的文件
//! 进与 LocalSend 设备发来时同一套接收任务、传输记录、通知。
//!
//! 一次只有一个当前链接。停止（按钮或关窗口）= 当前链接没了、不再接新的；那次服务在
//! 后台流里把开着的接收收完才关，期间的事件照常进任务面板。

use iced::futures::SinkExt;
use iced::Task;
use local_send::{CancellationToken, LinkControl, LinkEvent, LinkReceiveConfig, LinkReceiveServer};
use tokio::sync::broadcast;

use super::super::transfer_service::transfer_download_dir;
use super::super::FileBrowser;
use super::qr_links::QrLinks;
use super::tasks::ReceiveSource;
use super::TransferMessage;
use crate::model::Message;

#[derive(Debug, Default)]
pub(crate) struct LinkReceiving {
    pub(crate) current: Option<LinkReceive>,
    next_id: u64,
}

#[derive(Debug)]
pub(crate) struct LinkReceive {
    /// 这次链接的编号：停下的旧链接后来发的消息不动当前的。
    id: u64,
    pub(crate) phase: LinkReceivePhase,
    /// 取消 = 停止：后台流让服务不再接新的。
    stop: CancellationToken,
}

#[derive(Debug, Clone)]
pub(crate) enum LinkReceivePhase {
    Starting,
    Ready(QrLinks),
    Failed(String),
    /// 10 分钟没人用，服务已停。
    TimedOut,
}

#[derive(Debug, Clone)]
pub(crate) enum LinkReceiveMessage {
    StartPressed,
    StopPressed,
    Started {
        link: u64,
        urls: Vec<String>,
    },
    Failed {
        link: u64,
        error: String,
    },
    AddressChosen(usize),
    AddressListRequested,
    Event {
        link: u64,
        control: LinkControl,
        event: LinkEvent,
    },
}

impl FileBrowser {
    pub(super) fn handle_link_receive(&mut self, message: LinkReceiveMessage) -> Task<Message> {
        match message {
            LinkReceiveMessage::StartPressed => return self.start_link_receive(),
            LinkReceiveMessage::StopPressed => self.stop_link_receive(),
            LinkReceiveMessage::Started { link, urls } => {
                if let Some(current) = self.current_link(link) {
                    current.phase = LinkReceivePhase::Ready(QrLinks::new(urls));
                }
            }
            LinkReceiveMessage::Failed { link, error } => {
                if let Some(current) = self.current_link(link) {
                    current.phase = LinkReceivePhase::Failed(error);
                }
            }
            LinkReceiveMessage::AddressChosen(index) => {
                if let Some(LinkReceivePhase::Ready(links)) = self.link_phase_mut() {
                    links.choose(index);
                }
            }
            LinkReceiveMessage::AddressListRequested => {
                if let Some(LinkReceivePhase::Ready(links)) = self.link_phase_mut() {
                    links.show_addresses();
                }
            }
            LinkReceiveMessage::Event {
                link,
                control,
                event,
            } => return self.accept_link_event(link, control, event),
        }
        Task::none()
    }

    /// 已经在出码就不重开；停下、超时、失败之后再点是新的一次（新端口、新随机码）。
    fn start_link_receive(&mut self) -> Task<Message> {
        if let Some(LinkReceivePhase::Starting | LinkReceivePhase::Ready(_)) = self.link_phase_mut()
        {
            return Task::none();
        }
        let link = self.link_receiving.next_id;
        self.link_receiving.next_id += 1;
        let stop = CancellationToken::new();
        self.link_receiving.current = Some(LinkReceive {
            id: link,
            phase: LinkReceivePhase::Starting,
            stop: stop.clone(),
        });
        let config = LinkReceiveConfig {
            computer_name: self.effective_transfer_device_alias(),
            download_dir: transfer_download_dir(&self.user_config),
        };
        link_receive_command(link, config, stop)
    }

    /// 停止按钮与关窗口共用：正在传的接着传完。
    pub(in crate::app) fn stop_link_receive(&mut self) {
        if let Some(current) = self.link_receiving.current.take() {
            current.stop.cancel();
        }
    }

    fn accept_link_event(
        &mut self,
        link: u64,
        control: LinkControl,
        event: LinkEvent,
    ) -> Task<Message> {
        match event {
            LinkEvent::Accepted {
                session_id,
                address,
                files,
            } => self.accept_receive_started(
                session_id,
                tr!("Browser %1", address),
                ReceiveSource::Link(control),
                files,
            ),
            LinkEvent::Progress {
                session_id,
                file_id,
                received,
            } => self.accept_receive_progress(&session_id, file_id, received),
            LinkEvent::Finished { session_id, report } => {
                return self.finish_receive(&session_id, &report);
            }
            LinkEvent::TimedOut => {
                if let Some(current) = self.current_link(link) {
                    current.phase = LinkReceivePhase::TimedOut;
                }
            }
            // 用户停下的：卡片早就收起了。
            LinkEvent::Closed => {}
        }
        Task::none()
    }

    fn current_link(&mut self, link: u64) -> Option<&mut LinkReceive> {
        self.link_receiving
            .current
            .as_mut()
            .filter(|current| current.id == link)
    }

    fn link_phase_mut(&mut self) -> Option<&mut LinkReceivePhase> {
        self.link_receiving
            .current
            .as_mut()
            .map(|current| &mut current.phase)
    }
}

/// 后台流持有服务：停下后照常转发事件，直到服务关掉（收完或超时）。
fn link_receive_command(
    link: u64,
    config: LinkReceiveConfig,
    stop: CancellationToken,
) -> Task<Message> {
    Task::stream(iced::stream::channel(32, async move |mut output| {
        let started = tokio::select! {
            () = stop.cancelled() => return,
            started = LinkReceiveServer::start(config) => started,
        };
        let handle = match started {
            Ok(handle) => handle,
            Err(error) => {
                let _ = output
                    .send(LinkReceiveMessage::Failed {
                        link,
                        error: error.to_string(),
                    })
                    .await;
                return;
            }
        };
        let control = handle.control();
        let mut events = handle.events();
        let _ = output
            .send(LinkReceiveMessage::Started {
                link,
                urls: handle.urls().to_vec(),
            })
            .await;
        let mut stopping = false;
        loop {
            tokio::select! {
                () = stop.cancelled(), if !stopping => {
                    stopping = true;
                    control.stop();
                }
                event = events.recv() => match event {
                    Ok(event) => {
                        let ended = matches!(event, LinkEvent::TimedOut | LinkEvent::Closed);
                        let _ = output
                            .send(LinkReceiveMessage::Event {
                                link,
                                control: control.clone(),
                                event,
                            })
                            .await;
                        if ended {
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return,
                },
            }
        }
    }))
    .map(|message| Message::Transfer(TransferMessage::LinkReceive(message)))
}

#[cfg(test)]
mod tests;
