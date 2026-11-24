//! 窗口根部决定「可点的控件」显示箭头还是手形。
//!
//! iced 的 `button` 把手形写死在自己的 `mouse_interaction` 里，全程序有几百处按钮，
//! 另有二十来处显式 `.interaction(Pointer)`；逐处改就是把同一条政策撒在各个控件里。
//! 所以政策只在窗口根部有一个主人：根部包一层 [`control_pointer`]，把子树报上来的
//! `Pointer` 按设置改成 `Idle`，各控件完全不知道这件事。
//!
//! `Interaction` 没有单独的「链接」变体，真链接和按钮在子树里都是 `Pointer`，根部分不出来。
//! 所以真链接自己在返回 `Pointer` 时调 [`LinkPointer::mark_hovered`] 报到，根部看到有人报到
//! 就保留手形。
//!
//! 这一层两种设置下都存在：根部控件类型一变，iced 会把每个窗口的部件状态（滚动位置、
//! 输入焦点）整棵重建，切设置不能有这个代价。

use std::cell::Cell;
use std::rc::Rc;

use iced::advanced::{layout, overlay, renderer, widget, Clipboard, Layout, Shell, Widget};
use iced::{mouse, Element, Event, Length, Rectangle, Size, Vector};

/// 可点的控件（按钮、行、标签页）上显示什么指针。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ControlPointer {
    #[default]
    Arrow,
    Hand,
}

impl ControlPointer {
    /// 设置里存的是「可点的地方要不要手形」这一个开关，换算只在这里。
    pub fn from_hand_setting(hand_on_controls: bool) -> Self {
        if hand_on_controls {
            Self::Hand
        } else {
            Self::Arrow
        }
    }
}

/// 「这一轮询问里有链接在指针下」的信号：只在同一次 `mouse_interaction` 询问期间有意义，
/// 每一轮由 [`control_pointer`] 在询问前清掉，不会把上一轮的结果带到下一轮。
#[derive(Debug, Clone, Default)]
pub struct LinkPointer(Rc<Cell<bool>>);

impl LinkPointer {
    /// 链接部件在自己的 `mouse_interaction` 里、返回 `Pointer` 的那一刻调用。
    pub fn mark_hovered(&self) {
        self.0.set(true);
    }
}

/// 根部包装和它的浮层包装共用的同一条映射，保证菜单、提示气泡与窗口内容遵守同一设置。
#[derive(Clone)]
struct PointerPolicy {
    controls: ControlPointer,
    links: LinkPointer,
}

impl PointerPolicy {
    fn apply(&self, ask_subtree: impl FnOnce() -> mouse::Interaction) -> mouse::Interaction {
        self.links.0.set(false);
        let interaction = ask_subtree();
        match (self.controls, interaction) {
            (ControlPointer::Arrow, mouse::Interaction::Pointer) if !self.links.0.get() => {
                mouse::Interaction::Idle
            }
            _ => interaction,
        }
    }
}

/// 包住一个窗口的整棵界面，让可点的控件按 `controls` 显示指针；`links` 里报到的链接始终是手形。
pub fn control_pointer<'a, Message, Theme, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
    controls: ControlPointer,
    links: LinkPointer,
) -> Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    Element::new(ControlPointerRoot {
        content: content.into(),
        policy: PointerPolicy { controls, links },
    })
}

struct ControlPointerRoot<'a, Message, Theme, Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
    policy: PointerPolicy,
}

impl<'a, Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for ControlPointerRoot<'a, Message, Theme, Renderer>
where
    Message: 'a,
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
        self.policy.apply(|| {
            self.content.as_widget().mouse_interaction(
                &tree.children[0],
                layout,
                cursor,
                viewport,
                renderer,
            )
        })
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
        let policy = self.policy.clone();
        self.content
            .as_widget_mut()
            .overlay(
                &mut tree.children[0],
                layout,
                renderer,
                viewport,
                translation,
            )
            .map(|content| follow_policy(content, policy))
    }
}

fn follow_policy<'a, Message, Theme, Renderer>(
    content: overlay::Element<'a, Message, Theme, Renderer>,
    policy: PointerPolicy,
) -> overlay::Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    overlay::Element::new(Box::new(ControlPointerOverlay { content, policy }))
}

/// 浮层里的按钮同样会报 `Pointer`；嵌套的浮层（菜单里再弹出的子菜单）由 `overlay` 逐层包上。
struct ControlPointerOverlay<'a, Message, Theme, Renderer> {
    content: overlay::Element<'a, Message, Theme, Renderer>,
    policy: PointerPolicy,
}

impl<'a, Message, Theme, Renderer> overlay::Overlay<Message, Theme, Renderer>
    for ControlPointerOverlay<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    fn layout(&mut self, renderer: &Renderer, bounds: Size) -> layout::Node {
        self.content.as_overlay_mut().layout(renderer, bounds)
    }

    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
    ) {
        self.content
            .as_overlay()
            .draw(renderer, theme, style, layout, cursor);
    }

    fn operate(
        &mut self,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.content
            .as_overlay_mut()
            .operate(layout, renderer, operation);
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
        self.content
            .as_overlay_mut()
            .update(event, layout, cursor, renderer, clipboard, shell);
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.policy.apply(|| {
            self.content
                .as_overlay()
                .mouse_interaction(layout, cursor, renderer)
        })
    }

    fn overlay<'b>(
        &'b mut self,
        layout: Layout<'b>,
        renderer: &Renderer,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        let policy = self.policy.clone();
        self.content
            .as_overlay_mut()
            .overlay(layout, renderer)
            .map(|nested| follow_policy(nested, policy))
    }

    /// 同一层里的先后由 `index` 定，包一层不能让提示气泡之类掉到菜单下面。
    fn index(&self) -> f32 {
        self.content.as_overlay().index()
    }
}

#[cfg(test)]
#[path = "control_pointer_tests.rs"]
mod tests;
