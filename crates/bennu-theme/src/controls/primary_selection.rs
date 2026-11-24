//! 输入框的 PRIMARY 选区：选中的文字自动进 PRIMARY，中键点输入框粘贴 PRIMARY。
//!
//! iced 的 text_input / text_editor 只读写标准剪贴板，所以每个输入框外面包一层本部件：
//! - 选区定下来才写 PRIMARY（按下、松开鼠标、按键之后）。拖选途中不写，否则指针每动一下
//!   就写一次剪贴板。
//! - 中键点在输入框上时，把 PRIMARY 当作 Ctrl+V 的内容交给输入框自己粘贴。点击位置的命中、
//!   聚焦、光标移动都由输入框自己做；粘贴走它原本的 `on_input` / `on_paste` / `on_action`，
//!   应用看到的是一次普通编辑。
//! - 聚焦时向 widget operation 补报 [`FocusedTextControl`]，让按键路由认得出多行文字控件。
//! - 聚焦的文字控件因 Esc 失焦时捕获这次事件，单行、多行一个样。
//! - 在控件里按下左键拖选时，指针出了控件也照样跟着选（见 [`drag_cursor`]）。

use std::marker::PhantomData;

use iced::advanced::clipboard::{self, Clipboard};
use iced::advanced::widget::{self, tree, Tree};
use iced::advanced::{layout, mouse, overlay, renderer, text, Layout, Shell, Widget};
use iced::keyboard::{self, key};
use iced::widget::text_editor::{self, Content};
use iced::widget::text_input::{self, Value};
use iced::window;
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector};

/// 被包输入控件的选区读法：选区存在各控件自己的内部状态里，读法各不相同。
pub(super) trait SelectionSource {
    /// 键盘焦点在这个输入控件上。别的输入框里残留的选区不是用户刚选的，不能写进 PRIMARY。
    fn is_focused(&self, inner: &Tree) -> bool;

    /// 选中的文字；只有光标没有选区时为 `None`。
    fn selected_text(&self, inner: &Tree) -> Option<String>;
}

/// 单行输入框：选区是内部状态里的光标区间，要拿当前文字换算成字符串。
pub(super) struct TextInputSelection<Paragraph> {
    value: String,
    paragraph: PhantomData<fn() -> Paragraph>,
}

impl<Paragraph> TextInputSelection<Paragraph> {
    pub(super) fn new(value: String) -> Self {
        Self {
            value,
            paragraph: PhantomData,
        }
    }
}

impl<Paragraph: text::Paragraph + 'static> SelectionSource for TextInputSelection<Paragraph> {
    fn is_focused(&self, inner: &Tree) -> bool {
        inner
            .state
            .downcast_ref::<text_input::State<Paragraph>>()
            .is_focused()
    }

    fn selected_text(&self, inner: &Tree) -> Option<String> {
        let state = inner.state.downcast_ref::<text_input::State<Paragraph>>();
        let value = Value::new(&self.value);
        let (start, end) = state.cursor().selection(&value)?;
        Some(value.select(start, end).to_string())
    }
}

/// 多行输入区：选区在应用持有的 `Content` 里。
pub(super) struct TextEditorSelection<'a, Renderer: text::Renderer> {
    pub(super) content: &'a Content<Renderer>,
}

impl<Renderer: text::Renderer> SelectionSource for TextEditorSelection<'_, Renderer> {
    fn is_focused(&self, inner: &Tree) -> bool {
        inner
            .state
            .downcast_ref::<text_editor::State<text::highlighter::PlainText>>()
            .is_focused()
    }

    fn selected_text(&self, _inner: &Tree) -> Option<String> {
        self.content.selection()
    }
}

/// 键盘焦点在文字控件上时，向 widget operation 报告的标记。
///
/// iced 的 `TextEditor` 只报告“可聚焦”，不像 `TextInput` 那样再报告“是文字输入”，按键路由
/// 单凭这个认不出多行文字控件在用键盘。所有文字控件都包着本部件，由它统一补报。
pub struct FocusedTextControl;

pub(super) struct PrimarySelection<'a, Message, Theme, Renderer> {
    pub(super) inner: Element<'a, Message, Theme, Renderer>,
    /// 密码框是 `None`：选区永远不外泄。
    pub(super) selection: Option<Box<dyn SelectionSource + 'a>>,
    /// 被包控件接受输入。禁用的输入框（没挂 `on_input`）中键不粘贴，也不会被中键聚焦。
    pub(super) accepts_paste: bool,
}

#[derive(Default)]
struct PrimaryState {
    /// 刚发生过会定下选区的操作，等下一次重绘再读选区。
    ///
    /// 不在事件里当场读：text_editor 只发 `Action`，选区要等应用处理完消息、重建视图后才变；
    /// iced 每条消息之后都会重绘，所以重绘时读到的一定是处理完的结果。
    selection_settled: bool,
    /// 上一次定下来时看到的选区（没有选区是 `None`）。
    ///
    /// 比较的是“选区变没变”而不是“写过什么”：别的程序抢走 PRIMARY 之后，这里清掉选区再
    /// 选回同一段文字，仍然要写回去；选区没变时（比如只是按 Ctrl+C）不能把别人的 PRIMARY 抢回来。
    last_selection: Option<String>,
    /// 真实的修饰键。text_input 自己记着修饰键，合成 Ctrl+V 之后要改回来。
    modifiers: keyboard::Modifiers,
    /// 左键是在本控件里按下的，还没松开。
    left_held_inside: bool,
}

/// 读标准剪贴板的人拿到 PRIMARY 的文字，其余照旧走真实剪贴板。
struct PrimaryAsStandard<'a> {
    primary: String,
    real: &'a mut dyn Clipboard,
}

impl Clipboard for PrimaryAsStandard<'_> {
    fn read(&self, kind: clipboard::Kind) -> Option<String> {
        match kind {
            clipboard::Kind::Standard => Some(self.primary.clone()),
            clipboard::Kind::Primary => self.real.read(kind),
        }
    }

    fn write(&mut self, kind: clipboard::Kind, contents: String) {
        self.real.write(kind, contents);
    }
}

/// “在点击处粘贴”要送进输入框的事件：按住粘贴的修饰键，左键点一下（聚焦并把光标移到点击处），
/// 按 V，最后把修饰键改回真实状态。
///
/// 要补上 V 的松开：text_input 在 V 松开之前记着“正在连续粘贴”，不松开的话用户下一次真正的
/// Ctrl+V 会把这里的 PRIMARY 再贴一遍。
///
/// 局限：合成的左键点击和用户上一次真实左键点击间隔不到 300 毫秒、位置也相近时，输入框会把它
/// 算成双击而选词。
fn click_and_paste(real_modifiers: keyboard::Modifiers) -> [Event; 6] {
    let modifiers = keyboard::Modifiers::COMMAND;
    let paste_key = keyboard::Key::Character("v".into());
    let physical_key = key::Physical::Code(key::Code::KeyV);
    [
        Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)),
        left_press(),
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
        Event::Keyboard(keyboard::Event::KeyPressed {
            key: paste_key.clone(),
            modified_key: paste_key.clone(),
            physical_key,
            location: keyboard::Location::Standard,
            modifiers,
            text: None,
            repeat: false,
        }),
        Event::Keyboard(keyboard::Event::KeyReleased {
            key: paste_key.clone(),
            modified_key: paste_key,
            physical_key,
            location: keyboard::Location::Standard,
            modifiers,
        }),
        Event::Keyboard(keyboard::Event::ModifiersChanged(real_modifiers)),
    ]
}

fn left_press() -> Event {
    Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
}

/// 鼠标按下、松开和按键之后选区可能刚定下来；`CursorMoved` 不算，拖选途中不写。
fn may_settle_selection(event: &Event) -> bool {
    matches!(
        event,
        Event::Mouse(mouse::Event::ButtonPressed(_) | mouse::Event::ButtonReleased(_))
            | Event::Keyboard(keyboard::Event::KeyPressed { .. })
    )
}

/// 交给被包控件的指针。
///
/// iced 的 `TextEditor` 拖选只认落在自己范围里的指针，一出范围选区就停在那儿。信息面板的值
/// 只有一行高，往上下行一拖就出界，看上去选区不跟手。所以在本控件里按下的左键没松开时，
/// 把指针换成范围里最近的一点：拖到上面选到开头，拖到下面选到结尾，左右出界夹到两边。
/// 单行输入框拖选读的是事件里的位置，不看这个指针，换了也一样。
fn drag_cursor(
    state: &mut PrimaryState,
    event: &Event,
    bounds: Rectangle,
    cursor: mouse::Cursor,
) -> mouse::Cursor {
    match event {
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
            state.left_held_inside = cursor.is_over(bounds);
        }
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
            state.left_held_inside = false;
        }
        Event::Mouse(mouse::Event::CursorMoved { .. }) if state.left_held_inside => {
            if let Some(position) = cursor.position() {
                return mouse::Cursor::Available(nearest_point_inside(bounds, position));
            }
        }
        _ => {}
    }
    cursor
}

/// 范围里离 `point` 最近、又照“上面算开头、下面算结尾”取的那一点。范围的右边和下边不算在
/// 里面（`Rectangle::contains` 是半开的），所以往里收半个像素。
fn nearest_point_inside(bounds: Rectangle, point: Point) -> Point {
    let right = (bounds.x + bounds.width - 0.5).max(bounds.x);
    let bottom = (bounds.y + bounds.height - 0.5).max(bounds.y);
    if point.y < bounds.y {
        Point::new(bounds.x, bounds.y)
    } else if point.y >= bounds.y + bounds.height {
        Point::new(right, bottom)
    } else {
        Point::new(point.x.clamp(bounds.x, right), point.y)
    }
}

fn is_escape_press(event: &Event) -> bool {
    matches!(
        event,
        Event::Keyboard(keyboard::Event::KeyPressed {
            key: keyboard::Key::Named(key::Named::Escape),
            ..
        })
    )
}

impl<Message, Theme, Renderer> PrimarySelection<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn is_focused(&self, tree: &Tree) -> bool {
        self.selection
            .as_ref()
            .is_some_and(|source| source.is_focused(&tree.children[0]))
    }

    fn write_settled_selection(
        &self,
        tree: &mut Tree,
        event: &Event,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
    ) {
        let Some(source) = &self.selection else {
            return;
        };
        let state = tree.state.downcast_mut::<PrimaryState>();
        if may_settle_selection(event) {
            if source.is_focused(&tree.children[0]) {
                state.selection_settled = true;
                shell.request_redraw();
            }
        } else if state.selection_settled
            && matches!(event, Event::Window(window::Event::RedrawRequested(_)))
        {
            state.selection_settled = false;
            let selection = source.selected_text(&tree.children[0]);
            if selection != state.last_selection {
                if let Some(text) = &selection {
                    clipboard.write(clipboard::Kind::Primary, text.clone());
                }
                state.last_selection = selection;
            }
        }
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for PrimarySelection<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<PrimaryState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(PrimaryState::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.inner)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.inner));
    }

    fn size(&self) -> Size<Length> {
        self.inner.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.inner.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.inner
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.inner
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
        if self.is_focused(tree) {
            operation.custom(None, layout.bounds(), &mut FocusedTextControl);
        }
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
        match event {
            Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
                tree.state.downcast_mut::<PrimaryState>().modifiers = *modifiers;
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Middle)) => {
                if !cursor.is_over(layout.bounds()) {
                    // 输入框靠左键点击得到和失去焦点。中键粘贴的目标是靠合成的左键点击聚焦的，
                    // 别的输入框看不到那次点击；中键点在别处不照左键点在外面的样子失焦，
                    // 就会有两个输入框同时有焦点，打字同时进两边。
                    self.inner.as_widget_mut().update(
                        &mut tree.children[0],
                        &left_press(),
                        layout,
                        cursor,
                        renderer,
                        clipboard,
                        shell,
                        viewport,
                    );
                } else if self.accepts_paste && !shell.is_event_captured() {
                    // 能粘贴的输入框拥有落在自己身上的中键：PRIMARY 为空时也不能让后面的控件
                    // （比如文件条目的中键动作）收到。
                    if let Some(text) = clipboard
                        .read(clipboard::Kind::Primary)
                        .filter(|text| !text.is_empty())
                    {
                        let real_modifiers = tree.state.downcast_ref::<PrimaryState>().modifiers;
                        let mut clipboard = PrimaryAsStandard {
                            primary: text,
                            real: clipboard,
                        };
                        for event in click_and_paste(real_modifiers) {
                            self.inner.as_widget_mut().update(
                                &mut tree.children[0],
                                &event,
                                layout,
                                cursor,
                                renderer,
                                &mut clipboard,
                                shell,
                                viewport,
                            );
                        }
                    }
                    shell.capture_event();
                    return;
                }
            }
            _ => {}
        }
        let focused_before = self.is_focused(tree);
        let cursor = drag_cursor(
            tree.state.downcast_mut::<PrimaryState>(),
            event,
            layout.bounds(),
            cursor,
        );
        self.inner.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
        // 聚焦的单行输入框按 Esc 失焦并捕获事件，多行的 `TextEditor` 失焦却不捕获。应用靠“被
        // 捕获”认出这一下 Esc 是文字控件吃掉的（只是离开控件，不再往下清选择、关窗口），
        // 所以所有文字控件在这里统一成捕获。
        if is_escape_press(event) && focused_before && !self.is_focused(tree) {
            shell.capture_event();
        }
        self.write_settled_selection(tree, event, clipboard, shell);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.inner.as_widget().mouse_interaction(
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
        self.inner.as_widget().draw(
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
        self.inner.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}
