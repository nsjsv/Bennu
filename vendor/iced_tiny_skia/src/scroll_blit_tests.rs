use super::*;
use crate::core::renderer::Renderer as _;
use crate::core::text::{Paragraph as _, Renderer as _};
use crate::core::{
    Border, Color, Font, Pixels, Shadow, Transformation, alignment, renderer,
    text,
};
use crate::graphics::Viewport;
use crate::graphics::damage;
use crate::graphics::text::Paragraph;
use crate::{ClipMask, Renderer};

const WINDOW: Size = Size::new(400.0, 700.0);
const PANE: Rectangle = Rectangle {
    x: 40.0,
    y: 40.0,
    width: 300.0,
    height: 600.0,
};
const ROW: f32 = 30.0;
const ROWS: usize = 40;
const BACKGROUND: Color = Color::from_rgb(0.95, 0.95, 0.97);

/// 一帧：滚动区里的行（条纹、悬停高亮、圆角、文字）按偏移平移；底下是带圆角和边框的窗格底，
/// 上面是跟着位置走的滑块和一块不动的半透明浮层。`first_row` 模拟虚拟列表换窗口。
#[derive(Clone, Copy)]
struct Scene {
    offset: f32,
    hovered: Option<usize>,
    first_row: usize,
}

impl Scene {
    fn at(offset: f32) -> Self {
        Self {
            offset,
            hovered: None,
            first_row: 0,
        }
    }
}

fn quad(bounds: Rectangle, radius: f32, border: f32) -> renderer::Quad {
    renderer::Quad {
        bounds,
        border: Border {
            color: Color::from_rgb(0.5, 0.5, 0.55),
            width: border,
            radius: radius.into(),
        },
        shadow: Shadow::default(),
        snap: false,
    }
}

fn draw_scene(renderer: &mut Renderer, labels: &[Paragraph], scene: Scene) {
    renderer.reset(Rectangle::with_size(WINDOW));
    renderer.fill_quad(quad(PANE, 6.0, 1.0), Color::WHITE);
    renderer.start_layer(PANE);
    renderer
        .start_transformation(Transformation::translate(0.0, -scene.offset));
    // 滚动区交给内容的可见范围（内容坐标），文字按它裁剪。
    let visible = PANE + Vector::new(0.0, scene.offset);
    for row in scene.first_row..scene.first_row + ROWS {
        let bounds = Rectangle::new(
            Point::new(PANE.x + 2.0, PANE.y + row as f32 * ROW),
            Size::new(PANE.width - 4.0, ROW - 1.0),
        );
        let color = if scene.hovered == Some(row) {
            Color::from_rgb(0.6, 0.75, 1.0)
        } else if row % 2 == 0 {
            Color::from_rgb(0.9, 0.92, 0.95)
        } else {
            Color::WHITE
        };
        renderer.fill_quad(
            quad(bounds, if row % 3 == 0 { 3.0 } else { 0.0 }, 0.0),
            color,
        );
        renderer.fill_paragraph(
            &labels[row],
            bounds.position(),
            Color::BLACK,
            visible,
        );
    }
    renderer.end_transformation();
    renderer.end_layer();
    let thumb = Rectangle::new(
        Point::new(PANE.x + PANE.width - 5.0, PANE.y + scene.offset / 4.0),
        Size::new(4.0, 60.0),
    );
    renderer.start_layer(PANE);
    renderer
        .fill_quad(quad(thumb, 2.0, 0.0), Color::from_rgba(0.2, 0.2, 0.2, 0.6));
    renderer.fill_quad(
        quad(
            Rectangle::new(Point::new(120.0, 300.0), Size::new(90.0, 30.0)),
            4.0,
            0.0,
        ),
        Color::from_rgba(0.1, 0.1, 0.1, 0.3),
    );
    renderer.end_layer();
}

/// 每行一段文字，像控件状态一样跨帧复用（段落按身份比较）。
fn labels() -> Vec<Paragraph> {
    (0..ROWS + 16)
        .map(|row| {
            Paragraph::with_text(text::Text {
                content: &format!("row {row}"),
                bounds: Size::new(PANE.width - 4.0, ROW),
                size: Pixels(14.0),
                line_height: text::LineHeight::default(),
                font: Font::DEFAULT,
                align_x: text::Alignment::Left,
                align_y: alignment::Vertical::Top,
                shaping: text::Shaping::Basic,
                wrapping: text::Wrapping::None,
            })
        })
        .collect()
}

struct Frame {
    renderer: Renderer,
    labels: Vec<Paragraph>,
    viewport: Viewport,
    pixels: Vec<u32>,
    clip: ClipMask,
}

impl Frame {
    fn new(scene: Scene, scale_factor: f32) -> Self {
        let physical = Size::new(
            (WINDOW.width * scale_factor).ceil() as u32,
            (WINDOW.height * scale_factor).ceil() as u32,
        );
        let mut frame = Self {
            renderer: Renderer::new(Font::DEFAULT, Pixels(16.0)),
            labels: labels(),
            viewport: Viewport::with_physical_size(physical, scale_factor),
            pixels: vec![0; (physical.width * physical.height) as usize],
            clip: ClipMask::new(physical.width, physical.height).unwrap(),
        };
        draw_scene(&mut frame.renderer, &frame.labels, scene);
        frame.draw(&[Rectangle::with_size(WINDOW)], None);
        frame
    }

    fn draw(&mut self, damage: &[Rectangle], scroll: Option<Scroll>) {
        let size = self.viewport.physical_size();
        let mut pixels = tiny_skia::PixmapMut::from_bytes(
            bytemuck::cast_slice_mut(&mut self.pixels),
            size.width,
            size.height,
        )
        .unwrap();
        if let Some(scroll) = scroll {
            scroll.apply(&mut pixels);
        }
        self.renderer.draw(
            &mut pixels,
            &mut self.clip,
            &self.viewport,
            damage,
            BACKGROUND,
        );
    }

    /// 像合成器那样从上一帧更新到 `scene`：返回这次的计划。
    fn update(&mut self, scene: Scene) -> Plan {
        let previous = self.renderer.layers().to_vec();
        draw_scene(&mut self.renderer, &self.labels, scene);
        let plan = plan(
            &previous,
            self.renderer.layers(),
            self.viewport.scale_factor(),
            self.viewport.physical_size(),
        );
        let damage =
            damage::group(plan.damage.clone(), Rectangle::with_size(WINDOW));
        self.draw(&damage, plan.scroll);
        plan
    }

    /// 上游的做法：不挪，按位置配对的图元差异重画。
    fn update_without_scrolling(&mut self, scene: Scene) {
        let previous = self.renderer.layers().to_vec();
        draw_scene(&mut self.renderer, &self.labels, scene);
        let damage = damage::diff(
            &previous,
            self.renderer.layers(),
            |layer| vec![layer.bounds],
            Layer::damage,
        );
        let damage = damage::group(damage, Rectangle::with_size(WINDOW));
        self.draw(&damage, None);
    }
}

/// 允许的误差：带遮罩画的边缘像素与不带遮罩画的差 1，这是只重画部分区域本来就有的。
const TOLERANCE: u8 = 2;

fn assert_close(
    actual: &Frame,
    expected: &Frame,
    within: Option<Rectangle>,
    what: &str,
    plan: &Plan,
) {
    let size = expected.viewport.physical_size();
    let (x0, y0, x1, y1) = within.map_or(
        (0, 0, size.width as isize, size.height as isize),
        |bounds| pixel_rect(bounds * expected.viewport.scale_factor(), size),
    );
    for y in y0..y1 {
        for x in x0..x1 {
            let index = y as usize * size.width as usize + x as usize;
            let (a, b) = (actual.pixels[index], expected.pixels[index]);
            let close = a
                .to_le_bytes()
                .iter()
                .zip(b.to_le_bytes())
                .all(|(a, b)| a.abs_diff(b) <= TOLERANCE);
            assert!(
                close,
                "{what}: pixel ({x}, {y}) {a:08x} vs {b:08x} at scale {}; plan {plan:?}",
                expected.viewport.scale_factor()
            );
        }
    }
}

/// 从 `from` 更新到 `to`：与上游的只重画部分区域结果一样，滚动区里与整帧重画一样。返回这次的计划。
fn assert_draws_like_a_redraw(
    from: Scene,
    to: Scene,
    scale_factor: f32,
) -> Plan {
    let mut frame = Frame::new(from, scale_factor);
    let plan = frame.update(to);
    let mut upstream = Frame::new(from, scale_factor);
    upstream.update_without_scrolling(to);
    assert_close(
        &frame,
        &upstream,
        None,
        "against redrawing what changed",
        &plan,
    );
    let full = Frame::new(to, scale_factor);
    assert_close(&frame, &full, Some(PANE), "against a full redraw", &plan);
    plan
}

fn damaged_area(plan: &Plan) -> f32 {
    area(&damage::group(
        plan.damage.clone(),
        Rectangle::with_size(WINDOW),
    ))
}

/// 往下、往上滚，连同悬停换行：挪像素后只重画一小部分，结果与整帧重画相同。
#[test]
fn scrolling_moves_pixels_and_redraws_only_what_changed() {
    for scale_factor in [1.0, 2.0] {
        for (from, to) in [
            (Scene::at(0.0), Scene::at(7.0)),
            (Scene::at(40.0), Scene::at(33.0)),
            (
                Scene::at(20.0),
                Scene {
                    hovered: Some(4),
                    ..Scene::at(26.0)
                },
            ),
        ] {
            let plan = assert_draws_like_a_redraw(from, to, scale_factor);
            assert!(plan.scroll.is_some(), "{scale_factor}: {plan:?}");
            assert!(
                damaged_area(&plan) < PANE.area() / 2.0,
                "{scale_factor}: {} {plan:?}",
                damaged_area(&plan)
            );
        }
    }
}

/// 缩放 1.25 时只有挪动量是整数物理像素（4 逻辑像素 = 5 物理像素）才挪；别的照旧整块重画，结果都对。
#[test]
fn fractional_scale_moves_pixels_only_by_whole_physical_pixels() {
    let aligned =
        assert_draws_like_a_redraw(Scene::at(8.0), Scene::at(12.0), 1.25);
    assert!(aligned.scroll.is_some(), "{aligned:?}");
    let unaligned =
        assert_draws_like_a_redraw(Scene::at(8.0), Scene::at(11.0), 1.25);
    assert!(unaligned.scroll.is_none(), "{unaligned:?}");
}

/// 虚拟列表换了窗口（同一位置的图元换成了别的行）、内容没动、只换了悬停：照样画对。
#[test]
fn other_changes_still_draw_the_same_as_a_full_redraw() {
    let _ = assert_draws_like_a_redraw(
        Scene::at(30.0),
        Scene {
            first_row: 2,
            ..Scene::at(35.0)
        },
        1.0,
    );
    let hover = assert_draws_like_a_redraw(
        Scene::at(30.0),
        Scene {
            hovered: Some(5),
            ..Scene::at(30.0)
        },
        1.0,
    );
    assert!(hover.scroll.is_none(), "{hover:?}");
    let _ = assert_draws_like_a_redraw(Scene::at(30.0), Scene::at(30.0), 2.0);
}

/// 挪动区域的像素范围与不抗锯齿填矩形写到的像素一致（层裁剪、重画都这样填）。
#[test]
fn pixel_rect_matches_a_non_antialiased_fill() {
    let size = Size::new(64, 64);
    for bounds in [
        Rectangle::new(Point::new(3.25, 4.5), Size::new(40.75, 20.25)),
        Rectangle::new(Point::new(10.5, 0.5), Size::new(5.0, 63.0)),
        Rectangle::new(Point::new(0.0, 0.0), Size::new(64.0, 64.0)),
        Rectangle::new(Point::new(12.49, 7.51), Size::new(30.02, 9.98)),
        Rectangle::new(Point::new(12.5, 7.5), Size::new(30.0, 10.0)),
    ] {
        let mut mask = tiny_skia::Mask::new(size.width, size.height).unwrap();
        mask.fill_path(
            &tiny_skia::PathBuilder::from_rect(
                tiny_skia::Rect::from_xywh(
                    bounds.x,
                    bounds.y,
                    bounds.width,
                    bounds.height,
                )
                .unwrap(),
            ),
            tiny_skia::FillRule::EvenOdd,
            false,
            tiny_skia::Transform::default(),
        );
        let (x0, y0, x1, y1) = pixel_rect(bounds, size);
        for (index, coverage) in mask.data().iter().enumerate() {
            let (x, y) = ((index % 64) as isize, (index / 64) as isize);
            let inside = (x0..x1).contains(&x) && (y0..y1).contains(&y);
            assert_eq!(*coverage != 0, inside, "{bounds:?} at ({x}, {y})");
        }
    }
}

/// 一串来回、大跳、停住、换悬停的滚动，每一帧都画对。
#[test]
fn a_long_scroll_stays_correct_frame_after_frame() {
    let steps = [
        (3.0, None),
        (10.0, None),
        (10.0, Some(3)),
        (9.0, Some(3)),
        (0.0, None),
        (40.0, None),
        (41.0, Some(7)),
        (300.0, None),
        (296.0, None),
        (900.0, None),
        (600.0, Some(30)),
        (602.0, Some(30)),
        (610.0, None),
    ];
    for scale_factor in [1.0, 2.0, 1.25] {
        let mut moved = 0;
        let mut frame = Frame::new(Scene::at(0.0), scale_factor);
        let mut upstream = Frame::new(Scene::at(0.0), scale_factor);
        for (offset, hovered) in steps {
            let scene = Scene {
                hovered,
                ..Scene::at(offset)
            };
            let plan = frame.update(scene);
            moved += usize::from(plan.scroll.is_some());
            upstream.update_without_scrolling(scene);
            assert_close(
                &frame,
                &upstream,
                None,
                "against redrawing what changed",
                &plan,
            );
            let full = Frame::new(scene, scale_factor);
            assert_close(
                &frame,
                &full,
                Some(PANE),
                "against a full redraw",
                &plan,
            );
        }
        assert!(moved >= 3, "{scale_factor}: moved {moved} times");
    }
}

/// 双缓冲：这块缓冲里是两帧前的画面，就按两帧前的图元算挪多少、重画什么。
#[test]
fn a_buffer_two_frames_old_is_brought_up_to_date() {
    let scale_factor = 1.0;
    let mut renderer = Renderer::new(Font::DEFAULT, Pixels(16.0));
    let labels = labels();
    let template = Frame::new(Scene::at(0.0), scale_factor);
    let size = template.viewport.physical_size();
    let mut buffers: Vec<(Vec<u32>, Vec<Layer>)> = Vec::new();
    for _ in 0..2 {
        draw_scene(&mut renderer, &labels, Scene::at(0.0));
        let mut pixels = vec![0; (size.width * size.height) as usize];
        let mut clip = ClipMask::new(size.width, size.height).unwrap();
        renderer.draw(
            &mut tiny_skia::PixmapMut::from_bytes(
                bytemuck::cast_slice_mut(&mut pixels),
                size.width,
                size.height,
            )
            .unwrap(),
            &mut clip,
            &template.viewport,
            &[Rectangle::with_size(WINDOW)],
            BACKGROUND,
        );
        buffers.push((pixels, renderer.layers().to_vec()));
    }
    let mut clip = ClipMask::new(size.width, size.height).unwrap();
    let mut moved = 0;
    for (frame, offset) in [4.0, 9.0, 15.0, 22.0, 22.0, 18.0, 11.0]
        .into_iter()
        .enumerate()
    {
        let scene = Scene::at(offset);
        draw_scene(&mut renderer, &labels, scene);
        let (pixels, previous) = &mut buffers[frame % 2];
        let plan = plan(previous, renderer.layers(), scale_factor, size);
        let damage =
            damage::group(plan.damage.clone(), Rectangle::with_size(WINDOW));
        let mut pixmap = tiny_skia::PixmapMut::from_bytes(
            bytemuck::cast_slice_mut(pixels),
            size.width,
            size.height,
        )
        .unwrap();
        if let Some(scroll) = plan.scroll {
            scroll.apply(&mut pixmap);
            moved += 1;
        }
        renderer.draw(
            &mut pixmap,
            &mut clip,
            &template.viewport,
            &damage,
            BACKGROUND,
        );
        *previous = renderer.layers().to_vec();
        let mut actual = Frame::new(scene, scale_factor);
        actual.pixels.clone_from(pixels);
        let full = Frame::new(scene, scale_factor);
        assert_close(&actual, &full, Some(PANE), "two frames old", &plan);
    }
    assert!(moved >= 3, "moved {moved} times");
}

/// 滚了 184 像素、同时虚拟列表的窗口挪了 6 行（条纹、圆角正好转一圈）：按位置配对的每一对只差 4 像素、
/// 样子一样，文字一样宽但不是同一行。段落要按身份比，不能当成同一段平移了（实测在分栏里滚动时画面跳动）。
#[test]
fn same_width_labels_of_other_rows_are_not_taken_for_moved_ones() {
    for scale_factor in [1.0, 1.25] {
        let _ = assert_draws_like_a_redraw(
            Scene {
                first_row: 10,
                ..Scene::at(40.0)
            },
            Scene {
                first_row: 16,
                ..Scene::at(224.0)
            },
            scale_factor,
        );
    }
}
