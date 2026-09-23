//! 静态图片预览面板（缩略图/光栅原图/SVG 原图）与缩放平移媒体区。
//! 自 app-ui view/preview_panel.rs 下沉，逐字节保真；交互契约见
//! image-preview-guidelines 第一节（视口钳制/锚点缩放/双击重置）。

use iced::widget::{container, image, mouse_area, svg, Stack};
use iced::{ContentFit, Element, Length, Vector};

use bennu_theme::preview_styles::preview_media_style;
use bennu_theme::translated_surface::translated_surface;

use crate::image_preview_viewport::{ImagePreviewViewport, PreviewImageViewportMessage};
use crate::preview::{image_preview_size, PreviewSize};
use crate::preview_message::PreviewMessage;

pub(crate) fn image_preview_panel<Message>(
    handle: &image::Handle,
    width: u32,
    height: u32,
    size: PreviewSize,
    viewport: &ImagePreviewViewport,
) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let (fit_width, fit_height) = image_preview_size(size, width, height);
    zoomable_media_area(
        fit_width,
        fit_height,
        size,
        viewport,
        |image_width, image_height| preview_image_frame(handle, image_width, image_height).into(),
    )
}

pub(crate) fn raster_image_preview_panel<Message>(
    placeholder_handle: &image::Handle,
    raster_handle: &image::Handle,
    width: u32,
    height: u32,
    size: PreviewSize,
    viewport: &ImagePreviewViewport,
) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let (fit_width, fit_height) = image_preview_size(size, width, height);
    zoomable_media_area(
        fit_width,
        fit_height,
        size,
        viewport,
        |image_width, image_height| {
            let image = image::Image::new(raster_handle.clone())
                .width(Length::Fixed(image_width))
                .height(Length::Fixed(image_height))
                .content_fit(ContentFit::Contain);
            let placeholder = image::Image::new(placeholder_handle.clone())
                .width(Length::Fixed(image_width))
                .height(Length::Fixed(image_height))
                .content_fit(ContentFit::Contain);
            Stack::new()
                .width(Length::Fixed(image_width))
                .height(Length::Fixed(image_height))
                .push(placeholder)
                .push(image)
                .into()
        },
    )
}

pub(crate) fn svg_preview_panel<Message>(
    handle: &svg::Handle,
    width: u32,
    height: u32,
    size: PreviewSize,
    viewport: &ImagePreviewViewport,
) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
{
    let (fit_width, fit_height) = image_preview_size(size, width, height);
    zoomable_media_area(
        fit_width,
        fit_height,
        size,
        viewport,
        |render_width, render_height| {
            svg::Svg::new(handle.clone())
                .width(Length::Fixed(render_width))
                .height(Length::Fixed(render_height))
                .content_fit(ContentFit::Contain)
                .into()
        },
    )
}

/// 静态图片媒体区：适应窗口时居中；缩放后按视口位移摆放，
/// 并用 mouse_area 提供滚轮缩放、拖动平移与双击重置。
fn zoomable_media_area<Message>(
    fit_width: f32,
    fit_height: f32,
    size: PreviewSize,
    viewport: &ImagePreviewViewport,
    content_at_scaled_size: impl Fn(f32, f32) -> Element<'static, Message>,
) -> Element<'static, Message>
where
    Message: 'static + Clone + From<PreviewMessage>,
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

    mouse_area(
        container(media)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(preview_media_style),
    )
    .on_move(|position| {
        Message::from(PreviewMessage::PreviewImageViewport(
            PreviewImageViewportMessage::PointerMoved(position),
        ))
    })
    .on_press(Message::from(PreviewMessage::PreviewImageViewport(
        PreviewImageViewportMessage::PanStarted,
    )))
    .on_release(Message::from(PreviewMessage::PreviewImageViewport(
        PreviewImageViewportMessage::PanEnded,
    )))
    .on_double_click(Message::from(PreviewMessage::PreviewImageViewport(
        PreviewImageViewportMessage::ResetRequested,
    )))
    .on_scroll(|delta| {
        Message::from(PreviewMessage::PreviewImageViewport(
            PreviewImageViewportMessage::Zoomed(delta),
        ))
    })
    .interaction(iced::mouse::Interaction::Grab)
    .into()
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
