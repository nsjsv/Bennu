//! 宿主滚动接线：面板经本结构接收单个滚动区域的接线——惯性滚动、scrollable id 与可选的
//! 视口回传闭包——由宿主按自己的区域模型构造后注入，本 crate 不感知宿主的区域枚举。
//! 滑块本身由 `bennu_theme::scrollbar` 按当帧布局自绘，不需要宿主提供视口。

use bennu_theme::smooth_scroll::SmoothScroll;
use iced::widget::{scrollable, Scrollable};
use iced::{Element, Theme};

pub struct ScrollRegionWiring<'a, Message>
where
    Message: 'a,
{
    /// 宿主给这个区域的惯性滚动（滚轮在控件里按帧推进）。
    pub smooth_scroll: SmoothScroll<'a, Message>,
    /// 滚动容器的部件 id（宿主的跳转操作目标）。
    pub scrollable_id: iced::widget::Id,
    /// 视口回传闭包：只有需要记录滚动位置的区域（文档分页、纯文本、
    /// Markdown）才有；目录/归档树与 SQLite 表格不关心滚动位置。
    pub on_scroll: Option<Box<dyn Fn(scrollable::Viewport) -> Message + 'a>>,
}

impl<'a, Message> ScrollRegionWiring<'a, Message>
where
    Message: 'a,
{
    /// 按宿主接线建滚动容器：内容先经惯性滚动认领滚轮，挂上 id 与视口回传；方向、样式、
    /// 尺寸和滚动条由面板在 `finish` 里定，最后整个交给惯性滚动驱动。
    pub fn scrollable(
        self,
        content: Element<'a, Message, Theme>,
        finish: impl FnOnce(Scrollable<'a, Message>) -> Element<'a, Message, Theme>,
    ) -> Element<'a, Message, Theme> {
        let scroller = scrollable(self.smooth_scroll.capture(content)).id(self.scrollable_id);
        let scroller = match self.on_scroll {
            Some(on_scroll) => scroller.on_scroll(on_scroll),
            None => scroller,
        };
        self.smooth_scroll.drive(finish(scroller))
    }
}
