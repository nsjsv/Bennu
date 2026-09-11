use iced::advanced::{layout, overlay, renderer, widget, Clipboard, Layout, Shell, Widget};
use iced::mouse;
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector};

const FLOATING_SURFACE_MARGIN: f32 = 18.0;

pub(crate) fn floating_surface<'a, Message>(
    content: impl Into<Element<'a, Message>>,
    floating: Vec<FloatingContent<'a, Message>>,
) -> Element<'a, Message>
where
    Message: Clone + 'a,
{
    Element::new(FloatingSurface {
        content: content.into(),
        floating,
        background_input_policy: BackgroundInputPolicy::Interactive,
        outside_click_dismissal: None,
    })
}

pub(crate) fn modal_floating_surface<'a, Message>(
    content: impl Into<Element<'a, Message>>,
    floating: Vec<FloatingContent<'a, Message>>,
) -> Element<'a, Message>
where
    Message: Clone + 'a,
{
    Element::new(FloatingSurface {
        content: content.into(),
        floating,
        background_input_policy: BackgroundInputPolicy::Blocked,
        outside_click_dismissal: None,
    })
}

pub(crate) fn dismissable_blocking_floating_surface<'a, Message>(
    content: impl Into<Element<'a, Message>>,
    floating: Vec<FloatingContent<'a, Message>>,
    dismiss_message: Message,
) -> Element<'a, Message>
where
    Message: Clone + 'a,
{
    Element::new(FloatingSurface {
        content: content.into(),
        floating,
        background_input_policy: BackgroundInputPolicy::Blocked,
        outside_click_dismissal: Some(OutsideClickDismissal {
            message: dismiss_message,
            policy: OutsideDismissalPolicy::CapturedPrimaryPress,
        }),
    })
}

pub(crate) fn replaceable_context_menu_floating_surface<'a, Message>(
    content: impl Into<Element<'a, Message>>,
    floating: Vec<FloatingContent<'a, Message>>,
    dismiss_message: Message,
) -> Element<'a, Message>
where
    Message: Clone + 'a,
{
    Element::new(FloatingSurface {
        content: content.into(),
        floating,
        background_input_policy: BackgroundInputPolicy::Blocked,
        outside_click_dismissal: Some(OutsideClickDismissal {
            message: dismiss_message,
            policy: OutsideDismissalPolicy::ContextMenuReplacement,
        }),
    })
}

#[derive(Clone)]
struct OutsideClickDismissal<Message> {
    message: Message,
    policy: OutsideDismissalPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutsideDismissalPolicy {
    CapturedPrimaryPress,
    ContextMenuReplacement,
}

impl OutsideDismissalPolicy {
    fn dismissed_click_flow(self, input_event: FloatingInputEvent) -> Option<DismissedClickFlow> {
        match (self, input_event) {
            (Self::CapturedPrimaryPress, FloatingInputEvent::PrimaryPress)
            | (Self::ContextMenuReplacement, FloatingInputEvent::PrimaryPress) => {
                Some(DismissedClickFlow::Capture)
            }
            (Self::ContextMenuReplacement, FloatingInputEvent::SecondaryPress) => {
                Some(DismissedClickFlow::Continue)
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DismissedClickFlow {
    Capture,
    Continue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FloatingInputEvent {
    PrimaryPress,
    SecondaryPress,
    OtherMouse,
    NonMouse,
}

impl FloatingInputEvent {
    fn from_iced_event(event: &Event) -> Self {
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => Self::PrimaryPress,
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => Self::SecondaryPress,
            Event::Mouse(_) => Self::OtherMouse,
            _ => Self::NonMouse,
        }
    }

    fn is_mouse(self) -> bool {
        self != Self::NonMouse
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackgroundInputPolicy {
    Interactive,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FloatingPointerTarget {
    FloatingBounds,
    Background,
    CursorUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FloatingInputDecision<Message> {
    dismiss_message: Option<Message>,
    background_update: BackgroundUpdateDecision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackgroundUpdateDecision {
    Update,
    Stop,
    Capture,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum FloatingPlacement {
    Center,
    At(Point),
    Free(Point),
    /// 右下角钉在 anchor 上,元素往锚点左上展开;布局后按实际尺寸
    /// 反推位置,跟随锚点不做屏内回退(如拖拽聚合行挂在指针尖)。
    AnchorBottomRight { anchor: Point },
    BottomLeft {
        left: f32,
        bottom: f32,
    },
    BottomRightInArea {
        area_width: f32,
        right: f32,
        bottom: f32,
    },
}

pub(crate) struct FloatingContent<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    pub(crate) element: Element<'a, Message, Theme, Renderer>,
    pub(crate) placement: FloatingPlacement,
    /// 纯视觉浮层(如拖拽预览)会盖住光标,若捕获指针事件,底层
    /// widget 将收不到松手,拖拽状态无法收尾;此类浮层须置 false。
    pub(crate) captures_pointer: bool,
}

struct FloatingSurface<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    content: Element<'a, Message, Theme, Renderer>,
    floating: Vec<FloatingContent<'a, Message, Theme, Renderer>>,
    background_input_policy: BackgroundInputPolicy,
    outside_click_dismissal: Option<OutsideClickDismissal<Message>>,
}

impl<'a, Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for FloatingSurface<'a, Message, Theme, Renderer>
where
    Message: Clone + 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    fn children(&self) -> Vec<widget::Tree> {
        let mut children = vec![widget::Tree::new(&self.content)];

        for floating in &self.floating {
            children.push(widget::Tree::new(&floating.element));
        }

        children
    }

    fn diff(&self, tree: &mut widget::Tree) {
        let mut children = Vec::with_capacity(1 + self.floating.len());
        children.push(self.content.as_widget());

        for floating in &self.floating {
            children.push(floating.element.as_widget());
        }

        tree.diff_children(&children);
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
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
        let floating_pointer_target =
            self.floating_pointer_target(&mut tree.children, layout, cursor, renderer);
        let input_decision = decide_floating_input(
            self.background_input_policy,
            self.outside_click_dismissal.as_ref(),
            floating_pointer_target,
            FloatingInputEvent::from_iced_event(event),
        );
        if let Some(message) = input_decision.dismiss_message {
            shell.publish(message);
        }
        match input_decision.background_update {
            BackgroundUpdateDecision::Update => {}
            BackgroundUpdateDecision::Stop => return,
            BackgroundUpdateDecision::Capture => {
                shell.capture_event();
                return;
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
        )
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
            self.background_cursor(cursor),
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
            self.background_cursor(cursor),
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
        let mut overlays = Vec::new();
        let mut children = tree.children.iter_mut();

        if let Some(content_tree) = children.next() {
            if let Some(content_overlay) = self.content.as_widget_mut().overlay(
                content_tree,
                layout,
                renderer,
                viewport,
                translation,
            ) {
                overlays.push(content_overlay);
            }
        }

        for (floating, floating_tree) in self.floating.iter_mut().zip(children) {
            overlays.push(overlay::Element::new(Box::new(FloatingOverlay {
                floating: &mut floating.element,
                placement: floating.placement,
                state: floating_tree,
                captures_pointer: floating.captures_pointer,
            })));
        }

        (!overlays.is_empty()).then(|| overlay::Group::with_children(overlays).overlay())
    }
}

impl<'a, Message, Theme, Renderer> FloatingSurface<'a, Message, Theme, Renderer>
where
    Message: Clone + 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    fn floating_pointer_target(
        &mut self,
        children: &mut [widget::Tree],
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
    ) -> FloatingPointerTarget {
        let Some(position) = cursor.position() else {
            return FloatingPointerTarget::CursorUnavailable;
        };

        let surface_size = Size::new(layout.bounds().width, layout.bounds().height);
        let mut checked_any_floating = false;
        for (floating, tree) in self.floating.iter_mut().zip(children.iter_mut().skip(1)) {
            checked_any_floating = true;
            let limits = layout::Limits::new(
                Size::ZERO,
                floating_max_size(floating.placement, surface_size),
            );
            let node = floating
                .element
                .as_widget_mut()
                .layout(tree, renderer, &limits);
            if floating_bounds(floating.placement, node.size(), surface_size).contains(position) {
                return FloatingPointerTarget::FloatingBounds;
            }
        }

        if checked_any_floating {
            FloatingPointerTarget::Background
        } else {
            FloatingPointerTarget::CursorUnavailable
        }
    }

    fn background_cursor(&self, cursor: mouse::Cursor) -> mouse::Cursor {
        match self.background_input_policy {
            BackgroundInputPolicy::Interactive => cursor,
            BackgroundInputPolicy::Blocked => mouse::Cursor::Unavailable,
        }
    }
}

fn decide_floating_input<Message: Clone>(
    background_input_policy: BackgroundInputPolicy,
    outside_click_dismissal: Option<&OutsideClickDismissal<Message>>,
    pointer_target: FloatingPointerTarget,
    input_event: FloatingInputEvent,
) -> FloatingInputDecision<Message> {
    if pointer_target == FloatingPointerTarget::FloatingBounds
        && (background_input_policy == BackgroundInputPolicy::Blocked
            || outside_click_dismissal.is_some())
    {
        return FloatingInputDecision {
            dismiss_message: None,
            background_update: BackgroundUpdateDecision::Stop,
        };
    }

    if pointer_target == FloatingPointerTarget::Background {
        if let Some(dismissal) = outside_click_dismissal {
            if let Some(clicked_event_flow) = dismissal.policy.dismissed_click_flow(input_event) {
                return FloatingInputDecision {
                    dismiss_message: Some(dismissal.message.clone()),
                    background_update: match clicked_event_flow {
                        DismissedClickFlow::Capture => BackgroundUpdateDecision::Capture,
                        DismissedClickFlow::Continue => BackgroundUpdateDecision::Update,
                    },
                };
            }
        }
    }

    if background_input_policy == BackgroundInputPolicy::Blocked {
        return FloatingInputDecision {
            dismiss_message: None,
            background_update: if input_event.is_mouse() {
                BackgroundUpdateDecision::Capture
            } else {
                BackgroundUpdateDecision::Stop
            },
        };
    }

    FloatingInputDecision {
        dismiss_message: None,
        background_update: BackgroundUpdateDecision::Update,
    }
}

fn is_mouse_event(event: &Event) -> bool {
    FloatingInputEvent::from_iced_event(event).is_mouse()
}

struct FloatingOverlay<'a, 'b, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    floating: &'b mut Element<'a, Message, Theme, Renderer>,
    placement: FloatingPlacement,
    state: &'b mut widget::Tree,
    captures_pointer: bool,
}

impl<'a, 'b, Message, Theme, Renderer> overlay::Overlay<Message, Theme, Renderer>
    for FloatingOverlay<'a, 'b, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    fn layout(&mut self, renderer: &Renderer, bounds: Size) -> layout::Node {
        let max_size = floating_max_size(self.placement, bounds);
        let limits = layout::Limits::new(Size::ZERO, max_size);
        let node = self
            .floating
            .as_widget_mut()
            .layout(self.state, renderer, &limits);
        let size = node.size();
        node.move_to(floating_position(self.placement, size, bounds))
    }

    fn update(
        &mut self,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
    ) {
        let bounds = layout.bounds();

        self.floating.as_widget_mut().update(
            self.state, event, layout, cursor, renderer, clipboard, shell, &bounds,
        );

        if self.captures_pointer
            && should_capture_floating_overlay_event(event, cursor, bounds)
        {
            shell.capture_event();
        }
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let bounds = layout.bounds();
        self.floating
            .as_widget()
            .mouse_interaction(self.state, layout, cursor, &bounds, renderer)
    }

    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
    ) {
        let bounds = layout.bounds();
        self.floating
            .as_widget()
            .draw(self.state, renderer, theme, style, layout, cursor, &bounds);
    }

    fn operate(
        &mut self,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.floating
            .as_widget_mut()
            .operate(self.state, layout, renderer, operation);
    }

    fn overlay<'c>(
        &'c mut self,
        layout: Layout<'c>,
        renderer: &Renderer,
    ) -> Option<overlay::Element<'c, Message, Theme, Renderer>> {
        let bounds = layout.bounds();
        self.floating
            .as_widget_mut()
            .overlay(self.state, layout, renderer, &bounds, Vector::ZERO)
    }
}

fn should_capture_floating_overlay_event(
    event: &Event,
    cursor: mouse::Cursor,
    bounds: Rectangle,
) -> bool {
    is_mouse_event(event) && cursor.is_over(bounds)
}

fn floating_max_size(placement: FloatingPlacement, bounds: Size) -> Size {
    match placement {
        FloatingPlacement::Free(_) | FloatingPlacement::AnchorBottomRight { .. } => bounds,
        FloatingPlacement::Center
        | FloatingPlacement::At(_)
        | FloatingPlacement::BottomLeft { .. }
        | FloatingPlacement::BottomRightInArea { .. } => Size::new(
            (bounds.width - FLOATING_SURFACE_MARGIN * 2.0).max(0.0),
            (bounds.height - FLOATING_SURFACE_MARGIN * 2.0).max(0.0),
        ),
    }
}

fn floating_bounds(placement: FloatingPlacement, size: Size, surface: Size) -> Rectangle {
    Rectangle::new(floating_position(placement, size, surface), size)
}

fn floating_position(placement: FloatingPlacement, size: Size, surface: Size) -> Point {
    let desired = match placement {
        FloatingPlacement::Center => Point::new(
            (surface.width - size.width) / 2.0,
            (surface.height - size.height) / 2.0,
        ),
        FloatingPlacement::At(position) | FloatingPlacement::Free(position) => position,
        FloatingPlacement::AnchorBottomRight { anchor } => Point::new(
            anchor.x - size.width,
            anchor.y - size.height,
        ),
        FloatingPlacement::BottomLeft { left, bottom } => {
            Point::new(left, surface.height - bottom - size.height)
        }
        FloatingPlacement::BottomRightInArea {
            area_width,
            right,
            bottom,
        } => Point::new(
            area_width - right - size.width,
            surface.height - bottom - size.height,
        ),
    };
    if matches!(
        placement,
        FloatingPlacement::Free(_) | FloatingPlacement::AnchorBottomRight { .. }
    ) {
        return desired;
    }

    let max_x = (surface.width - size.width - FLOATING_SURFACE_MARGIN).max(FLOATING_SURFACE_MARGIN);
    let max_y =
        (surface.height - size.height - FLOATING_SURFACE_MARGIN).max(FLOATING_SURFACE_MARGIN);
    Point::new(
        desired.x.max(FLOATING_SURFACE_MARGIN).min(max_x),
        desired.y.max(FLOATING_SURFACE_MARGIN).min(max_y),
    )
}

#[cfg(test)]
mod tests;
