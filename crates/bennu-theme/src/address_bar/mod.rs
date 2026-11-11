//! 地址栏共享层：面包屑分段/宽度分配算法、编辑会话与过渡状态机、
//! 弹性面包屑布局 widget。主程序（多窗格）与 FileChooser portal
//! （单窗格）消费同一份实现；窗格归属、拖放、滚动区域等前端关注点
//! 留在各自的视图拼装层。

pub mod elastic;
pub mod model;

use iced::widget::container;
use iced::{Element, Length};

use crate::styles::address_bar_mask_style;

pub use elastic::{
    BreadcrumbMeasurement, ElasticBreadcrumbs, ADDRESS_BAR_HEIGHT, ADDRESS_TEXT_SIZE,
    BREADCRUMB_HOME_WIDTH, BREADCRUMB_HORIZONTAL_PADDING, BREADCRUMB_ICON_SIZE,
    BREADCRUMB_MINIMUM_TEXT_WIDTH, BREADCRUMB_SEPARATOR_SIZE, BREADCRUMB_SEPARATOR_WIDTH,
};
pub use model::{
    allocate_breadcrumb_widths, breadcrumb_segments, AddressBarTransition, AddressEditingSession,
    AddressEditingSessionId, AddressSuggestionRequest, BreadcrumbSegment, BreadcrumbSegmentKind,
    BreadcrumbWidthAllocation, ADDRESS_BAR_TRANSITION_DURATION,
};

/// 编辑输入框、退出快照这类盖在面包屑上的层：先垫一层不透明窗口底色，
/// 面包屑就不会透过来；上层自己的半透明底色再叠出与外框相同的颜色。
pub fn address_bar_overlay<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    container(content)
        .width(Length::Fill)
        .height(Length::Fixed(ADDRESS_BAR_HEIGHT))
        .style(address_bar_mask_style)
        .into()
}
