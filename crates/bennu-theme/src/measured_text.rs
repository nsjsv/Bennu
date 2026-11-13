use std::ops::Range;

use iced::advanced::text::Paragraph;
use iced::advanced::{layout, renderer, text, widget, Layout, Widget};
use iced::{alignment, Element, Font, Length, Pixels, Point, Rectangle, Size};

mod emphasis;
mod tooltip;

pub use tooltip::measured_middle_ellipsized_wrapped_text_with_tooltip;

const ELLIPSIS_MARKER: &str = "...";
const TEXT_MEASUREMENT_WIDTH: f32 = 100_000.0;
const TEXT_MEASUREMENT_HEIGHT: f32 = 10_000.0;

// 多栏列表等按最终像素宽度截断；这里必须等布局拿到 renderer 后再决定是否省略。
pub fn measured_middle_ellipsized_text<'a, Message>(
    content: impl Into<String>,
    size: f32,
) -> Element<'a, Message>
where
    Message: 'a,
{
    Element::new(
        MeasuredMiddleEllipsizedText::new(content)
            .size(size)
            .width(Length::Fill),
    )
}

/// 有加粗片段的单行变体：`emphasized` 是原文里的字节区间（按顺序、不重叠、落在字符边界上），
/// 省略后只加粗还看得见的部分。
pub fn measured_middle_ellipsized_emphasized_text<'a, Message>(
    content: impl Into<String>,
    emphasized: Vec<Range<usize>>,
    size: f32,
) -> Element<'a, Message>
where
    Message: 'a,
{
    let mut text = MeasuredMiddleEllipsizedText::new(content)
        .size(size)
        .width(Length::Fill);
    text.emphasized = emphasized;
    Element::new(text)
}

/// 网格瓦片用的居中变体:与带 tooltip 版的文本配置一致(对齐/换行/行高),
/// 只去掉悬停浮层——占位行等纯展示场景用。
pub fn measured_middle_ellipsized_centered_text<'a, Message>(
    content: impl Into<String>,
    size: f32,
    line_height_pixels: f32,
) -> Element<'a, Message>
where
    Message: 'a,
{
    Element::new(
        MeasuredMiddleEllipsizedText::file_name(content)
            .size(size)
            .line_height(text::LineHeight::Absolute(Pixels(line_height_pixels)))
            .wrapping(text::Wrapping::WordOrGlyph)
            .align_x(text::Alignment::Center)
            .width(Length::Fill)
            .height(Length::Fill),
    )
}

/// 居中变体的加粗版：网格里搜索结果的名字，搜到的字加粗。
pub fn measured_middle_ellipsized_centered_emphasized_text<'a, Message>(
    content: impl Into<String>,
    emphasized: Vec<Range<usize>>,
    size: f32,
    line_height_pixels: f32,
) -> Element<'a, Message>
where
    Message: 'a,
{
    let mut text = MeasuredMiddleEllipsizedText::file_name(content)
        .size(size)
        .line_height(text::LineHeight::Absolute(Pixels(line_height_pixels)))
        .wrapping(text::Wrapping::WordOrGlyph)
        .align_x(text::Alignment::Center)
        .width(Length::Fill)
        .height(Length::Fill);
    text.emphasized = emphasized;
    Element::new(text)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MiddleEllipsisKind {
    GeneralText,
    FileName,
}

#[derive(Debug, Clone)]
struct MeasuredMiddleEllipsizedText {
    content: String,
    size: Pixels,
    width: Length,
    height: Length,
    line_height: text::LineHeight,
    wrapping: text::Wrapping,
    align_x: text::Alignment,
    ellipsis_kind: MiddleEllipsisKind,
    emphasized: Vec<Range<usize>>,
}

impl MeasuredMiddleEllipsizedText {
    fn new(content: impl Into<String>) -> Self {
        Self::with_ellipsis_kind(content, MiddleEllipsisKind::GeneralText)
    }

    fn file_name(content: impl Into<String>) -> Self {
        Self::with_ellipsis_kind(content, MiddleEllipsisKind::FileName)
    }

    fn with_ellipsis_kind(content: impl Into<String>, ellipsis_kind: MiddleEllipsisKind) -> Self {
        Self {
            content: content.into(),
            size: Pixels(16.0),
            width: Length::Shrink,
            height: Length::Shrink,
            line_height: text::LineHeight::default(),
            wrapping: text::Wrapping::None,
            align_x: text::Alignment::Left,
            ellipsis_kind,
            emphasized: Vec::new(),
        }
    }

    fn size(mut self, size: impl Into<Pixels>) -> Self {
        self.size = size.into();
        self
    }

    fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    fn line_height(mut self, line_height: text::LineHeight) -> Self {
        self.line_height = line_height;
        self
    }

    fn wrapping(mut self, wrapping: text::Wrapping) -> Self {
        self.wrapping = wrapping;
        self
    }

    fn align_x(mut self, align_x: text::Alignment) -> Self {
        self.align_x = align_x;
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
struct MeasuredTextLayoutKey<Font> {
    content: String,
    bounds: Size<f32>,
    size: Pixels,
    line_height: text::LineHeight,
    font: Font,
    shaping: text::Shaping,
    wrapping: text::Wrapping,
    align_x: text::Alignment,
    ellipsis_kind: MiddleEllipsisKind,
    emphasized: Vec<Range<usize>>,
}

impl<Font: PartialEq> MeasuredTextLayoutKey<Font> {
    // 参数集合即 iced 文本布局的完整输入，拆结构体只会把同一组字段搬家
    #[allow(clippy::too_many_arguments)]
    fn new(
        content: &str,
        bounds: Size<f32>,
        size: Pixels,
        line_height: text::LineHeight,
        font: Font,
        shaping: text::Shaping,
        wrapping: text::Wrapping,
        align_x: text::Alignment,
        ellipsis_kind: MiddleEllipsisKind,
        emphasized: &[Range<usize>],
    ) -> Self {
        Self {
            content: content.to_owned(),
            bounds,
            size,
            line_height,
            font,
            shaping,
            wrapping,
            align_x,
            ellipsis_kind,
            emphasized: emphasized.to_vec(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn matches(
        &self,
        content: &str,
        bounds: Size<f32>,
        size: Pixels,
        line_height: text::LineHeight,
        font: Font,
        shaping: text::Shaping,
        wrapping: text::Wrapping,
        align_x: text::Alignment,
        ellipsis_kind: MiddleEllipsisKind,
        emphasized: &[Range<usize>],
    ) -> bool {
        // 先比廉价标量字段，最后才比较整段字符串：尺寸未变时零字符串比较开销。
        self.bounds == bounds
            && self.size == size
            && self.line_height == line_height
            && self.font == font
            && self.shaping == shaping
            && self.wrapping == wrapping
            && self.align_x == align_x
            && self.ellipsis_kind == ellipsis_kind
            && self.emphasized == emphasized
            && self.content == content
    }
}

struct MeasuredMiddleEllipsizedTextState<Paragraph, Font> {
    paragraph: Paragraph,
    displayed_content: String,
    layout_key: Option<MeasuredTextLayoutKey<Font>>,
}

impl<Paragraph: Default, Font> Default for MeasuredMiddleEllipsizedTextState<Paragraph, Font> {
    fn default() -> Self {
        Self {
            paragraph: Paragraph::default(),
            displayed_content: String::new(),
            layout_key: None,
        }
    }
}

fn displayed_content_is_ellipsized(content: &str, displayed_content: &str) -> bool {
    displayed_content != content
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer> for MeasuredMiddleEllipsizedText
where
    Renderer: text::Renderer<Font = Font>,
{
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<
            MeasuredMiddleEllipsizedTextState<Renderer::Paragraph, Renderer::Font>,
        >()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(MeasuredMiddleEllipsizedTextState::<
            Renderer::Paragraph,
            Renderer::Font,
        >::default())
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: self.width,
            height: self.height,
        }
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let text_state = tree
            .state
            .downcast_mut::<MeasuredMiddleEllipsizedTextState<Renderer::Paragraph, Renderer::Font>>(
            );

        layout::sized(limits, self.width, self.height, |limits| {
            let bounds = limits.max();
            let font = renderer.default_font();
            let line_height = self.line_height;
            let shaping = shaping_for_content(&self.content);
            let display_bounds = Size::new(bounds.width, bounds.height);
            let layout_key_matches = text_state.layout_key.as_ref().is_some_and(|key| {
                key.matches(
                    &self.content,
                    display_bounds,
                    self.size,
                    line_height,
                    font,
                    shaping,
                    self.wrapping,
                    self.align_x,
                    self.ellipsis_kind,
                    &self.emphasized,
                )
            });

            if !layout_key_matches {
                let displayed_content = fit_middle_ellipsized_text::<Renderer>(
                    &self.content,
                    &self.emphasized,
                    bounds.width,
                    bounds.height,
                    self.size,
                    line_height,
                    font,
                    shaping,
                    self.wrapping,
                    self.ellipsis_kind,
                );
                let displayed_emphasis = emphasis::displayed_emphasis(
                    &self.content,
                    &displayed_content,
                    self.ellipsis_kind,
                    &self.emphasized,
                );

                text_state.displayed_content = displayed_content;
                text_state.paragraph = emphasis::emphasized_paragraph(
                    text::Text {
                        content: text_state.displayed_content.as_str(),
                        bounds: display_bounds,
                        size: self.size,
                        line_height,
                        font,
                        align_x: self.align_x,
                        align_y: alignment::Vertical::Top,
                        shaping,
                        wrapping: self.wrapping,
                    },
                    &displayed_emphasis,
                );
                text_state.layout_key = Some(MeasuredTextLayoutKey::new(
                    &self.content,
                    display_bounds,
                    self.size,
                    line_height,
                    font,
                    shaping,
                    self.wrapping,
                    self.align_x,
                    self.ellipsis_kind,
                    &self.emphasized,
                ));
            }

            text_state.paragraph.min_bounds()
        })
    }
    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        _cursor_position: iced::mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let text_state = tree
            .state
            .downcast_ref::<MeasuredMiddleEllipsizedTextState<Renderer::Paragraph, Renderer::Font>>(
            );
        let bounds = layout.bounds();

        renderer.fill_paragraph(
            &text_state.paragraph,
            paragraph_draw_anchor(bounds, &text_state.paragraph),
            style.text_color,
            *viewport,
        );
    }
}
fn paragraph_draw_anchor<Paragraph>(bounds: Rectangle, paragraph: &Paragraph) -> Point
where
    Paragraph: text::Paragraph,
{
    bounds.anchor(
        paragraph.min_bounds(),
        paragraph.align_x(),
        paragraph.align_y(),
    )
}

// 参数集合即 iced 文本测量的完整输入，与 MeasuredTextLayoutKey 字段一一对应
#[allow(clippy::too_many_arguments)]
fn fit_middle_ellipsized_text<Renderer>(
    content: &str,
    emphasized: &[Range<usize>],
    available_width: f32,
    available_height: f32,
    size: Pixels,
    line_height: text::LineHeight,
    font: Font,
    shaping: text::Shaping,
    wrapping: text::Wrapping,
    ellipsis_kind: MiddleEllipsisKind,
) -> String
where
    Renderer: text::Renderer<Font = Font>,
{
    if content.is_empty() || available_width <= 0.0 || available_height <= 0.0 {
        return String::new();
    }

    fit_middle_ellipsized_by(content, ellipsis_kind, |candidate| {
        measured_text_fits::<Renderer>(
            candidate,
            &emphasis::displayed_emphasis(content, candidate, ellipsis_kind, emphasized),
            available_width,
            available_height,
            size,
            line_height,
            font,
            shaping,
            wrapping,
        )
    })
}

fn fit_middle_ellipsized_by(
    content: &str,
    ellipsis_kind: MiddleEllipsisKind,
    mut fits: impl FnMut(&str) -> bool,
) -> String {
    if content.is_empty() {
        return String::new();
    }

    let content_char_count = content.chars().count();
    const DIRECT_MEASUREMENT_CHAR_LIMIT: usize = 256;

    // Shaping the complete string is the dominant cost for long CJK names. For
    // those names, search from short candidates instead of measuring the whole
    // filename and then repeating the same expensive work in a binary search.
    if content_char_count <= DIRECT_MEASUREMENT_CHAR_LIMIT && fits(content) {
        return content.to_owned();
    }

    if !fits(ELLIPSIS_MARKER) {
        return String::new();
    }

    let mut best = ELLIPSIS_MARKER.to_owned();
    let mut lower = 0usize;
    let mut upper = 1usize;

    while upper < content_char_count {
        let candidate = ellipsis_kind.candidate(content, upper);
        if !fits(&candidate) {
            break;
        }

        best = candidate;
        lower = upper;
        upper = upper.saturating_mul(2);
    }

    if upper >= content_char_count {
        upper = content_char_count;
        let candidate = ellipsis_kind.candidate(content, upper);
        if fits(&candidate) {
            return candidate;
        }
    }

    while lower + 1 < upper {
        let visible_chars = lower + (upper - lower) / 2;
        let candidate = ellipsis_kind.candidate(content, visible_chars);

        if fits(&candidate) {
            best = candidate;
            lower = visible_chars;
        } else {
            upper = visible_chars;
        }
    }

    best
}
#[allow(clippy::too_many_arguments)]
fn measured_text_fits<Renderer>(
    content: &str,
    emphasis: &[Range<usize>],
    available_width: f32,
    available_height: f32,
    size: Pixels,
    line_height: text::LineHeight,
    font: Font,
    shaping: text::Shaping,
    wrapping: text::Wrapping,
) -> bool
where
    Renderer: text::Renderer<Font = Font>,
{
    let paragraph: Renderer::Paragraph = emphasis::emphasized_paragraph(
        text::Text {
            content,
            bounds: Size::new(available_width, TEXT_MEASUREMENT_HEIGHT),
            size,
            line_height,
            font,
            align_x: text::Alignment::Left,
            align_y: alignment::Vertical::Top,
            shaping,
            wrapping,
        },
        emphasis,
    );
    let bounds = paragraph.min_bounds();

    bounds.width <= available_width && bounds.height <= available_height
}

fn measured_text_width<Renderer>(
    content: &str,
    size: Pixels,
    line_height: text::LineHeight,
    font: Renderer::Font,
    shaping: text::Shaping,
) -> f32
where
    Renderer: text::Renderer,
{
    let paragraph = Renderer::Paragraph::with_text(text::Text {
        content,
        bounds: Size::new(TEXT_MEASUREMENT_WIDTH, TEXT_MEASUREMENT_HEIGHT),
        size,
        line_height,
        font,
        align_x: text::Alignment::Left,
        align_y: alignment::Vertical::Top,
        shaping,
        wrapping: text::Wrapping::None,
    });

    paragraph.min_width()
}

pub fn measured_text_natural_width<Renderer>(renderer: &Renderer, content: &str, size: f32) -> f32
where
    Renderer: text::Renderer,
{
    measured_text_width::<Renderer>(
        content,
        Pixels(size),
        text::LineHeight::default(),
        renderer.default_font(),
        shaping_for_content(content),
    )
}

fn shaping_for_content(content: &str) -> text::Shaping {
    if content.is_ascii() {
        text::Shaping::Basic
    } else {
        text::Shaping::Advanced
    }
}

/// 中间省略后原文留下的部分（字节）：头是 `..head_end`，尾是 `tail_start..`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MiddleCut {
    head_end: usize,
    tail_start: usize,
}

impl MiddleEllipsisKind {
    fn candidate(self, content: &str, visible_chars: usize) -> String {
        if visible_chars >= content.chars().count() {
            return content.to_owned();
        }
        let cut = self.cut(content, visible_chars);
        let mut candidate = String::with_capacity(
            cut.head_end + ELLIPSIS_MARKER.len() + content.len() - cut.tail_start,
        );
        candidate.push_str(&content[..cut.head_end]);
        candidate.push_str(ELLIPSIS_MARKER);
        candidate.push_str(&content[cut.tail_start..]);
        candidate
    }

    /// 只留 `visible_chars` 个字符时保留的头尾；`visible_chars` 小于原文字符数。文件名
    /// 至少把整个扩展名留在尾巴里。
    fn cut(self, content: &str, visible_chars: usize) -> MiddleCut {
        let minimum_suffix_chars = match self {
            Self::GeneralText => 0,
            Self::FileName => content
                .rsplit_once('.')
                .filter(|(stem, extension)| !stem.is_empty() && !extension.is_empty())
                .map_or(0, |(_, extension)| extension.chars().count() + 1),
        };
        let end_chars = (visible_chars / 2)
            .max(minimum_suffix_chars)
            .min(visible_chars.saturating_sub(1));
        let start_chars = visible_chars - end_chars;
        let byte_at_char = |index: usize| {
            content
                .char_indices()
                .nth(index)
                .map_or(content.len(), |(byte, _)| byte)
        };
        MiddleCut {
            head_end: byte_at_char(start_chars),
            tail_start: byte_at_char(content.chars().count() - end_chars),
        }
    }
}

#[cfg(test)]
fn middle_ellipsized_candidate(content: &str, visible_chars: usize) -> String {
    MiddleEllipsisKind::GeneralText.candidate(content, visible_chars)
}

#[cfg(test)]
fn middle_ellipsized_filename_candidate(content: &str, visible_chars: usize) -> String {
    MiddleEllipsisKind::FileName.candidate(content, visible_chars)
}

impl<'a, Message, Theme, Renderer> From<MeasuredMiddleEllipsizedText>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: text::Renderer<Font = Font> + 'a,
{
    fn from(text: MeasuredMiddleEllipsizedText) -> Self {
        Element::new(text)
    }
}

/// 按字符数做中间省略的纯字符串变体：用于无法进入布局测量阶段的场景
/// （通知文案、补全建议行等），与像素测量版共享“保头保尾”的省略语义。
pub fn format_middle_ellipsized_text(content: &str, max_chars: usize) -> String {
    const MARKER: &str = "...";

    if content.len() <= max_chars {
        return content.to_owned();
    }

    if content.is_ascii() {
        return format_ascii_middle_ellipsized_text(content, max_chars);
    }

    if content.chars().count() <= max_chars {
        return content.to_owned();
    }

    if max_chars <= MARKER.len() {
        return MARKER.chars().take(max_chars).collect();
    }

    let visible_chars = max_chars - MARKER.len();
    let start_chars = visible_chars.div_ceil(2);
    let end_chars = visible_chars / 2;
    let start: String = content.chars().take(start_chars).collect();
    let end: String = content
        .chars()
        .rev()
        .take(end_chars)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    format!("{start}{MARKER}{end}")
}

fn format_ascii_middle_ellipsized_text(content: &str, max_chars: usize) -> String {
    const MARKER: &str = "...";

    if max_chars <= MARKER.len() {
        return MARKER[..max_chars].to_owned();
    }

    let visible_chars = max_chars - MARKER.len();
    let start_chars = visible_chars.div_ceil(2);
    let end_chars = visible_chars / 2;
    let mut text = String::with_capacity(max_chars);
    text.push_str(&content[..start_chars]);
    text.push_str(MARKER);
    text.push_str(&content[content.len() - end_chars..]);
    text
}

#[cfg(test)]
#[path = "measured_text_tests.rs"]
mod tests;
