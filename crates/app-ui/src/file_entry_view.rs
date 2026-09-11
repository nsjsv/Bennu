use std::path::Path;

use file_core::{DirectoryEntry, FileKind};
use iced::alignment::{Horizontal, Vertical};
use iced::widget::{container, image, Stack, Svg};
use iced::{Background, Border, Element, Length, Theme};

use crate::app::panes::BrowserPaneView;
use crate::app::FileBrowser;
use crate::appearance::{
    base_text_color, hovered_row_style, icon_svg_style,
    open_child_row_style, selected_icon_svg_style, selected_row_style, selected_row_style_for_run,
    warning_icon_svg_style,
};
use crate::config::ViewDensityLevel;
use crate::file_entry_presentation::SelectionRunPosition;
use crate::icons::{file_entry_icon_symbol, IconSymbol};
use crate::matugen_theme::ui_colors;
use crate::model::{FileEntryContentModifier, Message};
use crate::thumbnail_cache::{
    COLUMN_THUMBNAIL_EDGE, COLUMN_THUMBNAIL_SIZE, LIST_THUMBNAIL_EDGE, LIST_THUMBNAIL_SIZE,
};

pub(crate) const ENTRY_ICON_SIZE: f32 = 18.0;
const COLUMN_ENTRY_ICON_SIZE: f32 = 16.0;
const ENTRY_BADGE_AREA_RATIO: f32 = 0.42;
const ENTRY_BADGE_ICON_RATIO: f32 = 0.66;
const ENTRY_BADGE_MIN_SIZE: f32 = 10.0;
const ENTRY_BADGE_MAX_SIZE: f32 = 24.0;

#[derive(Debug, Clone, Copy)]
pub(crate) enum FileEntryIconDensity {
    List(ViewDensityLevel),
    Column(ViewDensityLevel),
    Grid(u32),
}

impl FileEntryIconDensity {
    fn thumbnail_edge(self) -> u32 {
        match self {
            // List/Columns 的缩略图源尺寸固定，只有显示槽随密度档位缩放。
            Self::List(_) => LIST_THUMBNAIL_EDGE,
            Self::Column(_) => COLUMN_THUMBNAIL_EDGE,
            Self::Grid(icon_edge) => crate::icon_grid_geometry::thumbnail_edge(icon_edge),
        }
    }

    // 显示槽与密度几何一致取整到整数像素；缩略图源尺寸保持固定。
    fn thumbnail_size(self) -> f32 {
        match self {
            Self::List(level) => (LIST_THUMBNAIL_SIZE * level.scale()).round(),
            Self::Column(level) => (COLUMN_THUMBNAIL_SIZE * level.scale()).round(),
            Self::Grid(icon_edge) => icon_edge as f32,
        }
    }

    fn icon_size(self) -> f32 {
        match self {
            Self::List(level) => (ENTRY_ICON_SIZE * level.scale()).round(),
            Self::Column(level) => (COLUMN_ENTRY_ICON_SIZE * level.scale()).round(),
            Self::Grid(icon_edge) => icon_edge as f32 * 0.68,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileEntryVisualState {
    Normal,
    Hovered,
    OpenChild,
    Selected,
    Dragged,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum FileEntryIconTone {
    Normal,
    Selected,
    Warning,
}

impl FileEntryVisualState {
    pub(crate) fn from_entry_context(
        pane: BrowserPaneView<'_>,
        path: &Path,
        is_open_child: bool,
    ) -> Self {
        if is_drag_source(pane, path) {
            Self::Dragged
        } else if pane.is_path_selected(path) {
            Self::Selected
        } else if pane
            .hovered_entry
            .is_some_and(|hovered| hovered.as_path() == path)
        {
            Self::Hovered
        } else if is_open_child {
            Self::OpenChild
        } else {
            Self::Normal
        }
    }

    pub(crate) fn icon_tone(self) -> FileEntryIconTone {
        match self {
            // 拖拽源与选中共用同一高亮观感,拖拽预览由光标旁的堆叠图标负责。
            Self::Dragged | Self::Selected => FileEntryIconTone::Selected,
            Self::Normal | Self::Hovered | Self::OpenChild => FileEntryIconTone::Normal,
        }
    }

    pub(crate) fn content_style(
        self,
        modifier: FileEntryContentModifier,
    ) -> impl Fn(&Theme) -> iced::widget::container::Style + Clone {
        move |theme| {
            let text_color = match self {
                Self::Dragged | Self::Selected => selected_row_style(theme).text_color,
                Self::Hovered => hovered_row_style(theme).text_color,
                Self::OpenChild => open_child_row_style(theme).text_color,
                Self::Normal => Some(base_text_color(theme)),
            }
            .unwrap_or_else(|| base_text_color(theme))
            .scale_alpha(modifier.opacity());
            iced::widget::container::Style {
                text_color: Some(text_color),
                ..iced::widget::container::Style::default()
            }
        }
    }

    pub(crate) fn row_style_for_selection_run(
        self,
        selection_run_position: Option<SelectionRunPosition>,
    ) -> Option<Box<dyn Fn(&Theme) -> iced::widget::container::Style>> {
        match self {
            Self::Dragged | Self::Selected => {
                let style: Box<dyn Fn(&Theme) -> iced::widget::container::Style> =
                    match selection_run_position {
                        Some(position) => Box::new(selected_row_style_for_run(position)),
                        None => Box::new(selected_row_style),
                    };
                Some(style)
            }
            Self::Hovered => Some(Box::new(hovered_row_style)),
            Self::OpenChild => Some(Box::new(open_child_row_style)),
            Self::Normal => None,
        }
    }
}

impl FileBrowser {
    pub(crate) fn file_entry_content_modifier(&self, path: &Path) -> FileEntryContentModifier {
        self.pending_operation
            .as_ref()
            .map_or(FileEntryContentModifier::None, |operation| {
                operation.content_modifier_for_path(path)
            })
    }

    /// 拖拽预览图标:按源条目的文件类型取图标;损坏的符号链接显示警告。
    pub(crate) fn file_drag_icon_symbol(&self, path: &Path) -> IconSymbol {
        match self.entry_for_path(path) {
            Some(entry) if entry.kind == FileKind::Symlink && entry.is_broken_symlink => {
                IconSymbol::TriangleAlert
            }
            Some(entry) => crate::icons::file_kind_solid_symbol(entry.kind),
            None => IconSymbol::FileSolid,
        }
    }

    /// 拖拽组内文件夹与文件的数量,聚合预览文案用。
    pub(crate) fn file_drag_group_counts(&self) -> (usize, usize) {
        let Some(drag) = self.file_drag.as_ref() else {
            return (0, 0);
        };
        let mut folders = 0;
        let mut files = 0;
        for source in &drag.sources {
            match self.entry_for_path(source).map(|entry| entry.kind) {
                Some(FileKind::Directory) => folders += 1,
                _ => files += 1,
            }
        }
        (folders, files)
    }

    /// 拖拽预览的缩略图:按各视图的源尺寸依次命中缓存;文件夹与
    /// 尚未生成缩略图的条目回退到类型图标(None)。
    pub(crate) fn drag_preview_thumbnail(&self, path: &Path) -> Option<image::Handle> {
        let entry = self.entry_for_path(path)?;
        let candidate_edges = [
            crate::thumbnail_cache::LIST_THUMBNAIL_EDGE,
            crate::thumbnail_cache::COLUMN_THUMBNAIL_EDGE,
            self.user_config().icons_icon_edge().saturating_mul(2),
        ];
        candidate_edges
            .iter()
            .find_map(|edge| self.thumbnail_cache.ready_for_entry(entry, *edge))
            .map(|thumbnail| thumbnail.handle.clone())
    }
}

pub(crate) fn entry_text_input_style(
    modifier: FileEntryContentModifier,
) -> impl Fn(&Theme, iced::widget::text_input::Status) -> iced::widget::text_input::Style + Clone {
    move |theme, status| {
        let mut style = iced::widget::text_input::default(theme, status);
        style.value = style.value.scale_alpha(modifier.opacity());
        style.placeholder = style.placeholder.scale_alpha(modifier.opacity());
        style
    }
}

fn entry_thumbnail_image(
    handle: image::Handle,
    thumbnail_size: f32,
    modifier: FileEntryContentModifier,
) -> image::Image {
    image::Image::new(handle)
        .width(Length::Fixed(thumbnail_size))
        .height(Length::Fixed(thumbnail_size))
        .opacity(modifier.opacity())
}

pub(crate) fn entry_thumbnail_or_icon<'a>(
    browser: &'a FileBrowser,
    entry: &DirectoryEntry,
    tone: FileEntryIconTone,
    density: FileEntryIconDensity,
) -> Element<'a, Message> {
    let modifier = browser.file_entry_content_modifier(&entry.path);
    let thumbnail_edge = density.thumbnail_edge();
    let thumbnail_size = density.thumbnail_size();
    if let Some(thumbnail) = browser
        .thumbnail_cache
        .ready_for_entry(entry, thumbnail_edge)
    {
        let thumbnail: Element<'a, Message> = container(entry_thumbnail_image(
            thumbnail.handle.clone(),
            thumbnail_size,
            modifier,
        ))
        .width(Length::Fixed(thumbnail_size))
        .height(Length::Fixed(thumbnail_size))
        .into();
        return decorate_file_entry_icon(thumbnail, thumbnail_size, modifier);
    }

    let icon_size = density.icon_size();
    centered_file_entry_icon_slot(
        entry_icon(entry, tone, density)
            .opacity(modifier.opacity())
            .into(),
        icon_size,
        thumbnail_size,
        modifier,
    )
}

fn centered_file_entry_icon_slot<'a, Renderer>(
    content: Element<'a, Message, Theme, Renderer>,
    icon_area_size: f32,
    slot_size: f32,
    modifier: FileEntryContentModifier,
) -> Element<'a, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer + iced::advanced::svg::Renderer + 'a + 'static,
{
    container(decorate_file_entry_icon(content, icon_area_size, modifier))
        .width(Length::Fixed(slot_size))
        .height(Length::Fixed(slot_size))
        .center_x(Length::Fixed(slot_size))
        .center_y(Length::Fixed(slot_size))
        .into()
}

pub(crate) fn file_entry_symbol_icon(
    symbol: IconSymbol,
    tone: FileEntryIconTone,
    size: f32,
    modifier: FileEntryContentModifier,
) -> Element<'static, Message> {
    decorate_file_entry_icon(
        themed_icon(symbol, tone, size)
            .opacity(modifier.opacity())
            .into(),
        size,
        modifier,
    )
}

fn decorate_file_entry_icon<'a, Renderer>(
    content: Element<'a, Message, Theme, Renderer>,
    icon_area_size: f32,
    modifier: FileEntryContentModifier,
) -> Element<'a, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer + iced::advanced::svg::Renderer + 'a + 'static,
{
    let badge_symbol = match modifier {
        FileEntryContentModifier::None => return content,
        FileEntryContentModifier::Cut => IconSymbol::Scissors,
        FileEntryContentModifier::Copied => IconSymbol::Copy,
    };
    Stack::with_children([content, entry_badge_overlay(icon_area_size, badge_symbol)])
        .width(Length::Fixed(icon_area_size))
        .height(Length::Fixed(icon_area_size))
        .into()
}

fn entry_badge_overlay<'a, Renderer>(
    icon_area_size: f32,
    badge_symbol: IconSymbol,
) -> Element<'a, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer + iced::advanced::svg::Renderer + 'a + 'static,
{
    let badge_size =
        (icon_area_size * ENTRY_BADGE_AREA_RATIO).clamp(ENTRY_BADGE_MIN_SIZE, ENTRY_BADGE_MAX_SIZE);
    let badge_icon_size = badge_size * ENTRY_BADGE_ICON_RATIO;
    let badge = container(
        badge_symbol
            .view(badge_icon_size)
            .style(entry_badge_icon_style),
    )
    .width(Length::Fixed(badge_size))
    .height(Length::Fixed(badge_size))
    .center_x(Length::Fixed(badge_size))
    .center_y(Length::Fixed(badge_size))
    .style(entry_badge_style);

    container(badge)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Horizontal::Left)
        .align_y(Vertical::Bottom)
        .into()
}

fn entry_badge_style(theme: &Theme) -> iced::widget::container::Style {
    let colors = ui_colors(theme);
    iced::widget::container::Style {
        background: Some(Background::Color(colors.surface_bright)),
        border: Border {
            color: colors.outline,
            width: 1.0,
            radius: 3.0.into(),
        },
        ..iced::widget::container::Style::default()
    }
}

fn entry_badge_icon_style(
    theme: &Theme,
    _status: iced::widget::svg::Status,
) -> iced::widget::svg::Style {
    iced::widget::svg::Style {
        color: Some(ui_colors(theme).on_surface),
    }
}

pub(crate) fn themed_icon(
    symbol: IconSymbol,
    tone: FileEntryIconTone,
    size: f32,
) -> Svg<'static, Theme> {
    symbol.view(size).style(icon_tone_style(tone))
}

fn entry_icon(
    entry: &DirectoryEntry,
    tone: FileEntryIconTone,
    density: FileEntryIconDensity,
) -> Svg<'static, Theme> {
    let symbol = if entry.kind == FileKind::Symlink && entry.is_broken_symlink {
        IconSymbol::TriangleAlert
    } else {
        file_entry_icon_symbol(entry.kind, entry.name())
    };
    let tone = match (symbol, tone) {
        (IconSymbol::TriangleAlert, _) => FileEntryIconTone::Warning,
        _ => tone,
    };
    themed_icon(symbol, tone, density.icon_size())
}

fn icon_tone_style(
    tone: FileEntryIconTone,
) -> fn(&Theme, iced::widget::svg::Status) -> iced::widget::svg::Style {
    match tone {
        FileEntryIconTone::Normal => icon_svg_style(),
        FileEntryIconTone::Selected => selected_icon_svg_style(),
        FileEntryIconTone::Warning => warning_icon_svg_style(),
    }
}

fn is_drag_source(pane: BrowserPaneView<'_>, path: &Path) -> bool {
    pane.file_drag.is_some_and(|drag| {
        drag.is_dragging() && drag.sources.iter().any(|source| source.as_path() == path)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::advanced::{
        image as renderer_image, layout, mouse, renderer, svg as renderer_svg, widget::Tree, Layout,
    };
    use iced::{Rectangle, Size, Transformation};

    #[derive(Debug, Clone, Copy)]
    struct RecordedSvg {
        opacity: f32,
        bounds: Rectangle,
    }

    #[derive(Default)]
    struct RecordingRenderer {
        images: Vec<(f32, Rectangle)>,
        quads: Vec<(renderer::Quad, Background)>,
        svgs: Vec<RecordedSvg>,
    }

    impl iced::advanced::Renderer for RecordingRenderer {
        fn start_layer(&mut self, _bounds: Rectangle) {}

        fn end_layer(&mut self) {}

        fn start_transformation(&mut self, _transformation: Transformation) {}

        fn end_transformation(&mut self) {}

        fn fill_quad(&mut self, quad: renderer::Quad, background: impl Into<Background>) {
            self.quads.push((quad, background.into()));
        }

        fn reset(&mut self, _new_bounds: Rectangle) {}

        fn allocate_image(
            &mut self,
            _handle: &renderer_image::Handle,
            _callback: impl FnOnce(Result<renderer_image::Allocation, renderer_image::Error>)
                + Send
                + 'static,
        ) {
            panic!("file entry rendering test must not allocate images");
        }
    }

    impl renderer_svg::Renderer for RecordingRenderer {
        fn measure_svg(&self, _handle: &renderer_svg::Handle) -> Size<u32> {
            Size::new(24, 24)
        }

        fn draw_svg(&mut self, svg: renderer_svg::Svg, bounds: Rectangle, _clip_bounds: Rectangle) {
            self.svgs.push(RecordedSvg {
                opacity: svg.opacity,
                bounds,
            });
        }
    }

    impl renderer_image::Renderer for RecordingRenderer {
        type Handle = renderer_image::Handle;

        fn load_image(
            &self,
            _handle: &Self::Handle,
        ) -> Result<renderer_image::Allocation, renderer_image::Error> {
            Err(renderer_image::Error::Unsupported)
        }

        fn measure_image(&self, _handle: &Self::Handle) -> Option<Size<u32>> {
            Some(Size::new(1, 1))
        }

        fn draw_image(
            &mut self,
            image: renderer_image::Image<Self::Handle>,
            bounds: Rectangle,
            _clip_bounds: Rectangle,
        ) {
            self.images.push((image.opacity, bounds));
        }
    }

    fn draw_element(
        mut element: Element<'_, Message, Theme, RecordingRenderer>,
        available_size: Size,
    ) -> (Rectangle, RecordingRenderer) {
        let mut tree = Tree::new(element.as_widget());
        let mut renderer = RecordingRenderer::default();
        let limits = layout::Limits::new(Size::ZERO, available_size);
        let node = element
            .as_widget_mut()
            .layout(&mut tree, &renderer, &limits);
        let bounds = node.bounds();
        element.as_widget().draw(
            &tree,
            &mut renderer,
            &Theme::Light,
            &renderer::Style::default(),
            Layout::new(&node),
            mouse::Cursor::Unavailable,
            &bounds,
        );
        (bounds, renderer)
    }

    fn assert_approximately_equal(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 0.001,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn list_fallback_icon_is_centered_in_the_thumbnail_sized_slot() {
        let density = FileEntryIconDensity::List(crate::config::ViewDensityLevel::DEFAULT);
        let icon_size = density.icon_size();
        let slot_size = density.thumbnail_size();
        let body: Element<'_, Message, Theme, RecordingRenderer> = IconSymbol::File
            .view(icon_size)
            .style(icon_svg_style())
            .into();
        let slotted = centered_file_entry_icon_slot(
            body,
            icon_size,
            slot_size,
            FileEntryContentModifier::None,
        );

        let (bounds, renderer) = draw_element(slotted, Size::new(slot_size, slot_size));

        assert_eq!(bounds.size(), Size::new(slot_size, slot_size));
        assert_eq!(renderer.svgs.len(), 1);
        let icon_bounds = renderer.svgs[0].bounds;
        assert_eq!(icon_bounds.size(), Size::new(icon_size, icon_size));
        assert_eq!(icon_bounds.center(), bounds.center());
    }

    #[test]
    fn cut_svg_draws_dimmed_body_and_opaque_lower_left_badge_without_resizing() {
        let icon_area_size = ENTRY_ICON_SIZE;
        let body: Element<'_, Message, Theme, RecordingRenderer> = IconSymbol::File
            .view(icon_area_size)
            .style(icon_svg_style())
            .opacity(FileEntryContentModifier::Cut.opacity())
            .into();
        let decorated =
            decorate_file_entry_icon(body, icon_area_size, FileEntryContentModifier::Cut);

        let (bounds, renderer) = draw_element(decorated, Size::new(icon_area_size, icon_area_size));

        assert_eq!(bounds.size(), Size::new(icon_area_size, icon_area_size));
        assert_eq!(renderer.svgs.len(), 2);
        assert_approximately_equal(
            renderer.svgs[0].opacity,
            FileEntryContentModifier::Cut.opacity(),
        );
        assert_approximately_equal(renderer.svgs[1].opacity, 1.0);
        assert!(renderer.svgs[1].bounds.center_x() < bounds.center_x());
        assert!(renderer.svgs[1].bounds.center_y() > bounds.center_y());

        let badge_backplate_color = ui_colors(&Theme::Light).surface_bright;
        let (badge_quad, badge_background) = renderer
            .quads
            .iter()
            .find(|(_, background)| {
                matches!(background, Background::Color(color) if *color == badge_backplate_color)
            })
            .expect("cut badge must draw an opaque contrast backplate");
        let expected_badge_size = (icon_area_size * ENTRY_BADGE_AREA_RATIO)
            .clamp(ENTRY_BADGE_MIN_SIZE, ENTRY_BADGE_MAX_SIZE);
        assert_approximately_equal(badge_quad.bounds.x, bounds.x);
        assert_approximately_equal(
            badge_quad.bounds.y,
            bounds.y + bounds.height - expected_badge_size,
        );
        assert_approximately_equal(badge_quad.bounds.width, expected_badge_size);
        assert_approximately_equal(badge_quad.bounds.height, expected_badge_size);
        assert_approximately_equal(badge_quad.border.color.a, 1.0);
        let Background::Color(backplate_color) = badge_background else {
            unreachable!();
        };
        assert_approximately_equal(backplate_color.a, 1.0);
    }

    #[test]
    fn copied_svg_draws_undimmed_body_with_lower_left_badge() {
        let icon_area_size = ENTRY_ICON_SIZE;
        let body: Element<'_, Message, Theme, RecordingRenderer> = IconSymbol::File
            .view(icon_area_size)
            .style(icon_svg_style())
            .opacity(FileEntryContentModifier::Copied.opacity())
            .into();
        let decorated =
            decorate_file_entry_icon(body, icon_area_size, FileEntryContentModifier::Copied);

        let (bounds, renderer) = draw_element(decorated, Size::new(icon_area_size, icon_area_size));

        assert_eq!(bounds.size(), Size::new(icon_area_size, icon_area_size));
        assert_eq!(renderer.svgs.len(), 2);
        assert_approximately_equal(renderer.svgs[0].opacity, 1.0);
        assert_approximately_equal(renderer.svgs[1].opacity, 1.0);
        assert!(renderer.svgs[1].bounds.center_x() < bounds.center_x());
        assert!(renderer.svgs[1].bounds.center_y() > bounds.center_y());

        let badge_backplate_color = ui_colors(&Theme::Light).surface_bright;
        let (badge_quad, _) = renderer
            .quads
            .iter()
            .find(|(_, background)| {
                matches!(background, Background::Color(color) if *color == badge_backplate_color)
            })
            .expect("copied badge must draw an opaque contrast backplate");
        let expected_badge_size = (icon_area_size * ENTRY_BADGE_AREA_RATIO)
            .clamp(ENTRY_BADGE_MIN_SIZE, ENTRY_BADGE_MAX_SIZE);
        assert_approximately_equal(badge_quad.bounds.x, bounds.x);
        assert_approximately_equal(
            badge_quad.bounds.y,
            bounds.y + bounds.height - expected_badge_size,
        );
    }

    #[test]
    fn cut_thumbnail_widget_passes_real_opacity_to_image_renderer() {
        let thumbnail_size = 32.0;
        let thumbnail = entry_thumbnail_image(
            renderer_image::Handle::from_rgba(1, 1, vec![255, 255, 255, 255]),
            thumbnail_size,
            FileEntryContentModifier::Cut,
        );
        let element: Element<'_, Message, Theme, RecordingRenderer> = thumbnail.into();

        let (bounds, renderer) = draw_element(element, Size::new(thumbnail_size, thumbnail_size));

        assert_eq!(bounds.size(), Size::new(thumbnail_size, thumbnail_size));
        assert_eq!(renderer.images.len(), 1);
        assert_approximately_equal(
            renderer.images[0].0,
            FileEntryContentModifier::Cut.opacity(),
        );
    }

    #[test]
    fn cut_text_styles_scale_body_alpha_without_changing_interaction_colors() {
        let theme = Theme::Light;
        for visual_state in [
            FileEntryVisualState::Normal,
            FileEntryVisualState::Hovered,
            FileEntryVisualState::OpenChild,
            FileEntryVisualState::Selected,
            FileEntryVisualState::Dragged,
        ] {
            let base_color = match visual_state {
                FileEntryVisualState::Normal => base_text_color(&theme),
                FileEntryVisualState::Hovered => hovered_row_style(&theme)
                    .text_color
                    .expect("hover style must define text color"),
                FileEntryVisualState::OpenChild => open_child_row_style(&theme)
                    .text_color
                    .expect("open-child style must define text color"),
                FileEntryVisualState::Dragged | FileEntryVisualState::Selected => {
                    selected_row_style(&theme)
                        .text_color
                        .expect("selection style must define text color")
                }
            };
            let style = visual_state.content_style(FileEntryContentModifier::Cut)(&theme);
            let cut_color = style
                .text_color
                .expect("cut content style must define inherited text color");
            assert_approximately_equal(
                cut_color.a,
                base_color.a * FileEntryContentModifier::Cut.opacity(),
            );
            assert_eq!(cut_color.r, base_color.r);
            assert_eq!(cut_color.g, base_color.g);
            assert_eq!(cut_color.b, base_color.b);
            assert!(style.background.is_none());
        }

        let base_input =
            iced::widget::text_input::default(&theme, iced::widget::text_input::Status::Active);
        let cut_input = entry_text_input_style(FileEntryContentModifier::Cut)(
            &theme,
            iced::widget::text_input::Status::Active,
        );
        assert_approximately_equal(
            cut_input.value.a,
            base_input.value.a * FileEntryContentModifier::Cut.opacity(),
        );
        assert_approximately_equal(
            cut_input.placeholder.a,
            base_input.placeholder.a * FileEntryContentModifier::Cut.opacity(),
        );
    }
}
