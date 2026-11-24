//! 根部指针政策的不变量：箭头设置只把可点控件的手形换成箭头，其余形状、真链接不动；
//! 链接信号每一轮重新算；菜单、提示气泡这些浮层（含嵌套的）遵守同一条映射。
//!
//! 用 iced 的空渲染器 `()` 直接驱动 `Widget::mouse_interaction` 和浮层。

use std::cell::Cell;
use std::rc::Rc;

use iced::advanced::layout::{self, Limits};
use iced::advanced::mouse::Interaction;
use iced::advanced::widget::{tree, Tree};
use iced::advanced::{overlay, renderer, Layout, Widget};
use iced::{mouse, Element, Length, Point, Rectangle, Size, Vector};

use super::{control_pointer, ControlPointer, LinkPointer};

/// 子树「按脚本」回答：当前报什么形状、是不是链接（返回 `Pointer` 时报到）、有没有浮层。
struct Script {
    interaction: Cell<Interaction>,
    marks_link: Cell<bool>,
    overlay_interaction: Cell<Option<Interaction>>,
    links: LinkPointer,
}

impl Script {
    fn answer(&self, interaction: Interaction) -> Interaction {
        if self.marks_link.get() && interaction == Interaction::Pointer {
            self.links.mark_hovered();
        }
        interaction
    }
}

struct ScriptedWidget(Rc<Script>);

/// 部件自己的状态（滚动位置、输入焦点的替身）：根部类型一变就会被重建成初始值。
const FRESH_STATE: u32 = 7;

impl Widget<(), (), ()> for ScriptedWidget {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<u32>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(FRESH_STATE)
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn layout(&mut self, _tree: &mut Tree, _renderer: &(), limits: &Limits) -> layout::Node {
        layout::Node::new(limits.max())
    }

    fn draw(
        &self,
        _tree: &Tree,
        _renderer: &mut (),
        _theme: &(),
        _style: &renderer::Style,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &(),
    ) -> Interaction {
        self.0.answer(self.0.interaction.get())
    }

    fn overlay<'b>(
        &'b mut self,
        _tree: &'b mut Tree,
        _layout: Layout<'b>,
        _renderer: &(),
        _viewport: &Rectangle,
        _translation: Vector,
    ) -> Option<overlay::Element<'b, (), (), ()>> {
        self.0.overlay_interaction.get()?;
        Some(overlay::Element::new(Box::new(ScriptedOverlay {
            script: self.0.clone(),
            has_nested: true,
        })))
    }
}

/// 一层浮层，带一层嵌套浮层（菜单里再弹出的子菜单）。
struct ScriptedOverlay {
    script: Rc<Script>,
    has_nested: bool,
}

impl overlay::Overlay<(), (), ()> for ScriptedOverlay {
    fn layout(&mut self, _renderer: &(), bounds: Size) -> layout::Node {
        layout::Node::new(bounds)
    }

    fn draw(
        &self,
        _renderer: &mut (),
        _theme: &(),
        _style: &renderer::Style,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
    ) {
    }

    fn mouse_interaction(
        &self,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _renderer: &(),
    ) -> Interaction {
        self.script
            .answer(self.script.overlay_interaction.get().unwrap_or_default())
    }

    fn overlay<'b>(
        &'b mut self,
        _layout: Layout<'b>,
        _renderer: &(),
    ) -> Option<overlay::Element<'b, (), (), ()>> {
        self.has_nested.then(|| {
            overlay::Element::new(Box::new(ScriptedOverlay {
                script: self.script.clone(),
                has_nested: false,
            }))
        })
    }
}

struct Window {
    element: Element<'static, (), (), ()>,
    tree: Tree,
    node: layout::Node,
    script: Rc<Script>,
}

impl Window {
    fn new(controls: ControlPointer) -> Self {
        let links = LinkPointer::default();
        let script = Rc::new(Script {
            interaction: Cell::new(Interaction::None),
            marks_link: Cell::new(false),
            overlay_interaction: Cell::new(None),
            links: links.clone(),
        });
        let mut element = control_pointer(
            Element::new(ScriptedWidget(script.clone())),
            controls,
            links,
        );
        let mut tree = Tree::new(&element);
        let node = element.as_widget_mut().layout(
            &mut tree,
            &(),
            &Limits::new(Size::ZERO, Size::new(100.0, 100.0)),
        );
        Self {
            element,
            tree,
            node,
            script,
        }
    }

    fn content_shows(&self) -> Interaction {
        self.element.as_widget().mouse_interaction(
            &self.tree,
            Layout::new(&self.node),
            mouse::Cursor::Unavailable,
            &Rectangle::new(Point::ORIGIN, Size::new(100.0, 100.0)),
            &(),
        )
    }

    /// 浮层与它嵌套的浮层各自报的形状。
    fn overlays_show(&mut self) -> (Interaction, Interaction) {
        let layout = Layout::new(&self.node);
        let mut top = self
            .element
            .as_widget_mut()
            .overlay(
                &mut self.tree,
                layout,
                &(),
                &Rectangle::new(Point::ORIGIN, Size::new(100.0, 100.0)),
                Vector::ZERO,
            )
            .expect("the scripted widget has an overlay");
        let top_shows = top
            .as_overlay()
            .mouse_interaction(layout, mouse::Cursor::Unavailable, &());
        let nested_shows = top
            .as_overlay_mut()
            .overlay(layout, &())
            .expect("the scripted overlay has a nested overlay")
            .as_overlay()
            .mouse_interaction(layout, mouse::Cursor::Unavailable, &());
        (top_shows, nested_shows)
    }
}

#[test]
fn switching_the_setting_keeps_the_widget_state_below_the_root() {
    let mut window = Window::new(ControlPointer::Arrow);
    *window.tree.children[0].state.downcast_mut::<u32>() = FRESH_STATE + 1;

    let hand = Window::new(ControlPointer::Hand);
    window.tree.diff(&hand.element);

    assert_eq!(
        *window.tree.children[0].state.downcast_ref::<u32>(),
        FRESH_STATE + 1
    );
}

#[test]
fn arrow_turns_a_controls_hand_into_the_arrow() {
    let window = Window::new(ControlPointer::Arrow);
    window.script.interaction.set(Interaction::Pointer);

    assert_eq!(window.content_shows(), Interaction::Idle);
}

#[test]
fn arrow_leaves_every_other_shape_alone() {
    let window = Window::new(ControlPointer::Arrow);
    for shape in [
        Interaction::None,
        Interaction::Idle,
        Interaction::Text,
        Interaction::Grab,
        Interaction::NotAllowed,
        Interaction::ResizingColumn,
    ] {
        window.script.interaction.set(shape);
        assert_eq!(window.content_shows(), shape, "{shape:?}");
    }
}

#[test]
fn arrow_keeps_the_hand_over_a_link() {
    let window = Window::new(ControlPointer::Arrow);
    window.script.interaction.set(Interaction::Pointer);
    window.script.marks_link.set(true);

    assert_eq!(window.content_shows(), Interaction::Pointer);
}

#[test]
fn a_link_mark_does_not_outlive_its_pass() {
    let window = Window::new(ControlPointer::Arrow);
    window.script.interaction.set(Interaction::Pointer);
    window.script.marks_link.set(true);
    assert_eq!(window.content_shows(), Interaction::Pointer);

    // 指针从链接移到按钮上：这一轮没人报到，上一轮的报到不能算数。
    window.script.marks_link.set(false);
    assert_eq!(window.content_shows(), Interaction::Idle);
}

#[test]
fn hand_passes_everything_through() {
    let window = Window::new(ControlPointer::Hand);
    for marks_link in [false, true] {
        window.script.marks_link.set(marks_link);
        for shape in [Interaction::Pointer, Interaction::Text, Interaction::None] {
            window.script.interaction.set(shape);
            assert_eq!(window.content_shows(), shape, "{shape:?} {marks_link}");
        }
    }
}

#[test]
fn arrow_applies_to_overlays_and_their_nested_overlays() {
    let mut window = Window::new(ControlPointer::Arrow);
    window
        .script
        .overlay_interaction
        .set(Some(Interaction::Pointer));
    assert_eq!(
        window.overlays_show(),
        (Interaction::Idle, Interaction::Idle)
    );

    window.script.marks_link.set(true);
    assert_eq!(
        window.overlays_show(),
        (Interaction::Pointer, Interaction::Pointer)
    );

    // `None` 的意思是「指针不在浮层上」，必须原样交回，否则下面的窗口内容收不到指针。
    window.script.marks_link.set(false);
    window
        .script
        .overlay_interaction
        .set(Some(Interaction::None));
    assert_eq!(
        window.overlays_show(),
        (Interaction::None, Interaction::None)
    );
}

#[test]
fn hand_leaves_overlays_alone() {
    let mut window = Window::new(ControlPointer::Hand);
    window
        .script
        .overlay_interaction
        .set(Some(Interaction::Pointer));

    assert_eq!(
        window.overlays_show(),
        (Interaction::Pointer, Interaction::Pointer)
    );
}
