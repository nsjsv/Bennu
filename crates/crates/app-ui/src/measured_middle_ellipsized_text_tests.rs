use super::{
    displayed_content_is_ellipsized, fit_middle_ellipsized_by, fit_middle_ellipsized_text,
    measured_text_fits, middle_ellipsized_candidate, middle_ellipsized_filename_candidate,
    shaping_for_content, MeasuredMiddleEllipsizedText, MeasuredMiddleEllipsizedTextState,
    MeasuredTextLayoutKey, MiddleEllipsisKind, ELLIPSIS_MARKER,
};
use crate::icon_grid_geometry::{
    ICON_GRID_LABEL_HEIGHT, ICON_GRID_LABEL_LINE_HEIGHT_PX, ICON_GRID_LABEL_SIZE,
};
use iced::advanced::text::{self, Paragraph as _};
use iced::advanced::{image, layout, mouse, renderer, widget::Tree, Layout};
use iced::{
    Background, Color, Element, Font, Length, Pixels, Point, Rectangle, Size, Theme, Transformation,
};

#[derive(Default)]
struct RecordingRenderer {
    paragraph_positions: Vec<Point>,
}

impl iced::advanced::Renderer for RecordingRenderer {
    fn start_layer(&mut self, _bounds: Rectangle) {}

    fn end_layer(&mut self) {}

    fn start_transformation(&mut self, _transformation: Transformation) {}

    fn end_transformation(&mut self) {}

    fn fill_quad(&mut self, _quad: renderer::Quad, _background: impl Into<Background>) {}

    fn reset(&mut self, _new_bounds: Rectangle) {}

    fn allocate_image(
        &mut self,
        _handle: &image::Handle,
        _callback: impl FnOnce(Result<image::Allocation, image::Error>) + Send + 'static,
    ) {
        panic!("measured text draw must not allocate images");
    }
}

impl text::Renderer for RecordingRenderer {
    type Font = Font;
    type Paragraph = <iced::Renderer as text::Renderer>::Paragraph;
    type Editor = <iced::Renderer as text::Renderer>::Editor;

    const ICON_FONT: Font = <iced::Renderer as text::Renderer>::ICON_FONT;
    const CHECKMARK_ICON: char = <iced::Renderer as text::Renderer>::CHECKMARK_ICON;
    const ARROW_DOWN_ICON: char = <iced::Renderer as text::Renderer>::ARROW_DOWN_ICON;
    const SCROLL_UP_ICON: char = <iced::Renderer as text::Renderer>::SCROLL_UP_ICON;
    const SCROLL_DOWN_ICON: char = <iced::Renderer as text::Renderer>::SCROLL_DOWN_ICON;
    const SCROLL_LEFT_ICON: char = <iced::Renderer as text::Renderer>::SCROLL_LEFT_ICON;
    const SCROLL_RIGHT_ICON: char = <iced::Renderer as text::Renderer>::SCROLL_RIGHT_ICON;
    const ICED_LOGO: char = <iced::Renderer as text::Renderer>::ICED_LOGO;

    fn default_font(&self) -> Font {
        Font::default()
    }

    fn default_size(&self) -> Pixels {
        Pixels(16.0)
    }

    fn fill_paragraph(
        &mut self,
        _paragraph: &Self::Paragraph,
        position: Point,
        _color: Color,
        _clip_bounds: Rectangle,
    ) {
        self.paragraph_positions.push(position);
    }

    fn fill_editor(
        &mut self,
        _editor: &Self::Editor,
        _position: Point,
        _color: Color,
        _clip_bounds: Rectangle,
    ) {
        panic!("measured text draw must not fill an editor");
    }

    fn fill_text(
        &mut self,
        _text: text::Text<String, Font>,
        _position: Point,
        _color: Color,
        _clip_bounds: Rectangle,
    ) {
        panic!("measured text draw must not fill uncached text");
    }
}

fn recorded_widget_draw(align_x: text::Alignment) -> (Rectangle, Size, Point) {
    let widget = MeasuredMiddleEllipsizedText::new("short.txt")
        .size(ICON_GRID_LABEL_SIZE)
        .line_height(text::LineHeight::Absolute(Pixels(
            ICON_GRID_LABEL_LINE_HEIGHT_PX,
        )))
        .wrapping(text::Wrapping::WordOrGlyph)
        .width(Length::Fill)
        .height(Length::Fixed(ICON_GRID_LABEL_HEIGHT))
        .align_x(align_x);
    let mut element: Element<'_, (), Theme, RecordingRenderer> = widget.into();
    let mut tree = Tree::new(element.as_widget());
    let mut renderer = RecordingRenderer::default();
    let limits = layout::Limits::new(Size::ZERO, Size::new(128.0, ICON_GRID_LABEL_HEIGHT));
    let node = element
        .as_widget_mut()
        .layout(&mut tree, &renderer, &limits)
        .move_to(Point::new(10.0, 20.0));
    let bounds = node.bounds();
    let viewport = bounds;

    element.as_widget().draw(
        &tree,
        &mut renderer,
        &Theme::Light,
        &renderer::Style::default(),
        Layout::new(&node),
        mouse::Cursor::Unavailable,
        &viewport,
    );
    let paragraph = &tree
        .state
        .downcast_ref::<MeasuredMiddleEllipsizedTextState<
            <RecordingRenderer as text::Renderer>::Paragraph,
            <RecordingRenderer as text::Renderer>::Font,
        >>()
        .paragraph;
    assert_eq!(renderer.paragraph_positions.len(), 1);

    (
        bounds,
        paragraph.min_bounds(),
        renderer.paragraph_positions[0],
    )
}

#[test]
fn widget_draw_passes_the_aligned_anchor_to_fill_paragraph() {
    let (center_bounds, center_min_bounds, center_position) =
        recorded_widget_draw(text::Alignment::Center);
    assert_eq!(
        center_position,
        center_bounds.anchor(
            center_min_bounds,
            text::Alignment::Center,
            iced::alignment::Vertical::Top,
        )
    );
    assert!(center_position.x > center_bounds.x);

    let (left_bounds, _, left_position) = recorded_widget_draw(text::Alignment::Left);
    assert_eq!(left_position, left_bounds.position());
}

#[test]
fn tooltip_eligibility_matches_actual_displayed_content() {
    assert!(!displayed_content_is_ellipsized("short.txt", "short.txt"));
    assert!(displayed_content_is_ellipsized(
        "very-long-file-name.txt",
        "very...name.txt"
    ));
}

#[test]
fn candidate_preserves_start_and_end() {
    assert_eq!(
        middle_ellipsized_candidate("very-long-name.rs", 8),
        "very...e.rs"
    );
}

#[test]
fn layout_key_invalidates_changed_measurement_inputs() {
    let bounds = Size::new(96.0, 48.0);
    let size = Pixels(16.0);
    let line_height = text::LineHeight::Absolute(Pixels(20.0));
    let font = Font::default();
    let key = MeasuredTextLayoutKey::new(
        "文件名.txt",
        bounds,
        size,
        line_height,
        font,
        text::Shaping::Advanced,
        text::Wrapping::WordOrGlyph,
        text::Alignment::Center,
        MiddleEllipsisKind::FileName,
    );

    assert!(key.matches(
        "文件名.txt",
        bounds,
        size,
        line_height,
        font,
        text::Shaping::Advanced,
        text::Wrapping::WordOrGlyph,
        text::Alignment::Center,
        MiddleEllipsisKind::FileName,
    ));
    assert!(!key.matches(
        "另一个文件名.txt",
        bounds,
        size,
        line_height,
        font,
        text::Shaping::Advanced,
        text::Wrapping::WordOrGlyph,
        text::Alignment::Center,
        MiddleEllipsisKind::FileName,
    ));
    assert!(!key.matches(
        "文件名.txt",
        Size::new(97.0, 48.0),
        size,
        line_height,
        font,
        text::Shaping::Advanced,
        text::Wrapping::WordOrGlyph,
        text::Alignment::Center,
        MiddleEllipsisKind::FileName,
    ));
    assert!(!key.matches(
        "文件名.txt",
        bounds,
        size,
        line_height,
        font,
        text::Shaping::Basic,
        text::Wrapping::WordOrGlyph,
        text::Alignment::Center,
        MiddleEllipsisKind::FileName,
    ));
    assert!(!key.matches(
        "文件名.txt",
        bounds,
        size,
        line_height,
        font,
        text::Shaping::Advanced,
        text::Wrapping::None,
        text::Alignment::Center,
        MiddleEllipsisKind::FileName,
    ));
    assert!(!key.matches(
        "文件名.txt",
        bounds,
        size,
        line_height,
        font,
        text::Shaping::Advanced,
        text::Wrapping::WordOrGlyph,
        text::Alignment::Left,
        MiddleEllipsisKind::FileName,
    ));
    assert!(!key.matches(
        "文件名.txt",
        bounds,
        size,
        line_height,
        font,
        text::Shaping::Advanced,
        text::Wrapping::WordOrGlyph,
        text::Alignment::Center,
        MiddleEllipsisKind::GeneralText,
    ));
}
#[test]
fn candidate_keeps_whole_content_when_visible_chars_fit() {
    assert_eq!(middle_ellipsized_candidate("short", 5), "short");
}

#[test]
fn candidate_handles_multibyte_characters() {
    assert_eq!(
        middle_ellipsized_candidate("项目文件夹名称", 4),
        "项目...名称"
    );
}

#[test]
fn filename_candidate_preserves_the_complete_extension() {
    let candidate = middle_ellipsized_filename_candidate("archive-name.longextension", 18);

    assert!(candidate.starts_with("arch"));
    assert!(candidate.ends_with(".longextension"));
    assert!(candidate.contains(ELLIPSIS_MARKER));
}

#[test]
fn fixed_three_line_fit_keeps_the_longest_measured_candidate() {
    let content = "abcdefghijklmnop.rs";
    let three_line_capacity = 13;
    let displayed = fit_middle_ellipsized_by(content, MiddleEllipsisKind::FileName, |candidate| {
        candidate.chars().count() <= three_line_capacity
    });

    assert_eq!(displayed.chars().count(), three_line_capacity);
    assert!(displayed.starts_with("abcde"));
    assert!(displayed.ends_with("op.rs"));
    assert!(displayed.contains(ELLIPSIS_MARKER));
}

#[test]
fn long_cjk_content_avoids_shaping_full_filename_candidates() {
    let content = "界".repeat(10_000);
    let mut measurement_count = 0;
    let mut largest_candidate_chars = 0;
    let displayed = fit_middle_ellipsized_by(&content, MiddleEllipsisKind::FileName, |candidate| {
        measurement_count += 1;
        largest_candidate_chars = largest_candidate_chars.max(candidate.chars().count());
        candidate.chars().count() <= 20
    });

    assert_eq!(displayed.chars().count(), 20);
    assert!(displayed.contains(ELLIPSIS_MARKER));
    assert!(measurement_count < 20);
    assert!(largest_candidate_chars < 100);
}

#[test]
fn fixed_three_line_fit_keeps_content_that_renderer_reports_as_fitting() {
    let content = "three-line-name.rs";

    assert_eq!(
        fit_middle_ellipsized_by(content, MiddleEllipsisKind::GeneralText, |candidate| {
            candidate == content
        }),
        content
    );
}

#[test]
fn wrapped_fit_uses_renderer_measurement_to_stay_within_three_lines() {
    let content = "very-long-file-name-with-many-segments-that-needs-four-lines.rs";
    let width = 96.0;
    let size = Pixels(ICON_GRID_LABEL_SIZE);
    let line_height = text::LineHeight::Absolute(Pixels(ICON_GRID_LABEL_LINE_HEIGHT_PX));
    let height = ICON_GRID_LABEL_HEIGHT;
    let font = iced::Font::default();
    let shaping = shaping_for_content(content);
    let wrapping = text::Wrapping::WordOrGlyph;

    assert!(!measured_text_fits::<iced::Renderer>(
        content,
        width,
        height,
        size,
        line_height,
        font,
        shaping,
        wrapping,
    ));

    let displayed = fit_middle_ellipsized_text::<iced::Renderer>(
        content,
        width,
        height,
        size,
        line_height,
        font,
        shaping,
        wrapping,
        MiddleEllipsisKind::FileName,
    );

    assert!(displayed.starts_with('v'));
    assert!(displayed.contains(ELLIPSIS_MARKER));
    assert!(displayed.ends_with(".rs"));
    assert!(displayed_content_is_ellipsized(content, &displayed));
    assert!(measured_text_fits::<iced::Renderer>(
        &displayed,
        width,
        height,
        size,
        line_height,
        font,
        shaping,
        wrapping,
    ));
}
