use crate::core::Rectangle;

/// Bennu 补丁：矩形裁剪遮罩。
///
/// iced 的裁剪永远是矩形，一帧里要换几十上百次（每层、每组图元、每段越界的
/// 文字）。上游每次都把窗口大小的遮罩整张清零再填，这是绘制路径上最大的固定
/// 开销。这里记住上一次填的矩形：同一个矩形直接复用；换矩形时只擦掉旧矩形
/// 覆盖的像素、再按原来的方式填新矩形，遮罩内容与整张清零重填逐字节相同。
pub struct ClipMask {
    mask: tiny_skia::Mask,
    filled: Option<Rectangle>,
}

impl ClipMask {
    pub fn new(width: u32, height: u32) -> Option<Self> {
        Some(Self {
            mask: tiny_skia::Mask::new(width, height)?,
            filled: None,
        })
    }

    /// 让遮罩恰好覆盖 `bounds`（物理像素），返回可直接交给 tiny-skia 的遮罩。
    pub fn cover(&mut self, bounds: Rectangle) -> &tiny_skia::Mask {
        if self.filled != Some(bounds) {
            if let Some(previous) = self.filled.take() {
                self.erase(previous);
            }
            fill(&mut self.mask, bounds);
            self.filled = Some(bounds);
        }
        &self.mask
    }

    /// 遮罩当前覆盖的就是调用方刚 `cover` 过的矩形：绘制函数只借用、不改它。
    pub fn current(&self) -> &tiny_skia::Mask {
        &self.mask
    }

    /// 非抗锯齿填充只会写到矩形向外取整的像素范围里，把这个范围清零就擦干净了。
    fn erase(&mut self, bounds: Rectangle) {
        let width = self.mask.width() as usize;
        let height = self.mask.height() as usize;
        let clamp = |value: f32, max: usize| (value.max(0.0) as usize).min(max);
        let x0 = clamp(bounds.x.floor(), width);
        let x1 = clamp((bounds.x + bounds.width).ceil(), width);
        let y0 = clamp(bounds.y.floor(), height);
        let y1 = clamp((bounds.y + bounds.height).ceil(), height);
        let data = self.mask.data_mut();
        for row in y0..y1 {
            data[row * width + x0..row * width + x1].fill(0);
        }
    }
}

fn fill(mask: &mut tiny_skia::Mask, bounds: Rectangle) {
    let Some(rect) =
        tiny_skia::Rect::from_xywh(bounds.x, bounds.y, bounds.width, bounds.height)
    else {
        return;
    };
    mask.fill_path(
        &tiny_skia::PathBuilder::from_rect(rect),
        tiny_skia::FillRule::EvenOdd,
        false,
        tiny_skia::Transform::default(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Rectangle {
        Rectangle {
            x,
            y,
            width,
            height,
        }
    }

    /// 任意一串矩形下来，遮罩都与「整张清零再填当前矩形」逐字节相同。
    #[test]
    fn covering_one_rectangle_after_another_matches_a_fresh_fill() {
        let rectangles = [
            rect(3.25, 4.5, 40.75, 20.25),
            rect(3.25, 4.5, 40.75, 20.25),
            rect(10.6, 0.0, 5.3, 63.9),
            rect(-8.0, -3.0, 30.0, 12.0),
            rect(50.4, 50.4, 30.0, 30.0),
            rect(0.0, 0.0, 64.0, 64.0),
            rect(31.5, 31.5, 0.5, 0.5),
        ];
        let mut clip = ClipMask::new(64, 64).unwrap();

        for bounds in rectangles {
            let mut fresh = tiny_skia::Mask::new(64, 64).unwrap();
            fill(&mut fresh, bounds);

            assert_eq!(clip.cover(bounds).data(), fresh.data(), "{bounds:?}");
        }
    }
}
