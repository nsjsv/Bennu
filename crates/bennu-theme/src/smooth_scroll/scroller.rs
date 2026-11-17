//! 驱动器怎么量、怎么滚它包着的那个滚动区：子树里第一个滚动区就是它。

use iced::advanced::widget::operation::scrollable::{AbsoluteOffset, Scrollable};
use iced::advanced::widget::{self, Operation, Tree};
use iced::advanced::{Layout, Widget};
use iced::{Rectangle, Vector};

use super::touch::{split_at_edges, Orientation, ScrollTarget};

/// 操作途中拿到的那个滚动区。
pub(super) struct Scroller<'a> {
    state: &'a mut dyn Scrollable,
    bounds: Rectangle,
    content_bounds: Rectangle,
    offset: Vector,
}

impl Scroller<'_> {
    pub(super) fn offset(&self) -> Vector {
        self.offset
    }

    fn max(&self) -> Vector {
        Vector::new(
            self.content_bounds.width - self.bounds.width,
            self.content_bounds.height - self.bounds.height,
        )
    }

    /// 滚 `delta`：和滚动区自己一样夹在边界内（不能滚的轴不动）。
    pub(super) fn scroll_by(&mut self, delta: Vector) {
        let max = self.max();
        let along =
            |offset: f32, max: f32, delta: f32| offset + split_at_edges(offset, max, delta).0;
        self.offset = Vector::new(
            along(self.offset.x, max.x, delta.x),
            along(self.offset.y, max.y, delta.y),
        );
        self.state.scroll_by(
            AbsoluteOffset {
                x: delta.x,
                y: delta.y,
            },
            self.bounds,
            self.content_bounds,
        );
    }
}

impl ScrollTarget for Scroller<'_> {
    fn scroll(&mut self, orientation: Orientation, delta: f32) -> (f32, f32) {
        let max = self.max();
        let (offset, max, along) = match orientation {
            Orientation::Horizontal => (self.offset.x, max.x, Vector::new(1.0, 0.0)),
            Orientation::Vertical => (self.offset.y, max.y, Vector::new(0.0, 1.0)),
        };
        let (scrolled, beyond) = split_at_edges(offset, max, delta);
        if scrolled != 0.0 {
            self.scroll_by(along * scrolled);
        }
        (scrolled, beyond)
    }

    fn extent(&self, orientation: Orientation) -> f32 {
        match orientation {
            Orientation::Horizontal => self.bounds.width,
            Orientation::Vertical => self.bounds.height,
        }
    }
}

/// 找到子树里第一个滚动区交给 `act`；没有滚动区时 `act` 不会被调用。
pub(super) fn with_first_scroller<Message, Theme, Renderer, Act>(
    content: &mut dyn Widget<Message, Theme, Renderer>,
    tree: &mut Tree,
    layout: Layout<'_>,
    renderer: &Renderer,
    act: Act,
) where
    Renderer: iced::advanced::Renderer,
    Act: for<'a> FnOnce(&mut Scroller<'a>) + Send,
{
    let mut operation = FirstScroller { act: Some(act) };
    content.operate(tree, layout, renderer, &mut operation);
}

struct FirstScroller<Act> {
    /// 遇到第一个滚动区时取走。
    act: Option<Act>,
}

impl<Act> Operation for FirstScroller<Act>
where
    Act: for<'a> FnOnce(&mut Scroller<'a>) + Send,
{
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        if self.act.is_some() {
            operate(self);
        }
    }

    fn scrollable(
        &mut self,
        _id: Option<&widget::Id>,
        bounds: Rectangle,
        content_bounds: Rectangle,
        translation: Vector,
        state: &mut dyn Scrollable,
    ) {
        if let Some(act) = self.act.take() {
            act(&mut Scroller {
                state,
                bounds,
                content_bounds,
                offset: translation,
            });
        }
    }
}
