//! 滚动位置只在应用需要时报告。
//!
//! 滚动区的 `on_scroll` 每帧都发应用消息，iced 每条消息重建、重排所有窗口；而应用按位置排的内容
//! （可见行多排了几行、缩略图多排了几行）在挪动不大时并不用变。这个包装每个事件后读一次滚动区的
//! 位置，只在应用会用到时报告：
//!
//! 1. 第一次见到这个滚动区，或它的边界、内容大小变了；
//! 2. 纵向偏移出了应用给的「不用知道」的范围；
//! 3. 里面的控件这次发了消息（点击、悬停进出），或按了键、按下松开了鼠标：报告排在这些消息前面，
//!    应用处理输入时手里的位置就是准的（键盘走订阅，比控件的消息晚一轮到）；
//! 4. 停下来了：位置一段时间没变。
//!
//! 每次报告都说明此刻在动还是停下了（`ScrollMotion`）。报告过「在动」之后一定还会报告一次「停下」：
//! 应用按它决定什么时候开始做费时的事（缩略图），滚动中一闪而过的格子不做。

use std::ops::Range;
use std::time::{Duration, Instant};

use iced::advanced::widget::operation::scrollable::Scrollable;
use iced::advanced::widget::{tree, Operation, Tree};
use iced::advanced::{layout, overlay, renderer, widget, Clipboard, Layout, Shell, Widget};
use iced::{keyboard, mouse, window, Element, Event, Length, Rectangle, Size, Vector};

/// 位置这么久没变就算停下：比惯性尾巴两次重画的最长间隔（100 ms）长。
const SETTLE: Duration = Duration::from_millis(150);

/// 滚动区此刻的位置。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollPosition {
    /// 内容滚过的距离（画出来的整像素位置）。
    pub offset: Vector,
    /// 滚动区在窗口里的位置和大小。
    pub bounds: Rectangle,
    pub content_bounds: Rectangle,
}

/// 报告时滚动区在动还是停下了。第一次见到的滚动区算停下（刚打开的目录不用等）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollMotion {
    Moving,
    Resting,
}

/// 包在滚动区（连同它的惯性驱动器）外面。`quiet` 是纵向偏移里应用不用知道的范围（按它已经排出来的
/// 内容算），空范围（`0.0..0.0`）表示每次变化都要报告。
pub fn report_scroll<'a, Message, Theme, Renderer>(
    scroller: impl Into<Element<'a, Message, Theme, Renderer>>,
    quiet: Range<f32>,
    on_report: impl Fn(ScrollPosition, ScrollMotion) -> Message + 'a,
) -> Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    Element::new(ScrollReport {
        content: scroller.into(),
        quiet,
        on_report: Box::new(on_report),
    })
}

struct ScrollReport<'a, Message, Theme, Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
    quiet: Range<f32>,
    on_report: Box<dyn Fn(ScrollPosition, ScrollMotion) -> Message + 'a>,
}

#[derive(Debug, Default)]
struct ReportState {
    /// 上次报告的是哪个滚动区、在哪儿、在不在动：同一位置的控件状态可能换给了另一个滚动区（换目录）。
    reported: Option<(Option<widget::Id>, ScrollPosition, ScrollMotion)>,
    /// 最近看到的偏移，和它最后一次变是什么时候（第一次见到不算变）。
    seen: Option<Vector>,
    moved_at: Option<Instant>,
}

enum Observation {
    /// 报告；还在动时到 `check_at` 再看一次，那时没再动就报告停下。
    Report {
        motion: ScrollMotion,
        check_at: Option<Instant>,
    },
    /// 现在不用报：到这个时刻还没动就算停下。
    CheckAt(Instant),
    UpToDate,
}

impl ReportState {
    fn observe(
        &mut self,
        id: Option<widget::Id>,
        position: ScrollPosition,
        quiet: &Range<f32>,
        acting: bool,
        now: Instant,
    ) -> Observation {
        if self.seen.is_some_and(|offset| offset != position.offset) {
            self.moved_at = Some(now);
        }
        self.seen = Some(position.offset);
        let settles_at = self.moved_at.map(|at| at + SETTLE).filter(|at| now < *at);
        let motion = match settles_at {
            Some(_) => ScrollMotion::Moving,
            None => ScrollMotion::Resting,
        };
        let observation = match (&self.reported, settles_at) {
            (Some((reported_id, reported, reported_motion)), _)
                if *reported_id == id && *reported == position && *reported_motion == motion =>
            {
                return settles_at.map_or(Observation::UpToDate, Observation::CheckAt);
            }
            (Some((reported_id, reported, _)), Some(settles_at))
                if *reported_id == id
                    && reported.bounds == position.bounds
                    && reported.content_bounds.size() == position.content_bounds.size()
                    && !acting
                    && quiet.contains(&position.offset.y) =>
            {
                Observation::CheckAt(settles_at)
            }
            _ => Observation::Report {
                motion,
                check_at: settles_at,
            },
        };
        if let Observation::Report { motion, .. } = observation {
            self.reported = Some((id, position, motion));
        }
        observation
    }
}

/// 应用会马上按位置处理的输入（键盘走订阅，晚于本轮的控件消息到）。
fn is_input(event: &Event) -> bool {
    matches!(
        event,
        Event::Keyboard(keyboard::Event::KeyPressed { .. })
            | Event::Mouse(mouse::Event::ButtonPressed(_) | mouse::Event::ButtonReleased(_))
    )
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for ScrollReport<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<ReportState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(ReportState::default())
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
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
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
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
        self.content
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
        let Tree {
            state, children, ..
        } = tree;
        let content = self.content.as_widget_mut();
        // 里面的消息先收在这儿：要报告时位置排在它们前面。
        let mut published = Vec::new();
        let mut inner = Shell::new(&mut published);
        content.update(
            &mut children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            &mut inner,
            viewport,
        );
        let acting = !inner.is_empty() || is_input(event);
        if let Some((id, position)) = read_first(content, &mut children[0], layout, renderer) {
            let now = match event {
                Event::Window(window::Event::RedrawRequested(now)) => *now,
                _ => Instant::now(),
            };
            let state = state.downcast_mut::<ReportState>();
            match state.observe(id, position, &self.quiet, acting, now) {
                Observation::Report { motion, check_at } => {
                    shell.publish((self.on_report)(position, motion));
                    if let Some(at) = check_at {
                        shell.request_redraw_at(at);
                    }
                }
                Observation::CheckAt(at) => shell.request_redraw_at(at),
                Observation::UpToDate => {}
            }
        }
        shell.merge(inner, |message| message);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
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
        tree: &Tree,
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
        tree: &'b mut Tree,
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

/// 子树里第一个滚动区（就是包着的那个）的 id 和位置。
fn read_first<Message, Theme, Renderer>(
    content: &mut dyn Widget<Message, Theme, Renderer>,
    tree: &mut Tree,
    layout: Layout<'_>,
    renderer: &Renderer,
) -> Option<(Option<widget::Id>, ScrollPosition)>
where
    Renderer: iced::advanced::Renderer,
{
    let mut operation = ReadFirst(None);
    content.operate(tree, layout, renderer, &mut operation);
    operation.0
}

struct ReadFirst(Option<(Option<widget::Id>, ScrollPosition)>);

impl Operation for ReadFirst {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        if self.0.is_none() {
            operate(self);
        }
    }

    fn scrollable(
        &mut self,
        id: Option<&widget::Id>,
        bounds: Rectangle,
        content_bounds: Rectangle,
        translation: Vector,
        _state: &mut dyn Scrollable,
    ) {
        self.0.get_or_insert((
            id.cloned(),
            ScrollPosition {
                offset: translation,
                bounds,
                content_bounds,
            },
        ));
    }
}

#[cfg(test)]
#[path = "scroll_report_tests.rs"]
mod tests;
