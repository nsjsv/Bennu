use iced::advanced::{layout, overlay, renderer, widget, Clipboard, Layout, Shell, Widget};
use iced::mouse;
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector};

const POPUP_GAP: f32 = 4.0;

pub fn anchored_popup<'a, Message>(
    anchor: impl Into<Element<'a, Message>>,
    popup: Option<Element<'a, Message>>,
) -> Element<'a, Message>
where
    Message: 'a,
{
    Element::new(AnchoredPopup {
        anchor: anchor.into(),
        popup,
        placement: PopupPlacement::AnchorWidth,
        on_outside_press: None,
    })
}

/// 向下展开的变体:弹层不受锚点宽度限制(内容自然定宽),右缘对齐锚点,
/// 空间不足也不翻到上方(顶多被窗口下缘钳住)。适合按钮旁的动作菜单。
pub fn anchored_popup_expanding<'a, Message>(
    anchor: impl Into<Element<'a, Message>>,
    popup: Option<Element<'a, Message>>,
) -> Element<'a, Message>
where
    Message: 'a,
{
    Element::new(AnchoredPopup {
        anchor: anchor.into(),
        popup,
        placement: PopupPlacement::RightAligned,
        on_outside_press: None,
    })
}

/// 下拉按钮的变体：弹层按内容定宽（至少和按钮一样宽），左缘对齐按钮，总是向下展开
/// （顶多被窗口边缘钳住）。适合一排筛选按钮各自的下拉。
pub fn anchored_dropdown<'a, Message>(
    anchor: impl Into<Element<'a, Message>>,
    popup: Option<Element<'a, Message>>,
) -> Element<'a, Message>
where
    Message: 'a,
{
    Element::new(AnchoredPopup {
        anchor: anchor.into(),
        popup,
        placement: PopupPlacement::Dropdown,
        on_outside_press: None,
    })
}

/// 向上展开的变体：弹层按内容定宽（至少和按钮一样宽），右缘对齐按钮，总是在按钮上方
/// （顶多被窗口边缘钳住）。适合贴着窗口底边的按钮（视频控制条上的字幕、音轨菜单）。
pub fn anchored_dropup<'a, Message>(
    anchor: impl Into<Element<'a, Message>>,
    popup: Option<Element<'a, Message>>,
) -> Element<'a, Message>
where
    Message: 'a,
{
    Element::new(AnchoredPopup {
        anchor: anchor.into(),
        popup,
        placement: PopupPlacement::DropUp,
        on_outside_press: None,
    })
}

/// 贴着底栏的菜单按钮：摆法同 [`anchored_dropup`]，另外在弹层和按钮以外按下鼠标时发
/// `on_dismiss` 并吞掉这一下——菜单的通行规矩，点外面只是把菜单收起来，不顺带点到下面的东西。
pub fn anchored_dropup_menu<'a, Message>(
    anchor: impl Into<Element<'a, Message>>,
    popup: Option<Element<'a, Message>>,
    on_dismiss: impl Fn() -> Message + 'a,
) -> Element<'a, Message>
where
    Message: 'a,
{
    Element::new(AnchoredPopup {
        anchor: anchor.into(),
        popup,
        placement: PopupPlacement::DropUp,
        on_outside_press: Some(Box::new(on_dismiss)),
    })
}

/// 弹层相对锚点怎么摆。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PopupPlacement {
    /// 和锚点一样宽、左对齐，哪边空间大往哪边展开（输入框下的建议列表）。
    AnchorWidth,
    /// 自然宽度、右缘对齐锚点、向下（按钮旁的动作菜单）。
    RightAligned,
    /// 自然宽度（不窄于锚点）、左缘对齐锚点、向下。
    Dropdown,
    /// 自然宽度（不窄于锚点）、右缘对齐锚点、向上。
    DropUp,
}

struct AnchoredPopup<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    anchor: Element<'a, Message, Theme, Renderer>,
    popup: Option<Element<'a, Message, Theme, Renderer>>,
    placement: PopupPlacement,
    /// 弹层开着时在它和锚点以外按下鼠标发的消息（菜单收起来）；`None` 的弹层不管外面的点击。
    on_outside_press: Option<Box<dyn Fn() -> Message + 'a>>,
}

impl<'a, Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for AnchoredPopup<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    fn children(&self) -> Vec<widget::Tree> {
        let mut children = vec![widget::Tree::new(&self.anchor)];
        if let Some(popup) = &self.popup {
            children.push(widget::Tree::new(popup));
        }
        children
    }

    fn diff(&self, tree: &mut widget::Tree) {
        let mut children = Vec::with_capacity(1 + usize::from(self.popup.is_some()));
        children.push(self.anchor.as_widget());
        if let Some(popup) = &self.popup {
            children.push(popup.as_widget());
        }
        tree.diff_children(&children);
    }

    fn size(&self) -> Size<Length> {
        self.anchor.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.anchor
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
        self.anchor
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
        self.anchor.as_widget_mut().update(
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
        self.anchor.as_widget().mouse_interaction(
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
        self.anchor.as_widget().draw(
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
        let mut overlays = Vec::new();
        let mut children = tree.children.iter_mut();

        if let Some(anchor_tree) = children.next() {
            if let Some(anchor_overlay) = self.anchor.as_widget_mut().overlay(
                anchor_tree,
                layout,
                renderer,
                viewport,
                translation,
            ) {
                overlays.push(anchor_overlay);
            }
        }

        if let (Some(popup), Some(popup_tree)) = (self.popup.as_mut(), children.next()) {
            let anchor_bounds =
                Rectangle::new(layout.position() + translation, layout.bounds().size());
            overlays.push(overlay::Element::new(Box::new(AnchoredPopupOverlay {
                popup,
                state: popup_tree,
                anchor_bounds,
                placement: self.placement,
                on_outside_press: self.on_outside_press.as_deref(),
            })));
        }

        (!overlays.is_empty()).then(|| overlay::Group::with_children(overlays).overlay())
    }
}

struct AnchoredPopupOverlay<'a, 'b, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    popup: &'b mut Element<'a, Message, Theme, Renderer>,
    state: &'b mut widget::Tree,
    anchor_bounds: Rectangle,
    placement: PopupPlacement,
    on_outside_press: Option<&'b (dyn Fn() -> Message + 'a)>,
}

impl<'a, 'b, Message, Theme, Renderer> overlay::Overlay<Message, Theme, Renderer>
    for AnchoredPopupOverlay<'a, 'b, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    fn layout(&mut self, renderer: &Renderer, bounds: Size) -> layout::Node {
        let target_width = self.anchor_bounds.width.min(bounds.width).max(0.0);
        let below_y = self.anchor_bounds.y + self.anchor_bounds.height + POPUP_GAP;
        let space_below = (bounds.height - below_y).max(0.0);
        let space_above = (self.anchor_bounds.y - POPUP_GAP).max(0.0);
        let show_below = match self.placement {
            PopupPlacement::AnchorWidth => space_below >= space_above,
            PopupPlacement::RightAligned | PopupPlacement::Dropdown => true,
            PopupPlacement::DropUp => false,
        };
        let max_height = if show_below { space_below } else { space_above };
        let limits = match self.placement {
            PopupPlacement::AnchorWidth => {
                layout::Limits::new(Size::ZERO, Size::new(target_width, max_height))
                    .width(target_width)
            }
            // 弹层自然宽度，只受窗口宽度约束。
            PopupPlacement::RightAligned => {
                layout::Limits::new(Size::ZERO, Size::new(bounds.width, max_height))
            }
            PopupPlacement::Dropdown | PopupPlacement::DropUp => layout::Limits::new(
                Size::new(target_width, 0.0),
                Size::new(bounds.width, max_height),
            ),
        };
        let node = self
            .popup
            .as_widget_mut()
            .layout(self.state, renderer, &limits);
        let size = node.size();
        let desired_x = match self.placement {
            PopupPlacement::RightAligned | PopupPlacement::DropUp => {
                self.anchor_bounds.x + self.anchor_bounds.width - size.width
            }
            PopupPlacement::AnchorWidth | PopupPlacement::Dropdown => self.anchor_bounds.x,
        };
        let x = desired_x.max(0.0).min((bounds.width - size.width).max(0.0));
        let desired_y = if show_below {
            below_y
        } else {
            self.anchor_bounds.y - POPUP_GAP - size.height
        };
        let y = desired_y
            .max(0.0)
            .min((bounds.height - size.height).max(0.0));
        node.move_to(Point::new(x, y))
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
        if let (Some(on_outside_press), Event::Mouse(mouse::Event::ButtonPressed(_))) =
            (self.on_outside_press, event)
        {
            let pressed_outside = cursor.position().is_some_and(|position| {
                !bounds.contains(position) && !self.anchor_bounds.contains(position)
            });
            if pressed_outside {
                shell.publish(on_outside_press());
                shell.capture_event();
                return;
            }
        }

        self.popup.as_widget_mut().update(
            self.state, event, layout, cursor, renderer, clipboard, shell, &bounds,
        )
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let bounds = layout.bounds();
        self.popup
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
        self.popup
            .as_widget()
            .draw(self.state, renderer, theme, style, layout, cursor, &bounds);
    }

    fn operate(
        &mut self,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.popup
            .as_widget_mut()
            .operate(self.state, layout, renderer, operation);
    }

    fn overlay<'c>(
        &'c mut self,
        layout: Layout<'c>,
        renderer: &Renderer,
    ) -> Option<overlay::Element<'c, Message, Theme, Renderer>> {
        let bounds = layout.bounds();
        self.popup
            .as_widget_mut()
            .overlay(self.state, layout, renderer, &bounds, Vector::ZERO)
    }
}

/// 弹出菜单"点外面就关"（同 GTK 弹出菜单）：鼠标在自身范围外按下时发出
/// `on_dismiss`。不拦截这次按下，点到的其它控件照常响应。
pub fn dismiss_on_outside_press<'a, Message>(
    content: impl Into<Element<'a, Message>>,
    on_dismiss: Message,
) -> Element<'a, Message>
where
    Message: Clone + 'a,
{
    Element::new(DismissOnOutsidePress {
        content: content.into(),
        on_dismiss,
    })
}

struct DismissOnOutsidePress<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
    on_dismiss: Message,
}

impl<'a, Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for DismissOnOutsidePress<'a, Message, Theme, Renderer>
where
    Message: Clone + 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    fn children(&self) -> Vec<widget::Tree> {
        vec![widget::Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut widget::Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
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
        if matches!(event, Event::Mouse(mouse::Event::ButtonPressed(_)))
            && !cursor.is_over(layout.bounds())
        {
            shell.publish(self.on_dismiss.clone());
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
}

#[cfg(test)]
mod tests {
    use iced::advanced::clipboard;
    use iced::widget::Space;

    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    enum Message {
        Dismissed,
    }

    struct NoClipboard;

    impl Clipboard for NoClipboard {
        fn read(&self, _kind: clipboard::Kind) -> Option<String> {
            None
        }

        fn write(&mut self, _kind: clipboard::Kind, _contents: String) {}
    }

    fn sized(width: f32, height: f32) -> Element<'static, Message, iced::Theme, ()> {
        Space::new()
            .width(Length::Fixed(width))
            .height(Length::Fixed(height))
            .into()
    }

    /// 底栏上的菜单按钮（窗口 400×400，按钮在 (20, 360)，弹层向上开）：按一下鼠标，
    /// 返回发出的消息和事件有没有被吞掉。
    fn press_at(point: Point) -> (Vec<Message>, bool) {
        let mut popup: AnchoredPopup<'static, Message, iced::Theme, ()> = AnchoredPopup {
            anchor: sized(60.0, 24.0),
            popup: Some(sized(120.0, 200.0)),
            placement: PopupPlacement::DropUp,
            on_outside_press: Some(Box::new(|| Message::Dismissed)),
        };
        let mut tree = widget::Tree::new(&popup as &dyn Widget<Message, iced::Theme, ()>);
        let limits = layout::Limits::new(Size::ZERO, Size::new(400.0, 400.0));
        let node = popup
            .layout(&mut tree, &(), &limits)
            .move_to(Point::new(20.0, 360.0));
        let window = Rectangle::new(Point::ORIGIN, Size::new(400.0, 400.0));
        let mut overlay = popup
            .overlay(&mut tree, Layout::new(&node), &(), &window, Vector::ZERO)
            .expect("an open popup has an overlay");
        let overlay_node = overlay.as_overlay_mut().layout(&(), window.size());
        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);
        overlay.as_overlay_mut().update(
            &Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            Layout::new(&overlay_node),
            mouse::Cursor::Available(point),
            &(),
            &mut NoClipboard,
            &mut shell,
        );
        let captured = shell.is_event_captured();
        (messages, captured)
    }

    #[test]
    fn a_press_outside_the_menu_dismisses_it_and_goes_no_further() {
        assert_eq!(
            press_at(Point::new(300.0, 50.0)),
            (vec![Message::Dismissed], true)
        );
    }

    #[test]
    fn presses_on_the_menu_or_its_button_are_left_to_them() {
        // 弹层在按钮上方：右缘对齐按钮右缘（80），底边在按钮上沿往上一个间隙。
        assert_eq!(press_at(Point::new(40.0, 250.0)), (vec![], false));
        assert_eq!(press_at(Point::new(40.0, 370.0)), (vec![], false));
    }
}
