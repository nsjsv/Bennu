//! Compute the damage between frames.
use crate::core::Rectangle;

/// Diffs the damage regions given some previous and current primitives.
pub fn diff<T>(
    previous: &[T],
    current: &[T],
    bounds: impl Fn(&T) -> Vec<Rectangle>,
    diff: impl Fn(&T, &T) -> Vec<Rectangle>,
) -> Vec<Rectangle> {
    let damage = previous.iter().zip(current).flat_map(|(a, b)| diff(a, b));

    if previous.len() == current.len() {
        damage.collect()
    } else {
        let (smaller, bigger) = if previous.len() < current.len() {
            (previous, current)
        } else {
            (current, previous)
        };

        // Extend damage by the added/removed primitives
        damage
            .chain(bigger[smaller.len()..].iter().flat_map(bounds))
            .collect()
    }
}

/// Computes the damage regions given some previous and current primitives.
pub fn list<T>(
    previous: &[T],
    current: &[T],
    bounds: impl Fn(&T) -> Vec<Rectangle>,
    are_equal: impl Fn(&T, &T) -> bool,
) -> Vec<Rectangle> {
    diff(previous, current, &bounds, |a, b| {
        if are_equal(a, b) {
            vec![]
        } else {
            bounds(a).into_iter().chain(bounds(b)).collect()
        }
    })
}

/// Groups the given damage regions that are close together inside the given
/// bounds.
///
/// Bennu 补丁：每块区域都要把所有层重走一遍（设裁剪遮罩、重填跨过它的矩形），
/// 一块区域的固定开销远大于它多出来的像素。上游只按「离原点的距离」排序后
/// 与前一块贪心合并：两列一起变（列表的图标和行尾箭头）在这个顺序里交错，
/// 永远合不上，滚动一帧碎成一百多块。现在一块区域能和任何一组合并就并进去，
/// 合并后剩下的组超过几块就合成外框。
pub fn group(damage: Vec<Rectangle>, bounds: Rectangle) -> Vec<Rectangle> {
    const AREA_THRESHOLD: f32 = 20_000.0;
    const MAX_GROUPS: usize = 4;

    let mut groups: Vec<Rectangle> = Vec::new();

    for region in damage
        .into_iter()
        .filter_map(|region| region.intersection(&bounds))
        .filter(|region| region.width >= 1.0 && region.height >= 1.0)
    {
        let mut merged = region;

        // 并完的组可能又能和别的组合并，一直吸收到没有划算的为止。
        while let Some(index) = groups.iter().position(|group| {
            merged.union(group).area() - merged.area() - group.area()
                <= AREA_THRESHOLD
        }) {
            merged = merged.union(&groups.swap_remove(index));
        }

        groups.push(merged);
    }

    if groups.len() > MAX_GROUPS {
        return groups
            .into_iter()
            .reduce(|bounding, group| bounding.union(&group))
            .into_iter()
            .collect();
    }

    groups
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

    #[test]
    fn two_columns_changing_together_stay_a_handful_of_regions() {
        let bounds = rect(0.0, 0.0, 1600.0, 1200.0);
        // 多栏视图滚动一帧：每行一个图标、一个行尾箭头。
        let damage = (0..40)
            .flat_map(|row| {
                let y = 40.0 + row as f32 * 26.0;
                [rect(236.0, y, 22.0, 22.0), rect(539.0, y, 16.0, 16.0)]
            })
            .collect();

        let groups = group(damage, bounds);

        assert!(groups.len() <= 4, "{groups:?}");
        for row in 0..40 {
            let y = 40.0 + row as f32 * 26.0;
            for changed in [rect(236.0, y, 22.0, 22.0), rect(539.0, y, 16.0, 16.0)] {
                assert!(groups.iter().any(|group| changed.is_within(group)));
            }
        }
    }

    #[test]
    fn far_apart_changes_stay_separate() {
        let bounds = rect(0.0, 0.0, 1600.0, 1200.0);
        let groups = group(
            vec![rect(10.0, 10.0, 20.0, 20.0), rect(1500.0, 1100.0, 20.0, 20.0)],
            bounds,
        );

        assert_eq!(groups.len(), 2);
    }

    #[test]
    fn regions_outside_the_bounds_are_dropped_and_clipped() {
        let bounds = rect(0.0, 0.0, 100.0, 100.0);
        let groups = group(
            vec![rect(-50.0, -50.0, 10.0, 10.0), rect(90.0, 90.0, 40.0, 40.0)],
            bounds,
        );

        assert_eq!(groups, vec![rect(90.0, 90.0, 10.0, 10.0)]);
    }
}
