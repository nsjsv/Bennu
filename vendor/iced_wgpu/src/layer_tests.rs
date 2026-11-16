use super::*;
use crate::core::text::Paragraph as _;
use crate::core::{
    Border, Font, Pixels, Shadow, Size, alignment, image as core_image,
};
use crate::graphics::Layer as _;

use std::sync::Arc;

const BOUNDS: Rectangle = Rectangle {
    x: 0.0,
    y: 0.0,
    width: 200.0,
    height: 100.0,
};

fn layer(draw: impl FnOnce(&mut Layer)) -> Layer {
    let mut layer = Layer::with_bounds(BOUNDS);
    draw(&mut layer);
    layer.flush();
    layer
}

fn quad(x: f32) -> renderer::Quad {
    renderer::Quad {
        bounds: Rectangle::new(Point::new(x, 0.0), Size::new(10.0, 10.0)),
        border: Border::default(),
        shadow: Shadow::default(),
        snap: false,
    }
}

fn label(content: &str) -> Paragraph {
    Paragraph::with_text(core::Text {
        content,
        bounds: Size::new(100.0, 20.0),
        size: Pixels(14.0),
        line_height: core::text::LineHeight::default(),
        font: Font::DEFAULT,
        align_x: core::text::Alignment::Left,
        align_y: alignment::Vertical::Top,
        shaping: core::text::Shaping::Basic,
        wrapping: core::text::Wrapping::None,
    })
}

fn picture() -> core_image::Handle {
    core_image::Handle::from_rgba(2, 2, vec![255; 16])
}

fn triangle() -> mesh::Cache {
    mesh::Cache::new(Arc::from(vec![Mesh::Solid {
        buffers: mesh::Indexed {
            vertices: vec![
                mesh::SolidVertex2D {
                    position: [0.0, 0.0],
                    color: color::pack(Color::BLACK),
                };
                3
            ],
            indices: vec![0, 1, 2],
        },
        transformation: Transformation::IDENTITY,
        clip_bounds: BOUNDS,
    }]))
}

/// 一帧画的东西：四边形、一段字、一张图、一个缓存的网格。
struct Scene<'a> {
    quad_x: f32,
    label: &'a Paragraph,
    picture: &'a core_image::Handle,
    triangle: &'a mesh::Cache,
}

fn draw(scene: &Scene<'_>) -> Layer {
    layer(|layer| {
        layer.draw_quad(
            quad(scene.quad_x),
            Background::Color(Color::WHITE),
            Transformation::IDENTITY,
        );
        layer.draw_paragraph(
            scene.label,
            Point::ORIGIN,
            Color::BLACK,
            BOUNDS,
            Transformation::IDENTITY,
        );
        layer.draw_raster(
            core::Image::new(scene.picture.clone()),
            Rectangle::new(Point::new(50.0, 0.0), Size::new(20.0, 20.0)),
            BOUNDS,
            Transformation::IDENTITY,
        );
        layer.draw_mesh_cache(scene.triangle.clone(), Transformation::IDENTITY);
    })
}

/// 同样的东西再画一遍相等；挪了四边形、换了一段同样内容的字、换了一张同样像素的图、网格更新了，都不等。
#[test]
fn only_the_same_drawing_is_equal() {
    let (label, picture, triangle) = (label("row"), picture(), triangle());
    let scene = Scene {
        quad_x: 0.0,
        label: &label,
        picture: &picture,
        triangle: &triangle,
    };
    assert!(draw(&scene) == draw(&scene));

    assert!(
        draw(&scene)
            != draw(&Scene {
                quad_x: 1.0,
                ..scene
            })
    );

    let same_text = self::label("row");
    assert!(
        draw(&scene)
            != draw(&Scene {
                label: &same_text,
                ..scene
            })
    );

    let same_pixels = self::picture();
    assert!(
        draw(&scene)
            != draw(&Scene {
                picture: &same_pixels,
                ..scene
            })
    );

    let mut updated = triangle.clone();
    updated.update(Arc::from(triangle.batch().to_vec()));
    assert!(
        draw(&scene)
            != draw(&Scene {
                triangle: &updated,
                ..scene
            })
    );
}

#[derive(Debug)]
struct Custom;

struct CustomPipeline;

impl primitive::Pipeline for CustomPipeline {
    fn new(
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _format: wgpu::TextureFormat,
    ) -> Self {
        Self
    }
}

impl Primitive for Custom {
    type Pipeline = CustomPipeline;

    fn prepare(
        &self,
        _pipeline: &mut CustomPipeline,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _bounds: &Rectangle,
        _viewport: &graphics::Viewport,
    ) {
    }
}

/// 自定义着色器图元比较不了内容：有它的层永远不等。
#[test]
fn layers_with_custom_primitives_are_never_equal() {
    let with_custom = || {
        layer(|layer| {
            layer.draw_primitive(BOUNDS, Custom, Transformation::IDENTITY);
        })
    };
    assert!(with_custom() != with_custom());
}
