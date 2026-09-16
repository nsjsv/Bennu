//! 分组索引栏:列表/大图右侧的竖排短标签浮层。整栏是一个自绘交互
//! 容器,职责有三:
//! 1. 事件屏障——光标在栏内时捕获鼠标事件,索引栏范围内的指针事件
//!    不得穿透成条目 hover/选中/框选/右键(Stack 的上层子节点捕获后
//!    下层滚动条与外层 mouse_area 都不再处理);唯独放行滚轮,悬停
//!    索引栏时列表照常滚动。
//! 2. 扫动手势——"按住"状态是容器局部 State:栏内按下即跳组并记下
//!    按住,按住移动逐组换档,任意位置的释放都结束手势(拖出栏外松开
//!    同样被兜底)。状态不进 FileBrowser,视图切换/分组关闭时组件随
//!    组件树整体丢弃,菜单打开后下一次释放也会清掉,不存在泄漏路径。
//! 3. 点击跳组——栏内按下那一步立即完成跳转,同一点击的释放由按钮
//!    再发一次同一目标,重复定位到相同偏移是幂等的。

use iced::advanced::widget::{self, Tree};
use iced::advanced::{
    layout, mouse, renderer, Clipboard, Layout, Shell, Widget,
};
use iced::widget::{button, column, container};
use iced::{Element, Event, Length, Rectangle, Size};

use crate::model::{BrowserPaneId, FileGroupRailEntry, Message};
use crate::typography::readable_text;

/// 每个索引项的固定行高:点击/扫动把纵向位置换算成组序的几何依据,
/// 标签在其中垂直居中,行高不随文字自适应。
pub(crate) const RAIL_ITEM_HEIGHT: f32 = 20.0;
const RAIL_LABEL_TEXT_SIZE: u32 = 11;
/// 栏右缘与滚动条 thumb 的间距:thumb 悬停加宽到 SCROLLBAR_HOVER_WIDTH,
/// 列表内容流还有 6px 右内边距(即列表 thumb 外缘最多伸到 20px 处),
/// 取 20px 让两个视图的索引栏都落在 thumb 区域内侧,永不遮挡滚动条。
const RAIL_TRAILING_GAP: f32 = crate::model::SCROLLBAR_HOVER_WIDTH + 6.0;

/// 纵向位置 → 组序:索引项等高纵排,除以行高取整再夹进有效范围。
/// 独立成纯函数供扫动换档的数学单测。
fn group_index_at_y(y: f32, group_count: usize) -> usize {
    if group_count == 0 {
        return 0;
    }
    ((y / RAIL_ITEM_HEIGHT).floor().max(0.0) as usize).min(group_count - 1)
}

/// 索引栏浮层:右缘避开滚动条 thumb,纵向居中;宽度由最宽标签撑出
/// (按钮 Shrink),每组一个透明无框文本按钮,当前可视组常亮高亮。
pub(crate) fn file_grouping_rail_view(
    entries: &[FileGroupRailEntry],
    active_group: Option<usize>,
    pane: BrowserPaneId,
) -> Element<'static, Message> {
    let labels = entries.iter().enumerate().map(|(index, entry)| {
        let label: Element<'static, Message> =
            readable_text(entry.index_label.clone()).size(RAIL_LABEL_TEXT_SIZE).into();
        button(
            container(label)
                .height(Length::Fixed(RAIL_ITEM_HEIGHT))
                .center_y(Length::Fixed(RAIL_ITEM_HEIGHT)),
        )
        .on_press(Message::FileGroupingRailTargetSelected {
            pane,
            group_index: index,
        })
        .padding([0, 5])
        .width(Length::Shrink)
        .height(Length::Fixed(RAIL_ITEM_HEIGHT))
        .style(crate::appearance::grouping_rail_button_style(
            active_group == Some(index),
        ))
        .into()
    });
    let labels: Element<'static, Message> = column(labels).spacing(0).into();
    // 扫动手势与事件屏障包住整个标签列;拦下的点击/移动只在此层生效。
    let scrubber: Element<'static, Message> = Element::new(GroupingRailScrubber::new(
        labels,
        entries.len(),
        pane,
    ));
    container(scrubber)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(iced::Padding {
            top: 0.0,
            right: RAIL_TRAILING_GAP,
            bottom: 0.0,
            left: 0.0,
        })
        .align_x(iced::alignment::Horizontal::Right)
        .center_y(Length::Fill)
        .into()
}

/// 扫动手势的容器局部状态:只关心左键是否按住,外加屏障的悬停事实。
#[derive(Debug, Default)]
struct RailScrubGesture {
    pressed: bool,
    hovered: bool,
}

/// 索引栏的自绘交互容器:把按钮列包成一层,统一处理事件屏障与
/// 按住扫动(按下记状态、移动换档、释放收尾)。
struct GroupingRailScrubber<'a> {
    content: Element<'a, Message>,
    group_count: usize,
    pane: BrowserPaneId,
}

impl<'a> GroupingRailScrubber<'a> {
    fn new(content: Element<'a, Message>, group_count: usize, pane: BrowserPaneId) -> Self {
        Self {
            content,
            group_count,
            pane,
        }
    }

    fn target_message(&self, group_index: usize) -> Message {
        Message::FileGroupingRailTargetSelected {
            pane: self.pane,
            group_index,
        }
    }
}

impl Widget<Message, iced::Theme, iced::Renderer> for GroupingRailScrubber<'_> {
    fn size(&self) -> Size<Length> {
        Size {
            width: Length::Shrink,
            height: Length::Shrink,
        }
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(RailScrubGesture::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        // 子层(按钮)先行:点击释放、悬停样式都由按钮自身处理。
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );

        let state: &mut RailScrubGesture = tree.state.downcast_mut();
        let bounds = layout.bounds();
        // 屏障进出账在任何鼠标事件上都核对一次:栏可能直接出现在静止
        // 光标下方(分组开启/切档),首个滚轮事件也要能补发 Entered——
        // 光标在栏内时下层行的 exit 永远不会发出,滚动补偿与条目 hover
        // 都依赖这份显式入账。
        if let Event::Mouse(_) = event {
            let over = cursor.is_over(bounds);
            if over != state.hovered {
                shell.publish(if over {
                    Message::FileGroupingRailCursorEntered(self.pane)
                } else {
                    Message::FileGroupingRailCursorExited(self.pane)
                });
                state.hovered = over;
            }
        }
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(position) = cursor.position_in(bounds) {
                    state.pressed = true;
                    shell.publish(self.target_message(group_index_at_y(
                        position.y,
                        self.group_count,
                    )));
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                // 换档只在按住期间发布;未按住的悬停不产生跳组。
                if state.pressed {
                    if let Some(position) = cursor.position_in(bounds) {
                        shell.publish(self.target_message(group_index_at_y(
                            position.y,
                            self.group_count,
                        )));
                    }
                }
            }
            // 释放与光标位置无关:拖出栏外松开同样结束手势。
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                state.pressed = false;
            }
            _ => {}
        }

        // 事件屏障:栏内指针事件在此截停,条目 hover/选中/框选/右键全部
        // 失效;滚轮刻意放行,悬停索引栏时内容区照常滚动。
        let blocks_pass_through = cursor.is_over(bounds)
            && matches!(event, Event::Mouse(mouse_event)
                if !matches!(mouse_event, mouse::Event::WheelScrolled { .. }));
        if blocks_pass_through {
            shell.capture_event();
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            renderer,
            operation,
        );
    }

    fn overlay<'a>(
        &'a mut self,
        tree: &'a mut Tree,
        layout: Layout<'a>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: iced::Vector,
    ) -> Option<iced::advanced::overlay::Element<'a, Message, iced::Theme, iced::Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_index_at_y_maps_item_height_to_group_order() {
        assert_eq!(group_index_at_y(0.0, 4), 0);
        assert_eq!(group_index_at_y(RAIL_ITEM_HEIGHT - 0.1, 4), 0);
        assert_eq!(group_index_at_y(RAIL_ITEM_HEIGHT, 4), 1);
        assert_eq!(group_index_at_y(RAIL_ITEM_HEIGHT * 2.5, 4), 2);
        // 越界位置夹到两端。
        assert_eq!(group_index_at_y(-12.0, 4), 0);
        assert_eq!(group_index_at_y(RAIL_ITEM_HEIGHT * 99.0, 4), 3);
    }

    #[test]
    fn group_index_at_y_is_safe_for_empty_rail() {
        assert_eq!(group_index_at_y(RAIL_ITEM_HEIGHT, 0), 0);
    }
}
