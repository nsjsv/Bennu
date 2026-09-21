//! 地址栏共享层：面包屑分段/宽度分配算法、编辑会话与过渡状态机、
//! 弹性面包屑布局 widget。主程序（多窗格）与 FileChooser portal
//! （单窗格）消费同一份实现；窗格归属、拖放、滚动区域等前端关注点
//! 留在各自的视图拼装层。

pub mod elastic;
pub mod model;

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
