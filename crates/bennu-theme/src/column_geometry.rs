//! 多栏（列浏览器）条目几何的唯一共享实现：主软件（app-ui）与 portal
//! 多栏视图共用同一份常量与按比例缩放的几何结构，禁止任何一侧复制
//! 副本。缩放输入是原生 `scale: f32`（app-ui 的密度档位在各自侧换算），
//! 依赖调用方私有类型的换算（图标密度、虚拟视口模型）留在各自 crate。

use iced::Padding;

/// 目录行尾 chevron 图标边长（基准档）。
pub const COLUMN_CHEVRON_ICON_SIZE: f32 = 11.0;
/// 行与行之间的容器 spacing（基准档）。
pub const COLUMN_CONTENT_SPACING: u32 = 2;
/// 栏内容外层 padding [纵向, 横向]。
pub const COLUMN_PADDING: [u16; 2] = [5, 5];
/// 条目文字字号（基准档）。
pub const COLUMN_ENTRY_TEXT_SIZE: u32 = 13;
/// 条目行高（基准档）。
pub const COLUMN_ENTRY_HEIGHT: f32 = 24.0;
/// 条目内部图标与文字的 spacing（基准档）。
pub const COLUMN_ENTRY_SPACING: u32 = 4;
/// 条目内边距 [纵向, 横向]（基准档）。
pub const COLUMN_ENTRY_PADDING: [u16; 2] = [1, 4];

/// 多栏条目几何按 scale 缩放；列宽、列间距和面板外层留白固定。
/// `entries_top_padding` 表达 padding 加 top spacer 后首个行间距的真实
/// 首行起点（app-ui 虚拟 spacer 布局的换算基准）。
#[derive(Debug, Clone, Copy)]
pub struct ColumnEntryGeometry {
    pub entry_height: f32,
    pub entry_scroll_height: f32,
    pub entries_top_padding: f32,
    pub content_spacing: f32,
    pub text_size: f32,
    pub chevron_icon_size: f32,
    pub entry_spacing: f32,
    pub entry_padding: Padding,
}

impl ColumnEntryGeometry {
    /// 缩放尺寸一律取整到整数像素：行槽高等整数几何让整行对齐的
    /// viewport 运算保持精确，虚拟范围、键盘揭示与缩略图调度不会各
    /// 舍入到不同行。
    pub fn for_scale(scale: f32) -> Self {
        let scaled = |base: f32| (base * scale).round();
        let entry_height = scaled(COLUMN_ENTRY_HEIGHT);
        let content_spacing = scaled(COLUMN_CONTENT_SPACING as f32);
        let entry_padding_vertical = scaled(COLUMN_ENTRY_PADDING[0] as f32);
        let entry_padding_horizontal = scaled(COLUMN_ENTRY_PADDING[1] as f32);
        Self {
            entry_height,
            entry_scroll_height: entry_height + content_spacing,
            entries_top_padding: COLUMN_PADDING[0] as f32 + content_spacing,
            content_spacing,
            text_size: scaled(COLUMN_ENTRY_TEXT_SIZE as f32),
            chevron_icon_size: scaled(COLUMN_CHEVRON_ICON_SIZE),
            entry_spacing: scaled(COLUMN_ENTRY_SPACING as f32),
            entry_padding: Padding {
                top: entry_padding_vertical,
                right: entry_padding_horizontal,
                bottom: entry_padding_vertical,
                left: entry_padding_horizontal,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_scale_keeps_base_geometry() {
        let geometry = ColumnEntryGeometry::for_scale(1.0);
        assert_eq!(geometry.entry_height, COLUMN_ENTRY_HEIGHT);
        assert_eq!(
            geometry.entry_scroll_height,
            COLUMN_ENTRY_HEIGHT + COLUMN_CONTENT_SPACING as f32
        );
        assert_eq!(
            geometry.entries_top_padding,
            COLUMN_PADDING[0] as f32 + COLUMN_CONTENT_SPACING as f32
        );
        assert_eq!(geometry.content_spacing, COLUMN_CONTENT_SPACING as f32);
        assert_eq!(geometry.text_size, COLUMN_ENTRY_TEXT_SIZE as f32);
        assert_eq!(geometry.chevron_icon_size, COLUMN_CHEVRON_ICON_SIZE);
        assert_eq!(geometry.entry_spacing, COLUMN_ENTRY_SPACING as f32);
        assert_eq!(
            geometry.entry_padding.right,
            f32::from(COLUMN_ENTRY_PADDING[1])
        );
    }

    #[test]
    fn scaled_dimensions_round_to_whole_pixels_and_padding_stays_fixed() {
        // 任意 scale 下尺寸四舍五入到整数像素；面板 padding 固定，只有
        // 行间距部分缩放。
        let scale = 1.35;
        let geometry = ColumnEntryGeometry::for_scale(scale);
        assert_eq!(geometry.entry_height, (COLUMN_ENTRY_HEIGHT * scale).round());
        assert_eq!(
            geometry.content_spacing,
            (COLUMN_CONTENT_SPACING as f32 * scale).round()
        );
        assert_eq!(
            geometry.entries_top_padding,
            COLUMN_PADDING[0] as f32 + (COLUMN_CONTENT_SPACING as f32 * scale).round()
        );
        assert_eq!(
            geometry.text_size,
            (COLUMN_ENTRY_TEXT_SIZE as f32 * scale).round()
        );
        assert_eq!(
            geometry.chevron_icon_size,
            (COLUMN_CHEVRON_ICON_SIZE * scale).round()
        );
        assert_eq!(
            geometry.entry_padding.right,
            (COLUMN_ENTRY_PADDING[1] as f32 * scale).round()
        );
    }
}
