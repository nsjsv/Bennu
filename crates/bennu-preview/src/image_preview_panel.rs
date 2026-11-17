//! 静态图片预览面板（缩略图/光栅原图/SVG 原图）：缩放平移媒体区、透明处的棋盘格、
//! 底部浮着的缩放控件。交互契约见 image-preview-guidelines 第一节（视口钳制/锚点缩放/双击）。

use iced::widget::image::FilterMethod;
use iced::widget::{container, image, row, svg, Space, Stack};
use iced::{Alignment, ContentFit, Element, Length, Size, Vector};

use bennu_theme::icons::IconSymbol;
use bennu_theme::preview_styles::preview_media_style;
use bennu_theme::translated_surface::translated_surface;

use crate::checkerboard::checkerboard;
use crate::image_info_text::image_info_rows;
use crate::image_preview_viewport::{ImagePreviewViewport, PreviewImageViewportMessage};
use crate::info_overlay::info_overlay;
use crate::media_controls::{
    floating_bar, floating_bar_text, floating_icon_button, floating_text_button,
};
use crate::pointer_area::pointer_area;
use crate::preview::{image_preview_size, ImageInfo, ImagePreviewContent, PreviewSize};
use crate::preview_message::PreviewMessage;
use crate::zoom_tiles::{tile_layer, TileView, ZoomTiles};

const ZOOM_PERCENT_WIDTH: f32 = 52.0;
/// 百分比和 100% 差这么多以内算 1:1（缩放倍率是浮点连乘出来的）。
const ACTUAL_SIZE_TOLERANCE_PERCENT: f32 = 0.5;

pub(crate) fn image_preview_panel<Message>(
    content: &ImagePreviewContent,
    size: PreviewSize,
    viewport: &ImagePreviewViewport,
    display_scale: f32,
    controls_opacity: f32,
    info: Option<&ImageInfo>,
    zoom_tiles: Option<&ZoomTiles>,
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
            Some(viewport.zoom_percent(*width, Size::new(fit_width, fit_height), display_scale))
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
    // 超大图放大后，屏幕大小那张垫在下面，看得见的块盖在上面（几何和这里摆图同一套）。
    let tile_view = TileView {
        panel: Size::new(size.width, size.height),
        fit: Size::new(fit_width, fit_height),
        scale: viewport.scale,
        offset: viewport.offset,
        display_scale,
    };
    let media = zoomable_media_area(fit_width, fit_height, size, viewport, |width, height| {
        let layers = image_layers(content, width, height, filter);
        match zoom_tiles.and_then(|tiles| tile_layer(tiles, &tile_view, filter)) {
            Some(tiles) => Stack::with_children([layers, tiles])
                .width(Length::Fixed(width))
                .height(Length::Fixed(height))
                .into(),
            None => layers,
        }
    });

    let mut layers = Stack::with_children([media])
        .width(Length::Fill)
        .height(Length::Fill);
    if let Some(info) = info {
        layers = layers.push(info_overlay(image_info_rows(content, info)));
    }
    if let Some(percent) = percent {
        if controls_opacity > f32::EPSILON {
            layers = layers.push(zoom_controls_layer(percent, controls_opacity));
        }
    }

    // 指针事件包住整块（含控件）：指针移到控件上不算离开，右侧面板的控件不会一碰就消失。
    // 缩放锚点随滚轮、双击一起带过去；逐次的移动只在平移时要。
    let viewport_message = |message| Message::from(PreviewMessage::PreviewImageViewport(message));
    let area = pointer_area(layers)
        .on_exit(viewport_message(PreviewImageViewportMessage::PointerLeft))
        .on_press(move |at| viewport_message(PreviewImageViewportMessage::PanStarted(at)))
        .on_release(viewport_message(PreviewImageViewportMessage::PanEnded))
        .on_double_click(move |at| viewport_message(PreviewImageViewportMessage::DoubleClicked(at)))
        .on_scroll(move |delta, at| {
            viewport_message(PreviewImageViewportMessage::Zoomed(delta, at))
        })
        .interaction(iced::mouse::Interaction::Grab);
    if viewport.panning {
        area.on_move(move |position| {
            viewport_message(PreviewImageViewportMessage::PointerMoved(position))
        })
        .into()
    } else {
        area.into()
    }
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

/// 底部浮条：缩小、比例、放大、适应窗口/1:1、信息。
fn zoom_controls_layer<Message>(percent: f32, opacity: f32) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let at_actual_size = (percent - 100.0).abs() <= ACTUAL_SIZE_TOLERANCE_PERCENT;
    let viewport = |message| Message::from(PreviewMessage::PreviewImageViewport(message));
    // 在 1:1 时按钮回到适应窗口，否则去 1:1。
    let toggle: Element<'static, Message> = if at_actual_size {
        floating_icon_button(
            IconSymbol::Maximize,
            viewport(PreviewImageViewportMessage::FitOrActualToggled),
            opacity,
        )
        .into()
    } else {
        floating_text_button(
            "1:1".to_owned(),
            viewport(PreviewImageViewportMessage::FitOrActualToggled),
            opacity,
        )
    };
    let controls = row![
        floating_icon_button(
            IconSymbol::ZoomOut,
            viewport(PreviewImageViewportMessage::ZoomedOut),
            opacity
        ),
        floating_bar_text(format!("{percent:.0}%"), ZOOM_PERCENT_WIDTH, opacity),
        floating_icon_button(
            IconSymbol::ZoomIn,
            viewport(PreviewImageViewportMessage::ZoomedIn),
            opacity
        ),
        Space::new().width(Length::Fixed(4.0)),
        toggle,
        floating_icon_button(
            IconSymbol::Info,
            Message::from(PreviewMessage::ImageInfoToggled),
            opacity
        ),
    ]
    .spacing(2)
    .align_y(Alignment::Center);
    floating_bar(controls.into(), opacity)
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
