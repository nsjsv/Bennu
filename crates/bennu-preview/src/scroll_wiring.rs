//! 宿主滚动管线接线：滚动显隐状态机与 `Message::SmoothScrollWheel` /
//! `Message::ScrollbarViewportChanged` 管线留在宿主（scrollbar-guidelines
//! 的分工），面板经本结构接收单个滚动区域的三段接线——smooth-scroll
//! 包装、scrollable id 与视口回传闭包——由宿主用
//! `smooth_scroll_content` / `smooth_scroll_id` / `scrollbar_on_scroll`
//! 构造后注入，本 crate 不感知 `ScrollbarRegion` 枚举。

use iced::widget::scrollable;
use iced::{Element, Theme};

use bennu_theme::scrollbar::ScrollbarViewport;
use bennu_theme::styles::ScrollbarVisibility;

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
    /// 视口回传闭包：宿主在内部合成 ScrollbarViewportChanged 与面板事件。
    pub on_scroll: Box<dyn Fn(scrollable::Viewport) -> Message + 'a>,
}

/// 单个滚动区域的完整接线包：显隐档位、视口快照与宿主持有的滚动管线
/// 接线。预览主面板有七个同构滚动区域，捆绑成命名结构避免平铺参数的
/// 错位风险（两个 visibility 互换能静默编译通过）。生命周期随所接线的
/// ScrollRegionWiring（'a 出现在 Fn 参数位，不变）；面板要求 'static
/// 接线的区域用 ScrollRegionState<'static, _>，宿主经生命周期泛型辅助
/// 在对应生命周期上实例化。
pub struct ScrollRegionState<'a, Message>
where
    Message: 'a,
{
    pub visibility: ScrollbarVisibility,
    pub viewport: Option<ScrollbarViewport>,
    pub wiring: ScrollRegionWiring<'a, Message>,
}
