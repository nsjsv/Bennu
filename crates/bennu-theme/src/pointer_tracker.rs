use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use crate::pointer_motion::PointerMotion;
use iced::advanced::widget::tree;
use iced::advanced::{layout, overlay, renderer, widget, Clipboard, Layout, Shell, Widget};
use iced::{mouse, Element, Event, Length, Point, Rectangle, Size, Vector};

/// 指针最后所在的位置（窗口坐标），由窗口根部的 [`track_pointer`] 静默写入。
///
/// 位置是输入状态，不是应用状态：大多数时候只在按下、悬停重算时读一次。若每次
/// 移动都发消息，iced 每条消息都要重建所有窗口的界面并重画（高回报率鼠标每秒
/// 三百多次），所以只写这里，不打扰 update。
#[derive(Debug, Clone, Default)]
pub struct PointerPosition {
    position: Rc<Cell<Option<Point>>>,
    /// 最后一次真移动的时刻（全屏控件「停多久才藏」读它）。
    motion: PointerMotion,
}

impl PointerPosition {
    pub fn get(&self) -> Option<Point> {
        self.position.get()
    }

    pub fn set(&self, position: Option<Point>) {
        self.position.set(position);
    }

    pub fn motion(&self) -> PointerMotion {
        self.motion.clone()
    }
}

/// 窗口里的指针移动怎样作为消息发出去。
pub enum PointerFollow<'a, Message> {
    /// 没有东西跟着指针走：只记位置和时刻。
    Still,
    /// 每次移动都发：拖动、改大小、框选，全屏时把藏着的控件叫出来。
    Moves(Box<dyn Fn(Point) -> Message + 'a>),
    /// 第一次移动、以及进出这一条（窗口坐标的纵向范围）时才发：底部控制条移到这里才出现。
    Band {
        top: f32,
        bottom: f32,
        on_move: Box<dyn Fn(Point) -> Message + 'a>,
    },
}

/// 包住一个窗口的整棵界面：记下指针位置和移动时刻，按 `follow` 决定哪些移动作为消息发出去。
pub fn track_pointer<'a, Message, Theme, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
    position: Option<PointerPosition>,
    follow: PointerFollow<'a, Message>,
) -> Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    Element::new(PointerTracker {
        content: content.into(),
        position,
        follow,
    })
}

struct PointerTracker<'a, Message, Theme, Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
    position: Option<PointerPosition>,
    follow: PointerFollow<'a, Message>,
}

/// 上次发出移动时指针在不在那一条里；不是按「一条」跟着时清掉，再按一条跟时第一次移动照发。
#[derive(Debug, Default)]
struct TrackerState {
    in_band: Option<bool>,
}

impl<'a, Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for PointerTracker<'a, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<TrackerState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(TrackerState::default())
    }

    fn children(&self) -> Vec<widget::Tree> {
        vec![widget::Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut widget::Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        // 坐标取自事件本身：指针在浮层上时 iced 把 `cursor` 标成不可用，但根部照样收到移动。
        if let Event::Mouse(mouse::Event::CursorMoved { position }) = event {
            if let Some(tracked) = &self.position {
                tracked.set(Some(*position));
                tracked.motion.record(Instant::now());
            }
            let state = tree.state.downcast_mut::<TrackerState>();
            match &self.follow {
                PointerFollow::Still => state.in_band = None,
                PointerFollow::Moves(on_move) => {
                    state.in_band = None;
                    shell.publish(on_move(*position));
                }
                PointerFollow::Band {
                    top,
                    bottom,
                    on_move,
                } => {
                    let in_band = (*top..=*bottom).contains(&position.y);
                    if state.in_band != Some(in_band) {
                        state.in_band = Some(in_band);
                        shell.publish(on_move(*position));
                    }
                }
            }
        }
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
    }

    fn mouse_interaction(
        &self,
        tree: &widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut Renderer,
        theme: &Theme,
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

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut widget::Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
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
    use iced::advanced::layout::Limits;
    use iced::advanced::{clipboard, widget::Tree};

    use super::*;

    /// 让一串指针移动经过记录器：返回记下的位置、有没有记下时刻、发出的消息。
    fn move_pointer_through(
        follow: impl Fn() -> PointerFollow<'static, Point>,
        moves: &[Point],
    ) -> (Option<Point>, bool, Vec<Point>) {
        let tracked = PointerPosition::default();
        let mut element: Element<'_, Point, (), ()> =
            track_pointer(iced::widget::Space::new(), Some(tracked.clone()), follow());
        let mut tree = Tree::new(&element);
        let node = element.as_widget_mut().layout(
            &mut tree,
            &(),
            &Limits::new(Size::ZERO, Size::new(100.0, 100.0)),
        );
        let mut messages = Vec::new();
        for moved_to in moves {
            let mut shell = Shell::new(&mut messages);
            element.as_widget_mut().update(
                &mut tree,
                &Event::Mouse(mouse::Event::CursorMoved {
                    position: *moved_to,
                }),
                Layout::new(&node),
                mouse::Cursor::Unavailable,
                &(),
                &mut clipboard::Null,
                &mut shell,
                &Rectangle::new(Point::ORIGIN, Size::new(100.0, 100.0)),
            );
        }
        (tracked.get(), tracked.motion().last().is_some(), messages)
    }

    /// 没人跟着指针走时只记位置和时刻、不发消息；每次都跟时每次都发。
    #[test]
    fn movement_is_recorded_and_only_published_while_followed() {
        let moved_to = Point::new(12.0, 24.0);

        assert_eq!(
            move_pointer_through(|| PointerFollow::Still, &[moved_to]),
            (Some(moved_to), true, Vec::new())
        );
        assert_eq!(
            move_pointer_through(
                || PointerFollow::Moves(Box::new(|position| position)),
                &[moved_to]
            ),
            (Some(moved_to), true, vec![moved_to])
        );
    }

    /// 按一条跟着时：第一次移动发，之后只在进出那一条时发。
    #[test]
    fn a_band_publishes_the_first_move_and_crossings_only() {
        let moves = [
            Point::new(5.0, 10.0),
            Point::new(6.0, 20.0),
            Point::new(7.0, 85.0),
            Point::new(8.0, 90.0),
            Point::new(9.0, 40.0),
            Point::new(9.0, 41.0),
        ];
        let (_, _, published) = move_pointer_through(
            || PointerFollow::Band {
                top: 80.0,
                bottom: 100.0,
                on_move: Box::new(|position| position),
            },
            &moves,
        );
        assert_eq!(published, vec![moves[0], moves[2], moves[4]]);
    }
}
