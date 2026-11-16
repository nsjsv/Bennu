use std::path::PathBuf;

use iced::widget::image;

use crate::engine::test_support::test_engine;
use crate::engine::{PreviewEngine, PreviewLoadSurface};
use crate::image_preview_viewport::PreviewImageViewportMessage;
use crate::original_image_preview::OriginalImagePreview;
use crate::preview::{
    DisplayMetrics, ImagePreviewContent, PreviewContent, PreviewSize, PreviewState,
};

const PANEL: PreviewSize = PreviewSize {
    width: 800.0,
    height: 600.0,
};

fn engine() -> PreviewEngine {
    test_engine(PreviewLoadSurface::RightDockedPanel)
}

/// 原图 3200×2400，按屏幕大小解成 1600×1200。
fn screen_sized_photo(engine: &mut PreviewEngine) -> (PathBuf, u64) {
    let path = PathBuf::from("/workspace/photo.jpg");
    let generation = engine.next_original_image_preview_generation();
    engine.preview = Some(PreviewState::Ready(PreviewContent::Image(
        ImagePreviewContent::OriginalRaster {
            path: path.clone(),
            raster_handle: image::Handle::from_rgba(1, 1, vec![0; 4]),
            placeholder_handle: image::Handle::from_rgba(1, 1, vec![0; 4]),
            width: 3200,
            height: 2400,
            pixel_width: 1600,
            has_alpha: false,
        },
    )));
    (path, generation)
}

fn full_photo() -> OriginalImagePreview {
    OriginalImagePreview::Raster {
        raster_handle: image::Handle::from_rgba(1, 1, vec![0; 4]),
        placeholder_handle: image::Handle::from_rgba(1, 1, vec![0; 4]),
        width: 3200,
        height: 2400,
        pixel_width: 3200,
        has_alpha: false,
    }
}

fn shown_pixel_width(engine: &PreviewEngine) -> u32 {
    match &engine.preview {
        Some(PreviewState::Ready(PreviewContent::Image(ImagePreviewContent::OriginalRaster {
            pixel_width,
            ..
        }))) => *pixel_width,
        other => panic!("not a raster: {other:?}"),
    }
}

#[test]
fn actual_size_follows_the_display_scale_and_toggles_back_to_fit() {
    let mut engine = engine();
    drop(engine.accept_preview_display(Some(DisplayMetrics {
        pixels: iced::Size::new(2560, 1600),
        scale_factor: 2.0,
    })));
    screen_sized_photo(&mut engine);

    // 适应窗口 800 宽、两倍缩放：3200 像素的 1:1 是 2 倍。
    drop(
        engine
            .update_preview_image_viewport(PreviewImageViewportMessage::ActualSizeRequested, PANEL),
    );
    assert_eq!(engine.preview_image_viewport.scale, 2.0);

    drop(
        engine
            .update_preview_image_viewport(PreviewImageViewportMessage::FitOrActualToggled, PANEL),
    );
    assert!(!engine.preview_image_viewport.is_zoomed());
    assert_eq!(engine.display_scale(), 2.0);
    assert_eq!(
        engine.image_decode_bound().fit(10_000, 10_000),
        (1600, 1600)
    );
}

#[test]
fn magnifying_past_the_decoded_pixels_asks_for_the_original_once() {
    let mut engine = engine();
    let (path, generation) = screen_sized_photo(&mut engine);

    // 1.5 倍还在 1600 像素以内。
    let task = engine.update_preview_image_viewport(
        PreviewImageViewportMessage::Zoomed(
            iced::mouse::ScrollDelta::Lines { x: 0.0, y: 4.0 },
            iced::Point::new(10.0, 10.0),
        ),
        PANEL,
    );
    assert_eq!(task.units(), 0);
    let task = engine
        .update_preview_image_viewport(PreviewImageViewportMessage::ActualSizeRequested, PANEL);
    assert_eq!(task.units(), 1);
    let again = engine.update_preview_image_viewport(PreviewImageViewportMessage::ZoomedIn, PANEL);
    assert_eq!(again.units(), 0);

    let scale = engine.preview_image_viewport.scale;
    drop(engine.accept_original_image_preview(path.clone(), generation, Ok(full_photo())));
    assert_eq!(shown_pixel_width(&engine), 3200);
    assert_eq!(engine.preview_image_viewport.scale, scale);
}

#[test]
fn an_original_nobody_asked_for_does_not_replace_the_shown_image() {
    let mut engine = engine();
    let (path, generation) = screen_sized_photo(&mut engine);
    drop(engine.accept_original_image_preview(path, generation, Ok(full_photo())));
    assert_eq!(shown_pixel_width(&engine), 1600);
}

#[test]
fn docked_image_controls_show_only_while_the_pointer_is_over_the_image() {
    let mut engine = engine();
    screen_sized_photo(&mut engine);
    assert_eq!(engine.docked_panel_controls_opacity(), 0.0);
    drop(engine.update_preview_image_viewport(
        PreviewImageViewportMessage::PointerMoved(iced::Point::new(10.0, 10.0)),
        PANEL,
    ));
    assert_eq!(engine.docked_panel_controls_opacity(), 1.0);
    drop(engine.update_preview_image_viewport(PreviewImageViewportMessage::PointerLeft, PANEL));
    assert_eq!(engine.docked_panel_controls_opacity(), 0.0);
}
