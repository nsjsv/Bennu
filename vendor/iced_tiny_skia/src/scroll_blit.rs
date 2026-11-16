//! Bennu 补丁：滚动时把画好的像素整块挪过去，只重画露出的一条和真正变了的地方。
//!
//! 上游按「前后两帧同一位置的图元是否相同」算要重画的区域。滚动时内容整体平移，每个图元都不同，
//! 整个滚动区每帧重画（分栏一列约 3.6 ms）。这里先找整层只是平移了的那一层：把旧帧这层的图元平移
//! 后与新帧逐个比，大多相同就把这层的像素整块挪过去，再重画：
//!
//! - 露出的一条：挪进来的地方原来在层外，没画过；
//! - 这层里平移后仍不同的图元：悬停高亮、贴边裁剪变了的行；
//! - 其他层压在这块区域里的图元：它们的像素被一起挪了，旧的落到了挪后的位置、新的位置要补画。
//!   整块盖住这块区域的纯色底（窗格背景）挪了也一样，只补圆角、边框和垂直于挪动方向的边。
//!
//! 只在挪动量正好是整数物理像素时挪：图元平移整数像素后栅格化结果逐像素相同（文字的亚像素位置、
//! 抗锯齿边都一样）；层的裁剪按像素中心取整（不抗锯齿），挪动区域的边界与重画区域对得上。

use crate::Primitive;
use crate::core::renderer::Quad;
use crate::core::{Background, Point, Rectangle, Size, Vector};
use crate::graphics::Image;
use crate::graphics::text::Text;
use crate::layer::{Item, Layer};

/// 挪了要重画的面积不到不挪的这么多才挪。
const WORTH: f32 = 0.5;

/// 一层画好的像素在物理像素里整块挪动：目标区域 [left, right) × [top, bottom)，内容挪了 (dx, dy)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scroll {
    left: usize,
    top: usize,
    right: usize,
    bottom: usize,
    dx: isize,
    dy: isize,
}

/// 这一帧怎么更新：先挪（有的话），再重画 `damage`（逻辑坐标）。
#[derive(Debug)]
pub struct Plan {
    pub scroll: Option<Scroll>,
    pub damage: Vec<Rectangle>,
}

pub fn plan(
    previous: &[Layer],
    current: &[Layer],
    scale_factor: f32,
    size: Size<u32>,
) -> Plan {
    let plain = plain_damage(previous, current);
    let unscrolled = || Plan {
        scroll: None,
        damage: plain.concat(),
    };
    let Some((index, shift)) = previous
        .iter()
        .zip(current)
        .enumerate()
        .filter_map(|(index, (old, new))| {
            shifted_layer(old, new, &plain[index], scale_factor)
                .map(|shift| (index, shift))
        })
        .min_by(|(_, a), (_, b)| a.cost.total_cmp(&b.cost))
    else {
        return unscrolled();
    };
    let bounds = current[index].bounds;
    let Some(region) = bounds.intersection(&(bounds + shift.offset)) else {
        return unscrolled();
    };
    let mut damage: Vec<Rectangle> = plain
        .iter()
        .enumerate()
        .filter(|(other, _)| *other != index)
        .flat_map(|(_, damage)| damage.iter().copied())
        .collect();
    damage.extend(shift.damage);
    for (other, layer) in previous.iter().enumerate() {
        if other != index {
            moved_along(layer, region, shift.offset, &mut damage);
        }
    }
    for (other, layer) in current.iter().enumerate() {
        if other != index {
            drawn_over(layer, region, shift.offset, &mut damage);
        }
    }
    if area(&damage) >= area(&plain.concat()) * WORTH {
        return unscrolled();
    }
    match Scroll::new(bounds, shift.offset, scale_factor, size) {
        Some(scroll) => Plan {
            scroll: Some(scroll),
            damage,
        },
        None => unscrolled(),
    }
}

impl Scroll {
    fn new(
        bounds: Rectangle,
        offset: Vector,
        scale_factor: f32,
        size: Size<u32>,
    ) -> Option<Self> {
        let (x0, y0, x1, y1) = pixel_rect(bounds * scale_factor, size);
        let dx = (offset.x * scale_factor).round() as isize;
        let dy = (offset.y * scale_factor).round() as isize;
        let left = x0.max(x0 + dx);
        let right = x1.min(x1 + dx);
        let top = y0.max(y0 + dy);
        let bottom = y1.min(y1 + dy);
        (left < right && top < bottom).then(|| Self {
            left: left as usize,
            top: top as usize,
            right: right as usize,
            bottom: bottom as usize,
            dx,
            dy,
        })
    }

    pub fn apply(&self, pixels: &mut tiny_skia::PixmapMut<'_>) {
        let stride = pixels.width() as usize * 4;
        let row = (self.right - self.left) * 4;
        let source_left = (self.left as isize - self.dx) as usize * 4;
        let data = pixels.data_mut();
        let mut copy = |y: usize| {
            let source = (y as isize - self.dy) as usize * stride + source_left;
            data.copy_within(source..source + row, y * stride + self.left * 4);
        };
        // 往下挪时从下往上拷，往上挪时从上往下拷，拷到的行不会先被覆盖。
        if self.dy > 0 {
            (self.top..self.bottom).rev().for_each(&mut copy);
        } else {
            (self.top..self.bottom).for_each(&mut copy);
        }
    }
}

/// 不抗锯齿地填一个矩形（物理像素）会写到的像素范围：四条边各自四舍五入到像素边界。
fn pixel_rect(
    bounds: Rectangle,
    size: Size<u32>,
) -> (isize, isize, isize, isize) {
    let edge = |value: f32, max: u32| {
        (value.round().max(0.0) as isize).min(max as isize)
    };
    (
        edge(bounds.x, size.width),
        edge(bounds.y, size.height),
        edge(bounds.x + bounds.width, size.width),
        edge(bounds.y + bounds.height, size.height),
    )
}

/// 不挪时每对层（按位置配对）要重画的区域，与上游的算法相同；多出来、少掉的层整层重画（放在最后一项）。
fn plain_damage(previous: &[Layer], current: &[Layer]) -> Vec<Vec<Rectangle>> {
    let mut damage: Vec<Vec<Rectangle>> = previous
        .iter()
        .zip(current)
        .map(|(old, new)| Layer::damage(old, new))
        .collect();
    let paired = damage.len();
    damage.push(
        previous[paired..]
            .iter()
            .chain(&current[paired..])
            .map(|layer| layer.bounds)
            .collect(),
    );
    damage
}

struct Shift {
    offset: Vector,
    damage: Vec<Rectangle>,
    cost: f32,
}

/// 这层像是整体平移了：按多数图元的位移挪，挪了要重画的比不挪少一半以上才算。
fn shifted_layer(
    old: &Layer,
    new: &Layer,
    plain: &[Rectangle],
    scale_factor: f32,
) -> Option<Shift> {
    let bounds = new.bounds;
    if old.bounds != bounds || !bounds.area().is_finite() {
        return None;
    }
    let before = area(plain);
    // 这层本来就没怎么变，不值得找。
    if before < bounds.area() / 4.0 {
        return None;
    }
    let offset = majority_offset(old, new)?;
    let physical = offset * scale_factor;
    if (physical.x - physical.x.round()).abs() > 1e-3
        || (physical.y - physical.y.round()).abs() > 1e-3
    {
        return None;
    }
    let mut damage = shifted_damage(old, new, offset);
    damage.extend(exposed(bounds, offset));
    let cost = area(&damage);
    (cost < before * WORTH).then_some(Shift {
        offset,
        damage,
        cost,
    })
}

/// 同一位置的图元前后两帧的位移里最多的那个（不动的不算）。
fn majority_offset(old: &Layer, new: &Layer) -> Option<Vector> {
    let mut votes: Vec<(Vector, usize)> = Vec::new();
    let quads = old.quads.iter().zip(&new.quads).filter_map(
        |((quad_a, background_a), (quad_b, background_b))| {
            (background_a == background_b
                && quad_a.bounds.size() == quad_b.bounds.size())
            .then(|| quad_b.bounds.position() - quad_a.bounds.position())
        },
    );
    let text = old.text.iter().zip(&new.text).filter_map(|(a, b)| {
        let (Item::Live(a), Item::Live(b)) = (a, b) else {
            return None;
        };
        Some(b.visible_bounds()?.position() - a.visible_bounds()?.position())
    });
    for offset in quads.chain(text).filter(|offset| *offset != Vector::ZERO) {
        match votes.iter_mut().find(|(candidate, _)| *candidate == offset) {
            Some((_, count)) => *count += 1,
            None => votes.push((offset, 1)),
        }
    }
    votes
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(offset, _)| offset)
}

/// 旧帧这层平移 `offset` 后与新帧逐个比（按位置配对）：不同的图元在两处都要重画。
fn shifted_damage(old: &Layer, new: &Layer, offset: Vector) -> Vec<Rectangle> {
    let bounds = new.bounds;
    let mut damage = Vec::new();
    shifted_list(
        &old.quads,
        &new.quads,
        offset,
        |(quad, _)| vec![quad.bounds.expand(1.0)],
        |(quad_a, background_a), (quad_b, background_b)| {
            background_a == background_b
                && shifted_quad(quad_a, offset) == *quad_b
        },
        &mut damage,
    );
    shifted_list(
        &old.text,
        &new.text,
        offset,
        text_bounds,
        |a, b| match (a, b) {
            (Item::Live(a), Item::Live(b)) => same_text(a, b, offset, bounds),
            _ => false,
        },
        &mut damage,
    );
    shifted_list(
        &old.images,
        &new.images,
        offset,
        |image| vec![image.bounds().expand(1.0)],
        |a, b| same_image(a, b, offset, bounds),
        &mut damage,
    );
    // 几何图元（画布）不认平移，照旧逐个比。
    shifted_list(
        &old.primitives,
        &new.primitives,
        offset,
        primitive_bounds,
        |_, _| false,
        &mut damage,
    );
    damage
        .into_iter()
        .filter_map(|region| region.intersection(&bounds))
        .collect()
}

fn shifted_list<T>(
    previous: &[T],
    current: &[T],
    offset: Vector,
    bounds: impl Fn(&T) -> Vec<Rectangle>,
    same: impl Fn(&T, &T) -> bool,
    damage: &mut Vec<Rectangle>,
) {
    for (a, b) in previous.iter().zip(current) {
        if !same(a, b) {
            damage.extend(bounds(a).into_iter().map(|region| region + offset));
            damage.extend(bounds(b));
        }
    }
    let paired = previous.len().min(current.len());
    damage.extend(
        previous[paired..]
            .iter()
            .flat_map(&bounds)
            .map(|region| region + offset),
    );
    damage.extend(current[paired..].iter().flat_map(&bounds));
}

fn shifted_quad(quad: &Quad, offset: Vector) -> Quad {
    Quad {
        bounds: quad.bounds + offset,
        ..*quad
    }
}

/// 裁剪框跟着平移了，或者前后都盖住整层（滚动区可见范围这种在窗口里不动的裁剪）：在这层里裁出来的一样。
fn same_clip(
    a: Rectangle,
    b: Rectangle,
    offset: Vector,
    layer: Rectangle,
) -> bool {
    a + offset == b || (layer.is_within(&a) && layer.is_within(&b))
}

fn same_text(a: &Text, b: &Text, offset: Vector, layer: Rectangle) -> bool {
    match (a, b) {
        (
            Text::Paragraph {
                paragraph: paragraph_a,
                position: position_a,
                color: color_a,
                clip_bounds: clip_a,
                transformation: transformation_a,
            },
            Text::Paragraph {
                paragraph: paragraph_b,
                position: position_b,
                color: color_b,
                clip_bounds: clip_b,
                transformation: transformation_b,
            },
        ) => {
            color_a == color_b
                && transformation_a.scale_factor()
                    == transformation_b.scale_factor()
                && *position_a * *transformation_a + offset
                    == *position_b * *transformation_b
                && same_clip(
                    *clip_a * *transformation_a,
                    *clip_b * *transformation_b,
                    offset,
                    layer,
                )
                && paragraph_a == paragraph_b
        }
        (
            Text::Editor {
                editor: editor_a,
                position: position_a,
                color: color_a,
                clip_bounds: clip_a,
                transformation: transformation_a,
            },
            Text::Editor {
                editor: editor_b,
                position: position_b,
                color: color_b,
                clip_bounds: clip_b,
                transformation: transformation_b,
            },
        ) => {
            color_a == color_b
                && transformation_a.scale_factor()
                    == transformation_b.scale_factor()
                && *position_a * *transformation_a + offset
                    == *position_b * *transformation_b
                && same_clip(
                    *clip_a * *transformation_a,
                    *clip_b * *transformation_b,
                    offset,
                    layer,
                )
                && editor_a == editor_b
        }
        (
            Text::Cached {
                content: content_a,
                bounds: bounds_a,
                color: color_a,
                size: size_a,
                line_height: line_height_a,
                font: font_a,
                align_x: align_x_a,
                align_y: align_y_a,
                shaping: shaping_a,
                clip_bounds: clip_a,
            },
            Text::Cached {
                content: content_b,
                bounds: bounds_b,
                color: color_b,
                size: size_b,
                line_height: line_height_b,
                font: font_b,
                align_x: align_x_b,
                align_y: align_y_b,
                shaping: shaping_b,
                clip_bounds: clip_b,
            },
        ) => {
            *bounds_a + offset == *bounds_b
                && same_clip(*clip_a, *clip_b, offset, layer)
                && (color_a, size_a, line_height_a, font_a)
                    == (color_b, size_b, line_height_b, font_b)
                && (align_x_a, align_y_a, shaping_a)
                    == (align_x_b, align_y_b, shaping_b)
                && content_a == content_b
        }
        _ => false,
    }
}

fn same_image(a: &Image, b: &Image, offset: Vector, layer: Rectangle) -> bool {
    match (a, b) {
        (
            Image::Raster {
                image: image_a,
                bounds: bounds_a,
                clip_bounds: clip_a,
            },
            Image::Raster {
                image: image_b,
                bounds: bounds_b,
                clip_bounds: clip_b,
            },
        ) => {
            *bounds_a + offset == *bounds_b
                && same_clip(*clip_a, *clip_b, offset, layer)
                && image_a == image_b
        }
        (
            Image::Vector {
                svg: svg_a,
                bounds: bounds_a,
                clip_bounds: clip_a,
            },
            Image::Vector {
                svg: svg_b,
                bounds: bounds_b,
                clip_bounds: clip_b,
            },
        ) => {
            *bounds_a + offset == *bounds_b
                && same_clip(*clip_a, *clip_b, offset, layer)
                && svg_a == svg_b
        }
        _ => false,
    }
}

/// 挪进这块区域的像素原来在 `region - offset`：旧帧其他层在那儿的图元被一起挪了，落到的地方要重画。
fn moved_along(
    layer: &Layer,
    region: Rectangle,
    offset: Vector,
    damage: &mut Vec<Rectangle>,
) {
    let source = region - offset;
    for_each_item(layer, source, offset, |bounds, uniform| match uniform {
        Some(zone) => sweep(zone, region, offset, damage),
        None => damage.extend((bounds + offset).intersection(&region)),
    });
}

/// 新帧其他层压在这块区域里的图元要补画到挪过来的像素上。
fn drawn_over(
    layer: &Layer,
    region: Rectangle,
    offset: Vector,
    damage: &mut Vec<Rectangle>,
) {
    for_each_item(layer, region, offset, |bounds, uniform| match uniform {
        Some(zone) => sweep(zone, region, offset, damage),
        None => damage.extend(bounds.intersection(&region)),
    });
}

/// 圆角、边框这些挪了不一样的地方，连同它挪过去扫过的范围。
fn sweep(
    zone: Vec<Rectangle>,
    region: Rectangle,
    offset: Vector,
    damage: &mut Vec<Rectangle>,
) {
    damage.extend(
        zone.into_iter().filter_map(|part| {
            part.union(&(part + offset)).intersection(&region)
        }),
    );
}

/// 层里落在 `area` 内的图元（按层裁剪后）：纯色、整块盖住 `area` 的矩形另给出挪了不一样的部分。
fn for_each_item(
    layer: &Layer,
    area: Rectangle,
    offset: Vector,
    mut visit: impl FnMut(Rectangle, Option<Vec<Rectangle>>),
) {
    let Some(area) = area.intersection(&layer.bounds) else {
        return;
    };
    let mut visit_bounds =
        |bounds: Rectangle, uniform: Option<Vec<Rectangle>>| {
            if let Some(bounds) = bounds.intersection(&layer.bounds) {
                if bounds.intersects(&area) {
                    visit(bounds, uniform);
                }
            }
        };
    for (quad, background) in &layer.quads {
        visit_bounds(
            quad.bounds.expand(1.0),
            uniform_zone(quad, background, area, offset),
        );
    }
    for item in &layer.text {
        text_bounds(item)
            .into_iter()
            .for_each(|bounds| visit_bounds(bounds, None));
    }
    for image in &layer.images {
        visit_bounds(image.bounds().expand(1.0), None);
    }
    for item in &layer.primitives {
        primitive_bounds(item)
            .into_iter()
            .for_each(|bounds| visit_bounds(bounds, None));
    }
}

/// 纯色、整块盖住 `area` 的矩形里像素到处一样，挪了也一样；不一样的只有圆角，以及垂直于挪动方向
/// 的边（边框和抗锯齿的那几行）。平行于挪动方向的边沿着自己挪，不变。
fn uniform_zone(
    quad: &Quad,
    background: &Background,
    area: Rectangle,
    offset: Vector,
) -> Option<Vec<Rectangle>> {
    let Background::Color(color) = background else {
        return None;
    };
    // 半透明的底下透出阴影，靠边处不均匀。
    if !area.is_within(&quad.bounds)
        || (quad.shadow.color.a > 0.0 && color.a < 1.0)
    {
        return None;
    }
    let bounds = quad.bounds;
    let border = if quad.border.color.a > 0.0 {
        quad.border.width
    } else {
        0.0
    };
    let edge = border + 1.0;
    let radius = quad.border.radius;
    let corners = [
        (radius.top_left, bounds.x, bounds.y),
        (radius.top_right, bounds.x + bounds.width, bounds.y),
        (
            radius.bottom_right,
            bounds.x + bounds.width,
            bounds.y + bounds.height,
        ),
        (radius.bottom_left, bounds.x, bounds.y + bounds.height),
    ];
    let mut zone: Vec<Rectangle> = corners
        .into_iter()
        .filter(|(radius, _, _)| radius.max(border) > 0.0)
        .map(|(radius, x, y)| {
            let reach = radius.max(border) + 1.0;
            Rectangle::new(
                Point::new(x - reach, y - reach),
                Size::new(reach * 2.0, reach * 2.0),
            )
        })
        .collect();
    if offset.y != 0.0 {
        zone.extend([
            Rectangle::new(
                Point::new(bounds.x - 1.0, bounds.y - 1.0),
                Size::new(bounds.width + 2.0, edge + 1.0),
            ),
            Rectangle::new(
                Point::new(bounds.x - 1.0, bounds.y + bounds.height - edge),
                Size::new(bounds.width + 2.0, edge + 1.0),
            ),
        ]);
    }
    if offset.x != 0.0 {
        zone.extend([
            Rectangle::new(
                Point::new(bounds.x - 1.0, bounds.y - 1.0),
                Size::new(edge + 1.0, bounds.height + 2.0),
            ),
            Rectangle::new(
                Point::new(bounds.x + bounds.width - edge, bounds.y - 1.0),
                Size::new(edge + 1.0, bounds.height + 2.0),
            ),
        ]);
    }
    Some(zone)
}

/// 挪进来的地方原来在层外，没画过。往里多算一像素，物理像素取整时不漏。
fn exposed(bounds: Rectangle, offset: Vector) -> Vec<Rectangle> {
    let mut strips = Vec::new();
    if offset.y != 0.0 {
        let height = offset.y.abs().min(bounds.height) + 1.0;
        let y = if offset.y > 0.0 {
            bounds.y
        } else {
            bounds.y + bounds.height - height
        };
        strips.push(Rectangle::new(
            Point::new(bounds.x, y),
            Size::new(bounds.width, height),
        ));
    }
    if offset.x != 0.0 {
        let width = offset.x.abs().min(bounds.width) + 1.0;
        let x = if offset.x > 0.0 {
            bounds.x
        } else {
            bounds.x + bounds.width - width
        };
        strips.push(Rectangle::new(
            Point::new(x, bounds.y),
            Size::new(width, bounds.height),
        ));
    }
    strips
        .into_iter()
        .filter_map(|strip| strip.intersection(&bounds))
        .collect()
}

fn text_bounds(item: &Item<Text>) -> Vec<Rectangle> {
    item.as_slice()
        .iter()
        .filter_map(Text::visible_bounds)
        .map(|bounds| bounds * item.transformation())
        .collect()
}

fn primitive_bounds(item: &Item<Primitive>) -> Vec<Rectangle> {
    match item {
        Item::Live(primitive) => vec![primitive.visible_bounds()],
        Item::Group(primitives, group_bounds, transformation) => primitives
            .iter()
            .map(Primitive::visible_bounds)
            .map(|bounds| bounds * *transformation)
            .filter_map(|bounds| bounds.intersection(group_bounds))
            .collect(),
        Item::Cached(_, bounds, transformation) => {
            vec![*bounds * *transformation]
        }
    }
}

fn area<'a>(damage: impl IntoIterator<Item = &'a Rectangle>) -> f32 {
    damage.into_iter().map(Rectangle::area).sum()
}

#[cfg(test)]
#[path = "scroll_blit_tests.rs"]
mod tests;
