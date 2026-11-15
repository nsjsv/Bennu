//! 静态图片预览面板（缩略图/光栅原图/SVG 原图）：缩放平移媒体区、透明处的棋盘格、
//! 底部浮着的缩放控件。交互契约见 image-preview-guidelines 第一节（视口钳制/锚点缩放/双击）。

use iced::alignment::{Horizontal, Vertical};
use iced::widget::image::FilterMethod;
use iced::widget::{button, container, image, mouse_area, row, svg, text, Space, Stack};
use iced::{Alignment, ContentFit, Element, Length, Shadow, Size, Vector};

use bennu_theme::icons::IconSymbol;
use bennu_theme::preview_styles::preview_media_style;
use bennu_theme::translated_surface::translated_surface;
use bennu_theme::ui_colors;

use crate::checkerboard::checkerboard;
use crate::image_preview_viewport::{ImagePreviewViewport, PreviewImageViewportMessage};
use crate::media_controls::{faded_media_button_style, faded_media_icon, faded_media_text_style};
use crate::panel_text::readable_text;
use crate::preview::{image_preview_size, ImagePreviewContent, PreviewSize};
use crate::preview_message::PreviewMessage;

const ZOOM_CONTROL_BUTTON_SIZE: f32 = 28.0;
const ZOOM_CONTROL_ICON_SIZE: f32 = 16.0;
const ZOOM_PERCENT_WIDTH: f32 = 52.0;
const ZOOM_CONTROLS_BOTTOM_MARGIN: f32 = 14.0;
const ZOOM_CONTROLS_RADIUS: f32 = 18.0;
const ZOOM_CONTROLS_BACKGROUND_ALPHA: f32 = 0.9;
/// 百分比和 100% 差这么多以内算 1:1（缩放倍率是浮点连乘出来的）。
const ACTUAL_SIZE_TOLERANCE_PERCENT: f32 = 0.5;

pub(crate) fn image_preview_panel<Message>(
    content: &ImagePreviewContent,
    size: PreviewSize,
    viewport: &ImagePreviewViewport,
    controls_opacity: f32,
) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let (width, height) = content.dimensions();
    let (fit_width, fit_height) = image_preview_size(size, width, height);
    // 缩略图占位时还不知道原图多大（尺寸是缩略图的），比例和缩放控件等原图到了才有。
    let percent = match content {
        ImagePreviewContent::Thumbnail { .. } => None,
        ImagePreviewContent::OriginalRaster { width, .. }
        | ImagePreviewContent::OriginalSvg { width, .. } => {
            Some(viewport.zoom_percent(*width, Size::new(fit_width, fit_height)))
        }
    };
    // 放大到超过原始像素后按像素块显示，不插值糊掉；适应窗口时小图被拉大照旧平滑。
    let filter = if viewport.is_zoomed()
        && percent.is_some_and(|percent| percent > 100.0 + ACTUAL_SIZE_TOLERANCE_PERCENT)
    {
        FilterMethod::Nearest
    } else {
        FilterMethod::Linear
    };
    let media = zoomable_media_area(fit_width, fit_height, size, viewport, |width, height| {
        image_layers(content, width, height, filter)
    });

    let mut layers = Stack::with_children([media])
        .width(Length::Fill)
        .height(Length::Fill);
    if let Some(percent) = percent {
        if controls_opacity > f32::EPSILON {
            layers = layers.push(zoom_controls_layer(percent, controls_opacity));
        }
    }

    // 指针事件包住整块（含控件）：指针移到控件上不算离开，右侧面板的控件不会一碰就消失。
    let viewport_message = |message| Message::from(PreviewMessage::PreviewImageViewport(message));
    mouse_area(layers)
        .on_move(move |position| {
            viewport_message(PreviewImageViewportMessage::PointerMoved(position))
        })
        .on_exit(viewport_message(PreviewImageViewportMessage::PointerLeft))
        .on_press(viewport_message(PreviewImageViewportMessage::PanStarted))
        .on_release(viewport_message(PreviewImageViewportMessage::PanEnded))
        .on_double_click(viewport_message(PreviewImageViewportMessage::DoubleClicked))
        .on_scroll(move |delta| viewport_message(PreviewImageViewportMessage::Zoomed(delta)))
        .interaction(iced::mouse::Interaction::Grab)
        .into()
}

/// 一张图按显示大小叠起来：透明的垫棋盘格；不透明的垫缩略图，原图大贴图还没传上显卡时不露底。
/// 透明图不垫缩略图——两张半透明叠在一起边缘会变深。
fn image_layers<Message>(
    content: &ImagePreviewContent,
    width: f32,
    height: f32,
    filter: FilterMethod,
) -> Element<'static, Message>
where
    Message: 'static + Clone,
{
    let sized = |handle: &image::Handle| {
        image::Image::new(handle.clone())
            .width(Length::Fixed(width))
            .height(Length::Fixed(height))
            .content_fit(ContentFit::Contain)
            .filter_method(filter)
    };
    let layers = Stack::new()
        .width(Length::Fixed(width))
        .height(Length::Fixed(height));
    match content {
        ImagePreviewContent::Thumbnail { handle, .. } => sized(handle).into(),
        ImagePreviewContent::OriginalRaster {
            raster_handle,
            has_alpha: true,
            ..
        } => layers
            .push(checkerboard(width, height))
            .push(sized(raster_handle))
            .into(),
        ImagePreviewContent::OriginalRaster {
            raster_handle,
            placeholder_handle,
            ..
        } => layers
            .push(sized(placeholder_handle))
            .push(sized(raster_handle))
            .into(),
        ImagePreviewContent::OriginalSvg { handle, .. } => layers
            .push(checkerboard(width, height))
            .push(
                svg::Svg::new(handle.clone())
                    .width(Length::Fixed(width))
                    .height(Length::Fixed(height))
                    .content_fit(ContentFit::Contain),
            )
            .into(),
    }
}

/// 静态图片媒体区：适应窗口时居中；缩放后按视口位移摆放（指针事件由外层统一接）。
fn zoomable_media_area<Message>(
    fit_width: f32,
    fit_height: f32,
    size: PreviewSize,
    viewport: &ImagePreviewViewport,
    content_at_scaled_size: impl Fn(f32, f32) -> Element<'static, Message>,
) -> Element<'static, Message>
where
    Message: 'static + Clone,
{
    let media: Element<'static, Message> = if viewport.is_zoomed() {
        let scaled_width = fit_width * viewport.scale;
        let scaled_height = fit_height * viewport.scale;
        let x = size.width / 2.0 + viewport.offset.x - scaled_width / 2.0;
        let y = size.height / 2.0 + viewport.offset.y - scaled_height / 2.0;
        translated_surface(
            content_at_scaled_size(scaled_width, scaled_height),
            Vector::new(x, y),
        )
    } else {
        container(content_at_scaled_size(fit_width, fit_height))
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    };

    container(media)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(preview_media_style)
        .into()
}

/// 底部居中浮着的一条：缩小、比例、放大、适应窗口/1:1。跟主题配色，深浅主题、图片明暗下都看得清。
fn zoom_controls_layer<Message>(percent: f32, opacity: f32) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let opacity = opacity.clamp(0.0, 1.0);
    let at_actual_size = (percent - 100.0).abs() <= ACTUAL_SIZE_TOLERANCE_PERCENT;
    let icon_button = |symbol, message| {
        button(faded_media_icon(symbol, ZOOM_CONTROL_ICON_SIZE, opacity))
            .on_press(Message::from(PreviewMessage::PreviewImageViewport(message)))
            .padding(6)
            .width(Length::Fixed(ZOOM_CONTROL_BUTTON_SIZE))
            .height(Length::Fixed(ZOOM_CONTROL_BUTTON_SIZE))
            .style(move |theme, status| faded_media_button_style(theme, status, opacity))
    };
    // 在 1:1 时按钮回到适应窗口，否则去 1:1。
    let toggle: Element<'static, Message> = if at_actual_size {
        icon_button(
            IconSymbol::Maximize,
            PreviewImageViewportMessage::FitOrActualToggled,
        )
        .into()
    } else {
        button(
            readable_text("1:1".to_owned())
                .style(move |theme| faded_media_text_style(theme, opacity)),
        )
        .on_press(Message::from(PreviewMessage::PreviewImageViewport(
            PreviewImageViewportMessage::FitOrActualToggled,
        )))
        .padding([4, 8])
        .height(Length::Fixed(ZOOM_CONTROL_BUTTON_SIZE))
        .style(move |theme, status| faded_media_button_style(theme, status, opacity))
        .into()
    };
    let controls = row![
        icon_button(IconSymbol::ZoomOut, PreviewImageViewportMessage::ZoomedOut),
        text(format!("{percent:.0}%"))
            .width(Length::Fixed(ZOOM_PERCENT_WIDTH))
            .align_x(Horizontal::Center)
            .style(move |theme| faded_media_text_style(theme, opacity)),
        icon_button(IconSymbol::ZoomIn, PreviewImageViewportMessage::ZoomedIn),
        Space::new().width(Length::Fixed(4.0)),
        toggle,
    ]
    .spacing(2)
    .align_y(Alignment::Center);
    let bar = container(controls)
        .padding([4, 8])
        .style(move |theme| zoom_controls_style(theme, opacity));
    container(bar)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Horizontal::Center)
        .align_y(Vertical::Bottom)
        .padding(iced::Padding {
            bottom: ZOOM_CONTROLS_BOTTOM_MARGIN,
            ..iced::Padding::ZERO
        })
        .into()
}

fn zoom_controls_style(theme: &iced::Theme, opacity: f32) -> container::Style {
    let colors = ui_colors(theme);
    container::Style {
        background: Some(
            colors
                .surface_container_high
                .scale_alpha(ZOOM_CONTROLS_BACKGROUND_ALPHA * opacity)
                .into(),
        ),
        border: iced::Border {
            color: colors.outline_variant.scale_alpha(opacity),
            width: 1.0,
            radius: ZOOM_CONTROLS_RADIUS.into(),
        },
        shadow: Shadow {
            color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.25 * opacity),
            offset: Vector::new(0.0, 2.0),
            blur_radius: 8.0,
        },
        ..container::Style::default()
    }
}

pub(crate) fn preview_image_frame(
    handle: &image::Handle,
    width: f32,
    height: f32,
) -> image::Image<image::Handle> {
    image::Image::new(handle.clone())
        .width(Length::Fixed(width))
        .height(Length::Fixed(height))
}
