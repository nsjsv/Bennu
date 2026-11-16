//! 虚拟列表的可见行按条目保留控件状态。
//!
//! iced 的 `Column` 按位置对比新旧子控件的状态。可见行窗口一挪（上面出去几行、下面进来几行），每个位置都
//! 换成了别的行：文件名的排版缓存对不上、整屏重排，悬停状态挂到了别的行上。iced 的 keyed column 只认得
//! 「中间插入或删除了一段」，认不出这种平移。这里记住每个位置上是哪一行（键：行在整个列表里的序号），对比前
//! 先把旧状态按键挪到新位置，新来的行给空状态（iced 对比时按控件类型新建），再交给 `Column`。

use std::collections::HashMap;

use iced::advanced::widget::{tree, Operation, Tree};
use iced::advanced::{layout, overlay, renderer, Clipboard, Layout, Shell, Widget};
use iced::widget::Column;
use iced::{mouse, Element, Event, Length, Rectangle, Size, Vector};

/// 一列行，每行带一个在整个列表里不变的键（行序号）。
pub(crate) fn keyed_rows<'a, Message, Theme, Renderer>(
    rows: impl IntoIterator<Item = (u64, Element<'a, Message, Theme, Renderer>)>,
) -> Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    let (keys, rows): (Vec<u64>, Vec<_>) = rows.into_iter().unzip();
    Element::new(KeyedRows {
        keys,
        // 宽高照 `Column` 的规则随子控件撑开（有撑满的行就撑满），与直接放进外层列时一样。
        column: Column::with_children(rows).into(),
    })
}

struct KeyedRows<'a, Message, Theme, Renderer> {
    keys: Vec<u64>,
    column: Element<'a, Message, Theme, Renderer>,
}

/// 上次每个位置上是哪一行。
struct KeyedRowsState {
    keys: Vec<u64>,
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for KeyedRows<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<KeyedRowsState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(KeyedRowsState {
            keys: self.keys.clone(),
        })
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.column)]
    }

    fn diff(&self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<KeyedRowsState>();
        if state.keys != self.keys {
            let rows = &mut tree.children[0].children;
            let mut previous: HashMap<u64, Tree> =
                state.keys.iter().copied().zip(rows.drain(..)).collect();
            rows.extend(
                self.keys
                    .iter()
                    .map(|key| previous.remove(key).unwrap_or_else(Tree::empty)),
            );
            state.keys.clone_from(&self.keys);
        }
        self.column.as_widget().diff(&mut tree.children[0]);
    }

    fn size(&self) -> Size<Length> {
        self.column.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.column.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.column
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.column
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.column.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.column.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.column.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        self.column.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

#[cfg(test)]
#[path = "keyed_rows_tests.rs"]
mod tests;
