//! 按面积平均把一行行送来的像素缩小：每个目标像素取落在它里面的源像素的平均，颜色按不透明度
//! 加权（透明的边不会把颜色带黑）。只留当前目标行的累加和。

use image::RgbaImage;

pub(super) struct AreaShrink {
    source: (u32, u32),
    target: RgbaImage,
    /// 源图第 x 列落在目标的哪一列。
    target_column: Vec<u32>,
    /// 当前目标行每列的 Σr·a、Σg·a、Σb·a、Σa 与源像素个数。
    sums: Vec<[u64; 5]>,
    next_source_row: u32,
    target_row: u32,
}

impl AreaShrink {
    /// `target` 每边都不大于 `source`。
    pub(super) fn new(source: (u32, u32), target: (u32, u32)) -> Self {
        let target_column = (0..source.0)
            .map(|x| (u64::from(x) * u64::from(target.0) / u64::from(source.0)) as u32)
            .collect();
        Self {
            source,
            target: RgbaImage::new(target.0, target.1),
            target_column,
            sums: vec![[0; 5]; target.0 as usize],
            next_source_row: 0,
            target_row: 0,
        }
    }

    /// 送入下一行（RGBA，宽度是源图宽度）。
    pub(super) fn push_row(&mut self, rgba: &[u8]) {
        let row = (u64::from(self.next_source_row) * u64::from(self.target.height())
            / u64::from(self.source.1)) as u32;
        if row != self.target_row {
            self.flush();
            self.target_row = row;
        }
        for (&[red, green, blue, alpha], &column) in
            rgba.as_chunks::<4>().0.iter().zip(&self.target_column)
        {
            let alpha_weight = u64::from(alpha);
            let sum = &mut self.sums[column as usize];
            sum[0] += u64::from(red) * alpha_weight;
            sum[1] += u64::from(green) * alpha_weight;
            sum[2] += u64::from(blue) * alpha_weight;
            sum[3] += alpha_weight;
            sum[4] += 1;
        }
        self.next_source_row += 1;
    }

    pub(super) fn finish(mut self) -> RgbaImage {
        self.flush();
        self.target
    }

    fn flush(&mut self) {
        let row = self.target_row;
        for (column, sum) in self.sums.iter_mut().enumerate() {
            let [red, green, blue, alpha, count] = std::mem::take(sum);
            if count == 0 {
                continue;
            }
            let color = |channel: u64| channel.checked_div(alpha).map_or(0, |value| value as u8);
            let pixel = [
                color(red + alpha / 2),
                color(green + alpha / 2),
                color(blue + alpha / 2),
                ((alpha + count / 2) / count) as u8,
            ];
            self.target
                .put_pixel(column as u32, row, image::Rgba(pixel));
        }
    }
}
