//! 二维码链接：本机每个局域网地址一条；先列地址，点一个才出码，能换（扫码下载与链接
//! 接收共用）。

use iced::widget::image::Handle;

use crate::view::transfer_qr::qr_image_handle;

#[derive(Debug, Clone)]
pub(crate) struct QrLinks {
    pub(crate) urls: Vec<String>,
    /// 选中的地址与它的二维码；None 时列出地址。
    pub(crate) chosen: Option<(usize, Handle)>,
}

impl QrLinks {
    pub(super) fn new(urls: Vec<String>) -> Self {
        Self { urls, chosen: None }
    }

    /// 点选即生成：fast_qr 矩阵计算在微秒级，不用异步。
    pub(super) fn choose(&mut self, index: usize) {
        if let Some(url) = self.urls.get(index) {
            self.chosen = Some((index, qr_image_handle(url)));
        }
    }

    pub(super) fn show_addresses(&mut self) {
        self.chosen = None;
    }
}
