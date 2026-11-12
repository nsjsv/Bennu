//! 宿主滚动管线接线：`Message::SmoothScrollWheel` 惯性管线和视口事件留在
//! 宿主，面板经本结构接收单个滚动区域的接线——smooth-scroll 包装、
//! scrollable id 与可选的视口回传闭包——由宿主用 `smooth_scroll_content` /
//! `smooth_scroll_id` 构造后注入，本 crate 不感知 `ScrollbarRegion` 枚举。
//! 滑块本身由 `bennu_theme::scrollbar` 按当帧布局自绘，不需要宿主提供视口。

use iced::widget::{scrollable, Scrollable};
use iced::{Element, Theme};

/// smooth-scroll 包装闭包类型：面板内容进出宿主捕获层。
type SmoothScrollWrap<'a, Message> =
    Box<dyn Fn(Element<'a, Message, Theme>) -> Element<'a, Message, Theme> + 'a>;

pub struct ScrollRegionWiring<'a, Message>
where
    Message: 'a,
{
    /// 把面板内容包进宿主的 smooth-scroll 捕获层（滚轮惯性管线）。
    pub smooth_scroll_wrap: SmoothScrollWrap<'a, Message>,
    /// 几何宿主 scrollable 的部件 id（scroll_by 操作目标）。
    pub scrollable_id: iced::widget::Id,
    /// 视口回传闭包：只有需要记录滚动位置的区域（文档分页、纯文本、
    /// Markdown）才有；目录/归档树与 SQLite 表格不关心滚动位置。
    pub on_scroll: Option<Box<dyn Fn(scrollable::Viewport) -> Message + 'a>>,
}

impl<'a, Message> ScrollRegionWiring<'a, Message>
where
    Message: 'a,
{
    /// 按宿主接线建滚动容器：内容进 smooth-scroll 捕获层，挂上 id 与视口回传。
    /// 方向、样式和尺寸仍由面板决定。
    pub fn scrollable(self, content: Element<'a, Message, Theme>) -> Scrollable<'a, Message> {
        let scroller = scrollable((self.smooth_scroll_wrap)(content)).id(self.scrollable_id);
        match self.on_scroll {
            Some(on_scroll) => scroller.on_scroll(on_scroll),
            None => scroller,
        }
    }
}
