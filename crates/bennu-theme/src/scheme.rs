//! 配色的唯一层次计算：每套配色只给底色、文字色、强调色三个颜色，其余 31 个角色
//! （侧栏、卡片、悬停、选中、分隔线、次要文字……）全部在这里按比例算出来。
//!
//! 层次靠“把文字色按比例混进底色”得到，结果是实色，叠在哪个容器上都一样。混色在 OKLab 里做：在 sRGB 里混，深色主题的
//! 层次会挤在一起、浅色主题又拉得太开；OKLab 里等比例混出来的台阶看起来一样大。

use iced::Color;

use super::{AppearanceMode, UiColorRoles};

/// 一套配色在一种深浅下的全部输入。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SchemeColors {
    pub background: Color,
    pub text: Color,
    pub accent: Color,
}

/// 各层离底色多远：除 `recessed` 外都是“文字色混进底色”的比例（0 = 底色，1 = 文字色），
/// 浅色主题往深处走、深色往亮处走，方向由文字色决定，一张表对所有配色都成立。
struct Layers {
    /// 侧栏：比内容区更“陷下去”，两种深浅都往黑走（深色下侧栏比内容暗）。
    recessed: f32,
    /// 菜单、补全面板、列表斑马纹这类浅浅一层的面。
    low: f32,
    card: f32,
    hover: f32,
    pressed: f32,
    /// 深色下浮起来的面（拖动缩略图、徽标底）。浅色下浮起来就是底色本身。
    elevated: f32,
    divider: f32,
    border: f32,
    muted_text: f32,
    /// 强调色混进底色的比例：选中的文件行。
    selection: f32,
    status_container: f32,
}

// 默认配色（白底 #0d0d0d 字）下：侧栏 #f9f9f9、卡片 #f4f4f4、悬停 #ececec、按下 #e3e3e3、
// 分隔线 #e5e5e5、次要文字 #5d5d5d。
const LIGHT_LAYERS: Layers = Layers {
    recessed: 0.02,
    low: 0.03,
    card: 0.04,
    hover: 0.07,
    pressed: 0.1,
    elevated: 0.0,
    divider: 0.09,
    border: 0.22,
    muted_text: 0.62,
    selection: 0.18,
    status_container: 0.14,
};

// 默认配色（#212121 底 #ececec 字）下：侧栏 #171717、卡片 #2f2f2f、悬停 #353535、
// 按下 #3d3d3d、分隔线 #383838、次要文字 #b4b4b4。
const DARK_LAYERS: Layers = Layers {
    recessed: 0.17,
    low: 0.05,
    card: 0.085,
    hover: 0.12,
    pressed: 0.16,
    elevated: 0.12,
    divider: 0.135,
    border: 0.33,
    muted_text: 0.75,
    selection: 0.24,
    status_container: 0.22,
};

/// 成功 / 警告 / 错误：只取三色的约定下不跟配色变，每种深浅一套。
struct StatusColors {
    success: Color,
    warning: Color,
    error: Color,
}

const LIGHT_STATUS: StatusColors = StatusColors {
    success: rgb(0x1b8553),
    warning: rgb(0x9c6e03),
    error: rgb(0xc01c28),
};

const DARK_STATUS: StatusColors = StatusColors {
    success: rgb(0x78e9ab),
    warning: rgb(0xf8e45c),
    error: rgb(0xff7b63),
};

/// 强调色要当图标、链接、选中边框用，对底色至少 3:1（WCAG 非文字对比）。
const MIN_ACCENT_CONTRAST: f32 = 3.0;
/// 实色上的白字（按钮文字、开关圆点）：按 WCAG 大字 / 界面元素的 3:1。
const MIN_LABEL_CONTRAST: f32 = 3.0;
/// 正文 4.5:1（WCAG AA）。
const MIN_TEXT_CONTRAST: f32 = 4.5;
/// 次要文字（说明、占位符）在它可能出现的每一层上至少 3:1。
const MIN_MUTED_TEXT_CONTRAST: f32 = 3.0;

impl UiColorRoles {
    pub(super) fn derive(mode: AppearanceMode, colors: SchemeColors) -> Self {
        let (layers, status) = match mode {
            AppearanceMode::Light => (&LIGHT_LAYERS, &LIGHT_STATUS),
            AppearanceMode::Dark => (&DARK_LAYERS, &DARK_STATUS),
        };
        let SchemeColors {
            background,
            text: scheme_text,
            accent,
        } = colors;
        // 所有面都按配色自己的文字色混出来，层次比例对每套配色一样。
        let layer = |amount| mix(background, scheme_text, amount);
        let sidebar = mix(background, Color::BLACK, layers.recessed);
        let low = layer(layers.low);
        let card = layer(layers.card);
        let hover = layer(layers.hover);
        let pressed = layer(layers.pressed);
        let primary = readable_on(accent, scheme_text, &[background], MIN_ACCENT_CONTRAST);
        let primary_container = mix(background, primary, layers.selection);
        let tinted = |color| mix(background, color, layers.status_container);
        let (success_container, warning_container, error_container) = (
            tinted(status.success),
            tinted(status.warning),
            tinted(status.error),
        );

        // 配色自己的文字色在某一层上不够 4.5:1 时（Tokyo Night 浅色的蓝字、Everforest
        // 浅色的灰字），只把那里的字往纯黑 / 纯白推一点，面不动。
        let text_on = |surfaces: &[Color]| {
            readable_on(scheme_text, extreme_text(mode), surfaces, MIN_TEXT_CONTRAST)
        };
        let text = text_on(&[background, sidebar, low, card, hover, pressed]);
        // 次要文字会出现在悬停行、侧栏选中行和选中的文件行上，按其中最难读的那层定深浅。
        let muted_text = readable_on(
            layer(layers.muted_text),
            text,
            &[hover, pressed, primary_container],
            MIN_MUTED_TEXT_CONTRAST,
        );

        Self {
            mode,
            background,
            on_background: text,
            surface: background,
            surface_dim: sidebar,
            surface_bright: layer(layers.elevated),
            surface_container_lowest: background,
            surface_container_low: low,
            surface_container: card,
            surface_container_high: hover,
            surface_container_highest: pressed,
            on_surface: text,
            on_surface_variant: muted_text,
            outline: layer(layers.border),
            outline_variant: layer(layers.divider),
            primary,
            on_primary: label_on(primary),
            primary_container,
            on_primary_container: text_on(&[primary_container]),
            secondary: status.success,
            on_secondary: label_on(status.success),
            secondary_container: success_container,
            on_secondary_container: text_on(&[success_container]),
            tertiary: status.warning,
            on_tertiary: label_on(status.warning),
            tertiary_container: warning_container,
            on_tertiary_container: text_on(&[warning_container]),
            error: status.error,
            on_error: label_on(status.error),
            error_container,
            on_error_container: text_on(&[error_container]),
        }
    }
}

/// 把 `color` 一点点往 `toward` 靠（保持色相），直到在每个 `surfaces` 上都有 `min_contrast`。
/// 强调色对底色不够 3:1 时（例如 Everforest 浅色的绿）也靠它往文字色调。
fn readable_on(color: Color, toward: Color, surfaces: &[Color], min_contrast: f32) -> Color {
    let readable = |candidate| {
        surfaces
            .iter()
            .all(|surface| contrast_ratio(candidate, *surface) >= min_contrast)
    };
    let mut adjusted = color;
    let mut amount = 0.0;
    while !readable(adjusted) && amount < 1.0 {
        amount += 0.05;
        adjusted = mix(color, toward, amount);
    }
    adjusted
}

/// 文字再往外推就只剩纯黑（浅色）或纯白（深色）。
fn extreme_text(mode: AppearanceMode) -> Color {
    match mode {
        AppearanceMode::Light => Color::BLACK,
        AppearanceMode::Dark => Color::WHITE,
    }
}

/// 放在实色（强调色按钮、开关圆点、状态色徽标）上的字：优先白色，饱和的强调色上白字
/// 最干净；白色不到 3:1 时（浅黄、浅绿、默认配色深色下的白色强调）才用黑色。
fn label_on(fill: Color) -> Color {
    if contrast_ratio(Color::WHITE, fill) >= MIN_LABEL_CONTRAST {
        Color::WHITE
    } else {
        Color::BLACK
    }
}

pub(super) const fn rgb(value: u32) -> Color {
    Color::from_rgb8((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

/// OKLab 里从 `from` 走向 `to` 的 `amount`（0..=1）。
pub(super) fn mix(from: Color, to: Color, amount: f32) -> Color {
    let from = Oklab::from_color(from);
    let to = Oklab::from_color(to);
    Oklab {
        l: from.l + (to.l - from.l) * amount,
        a: from.a + (to.a - from.a) * amount,
        b: from.b + (to.b - from.b) * amount,
    }
    .to_color()
}

/// WCAG 2 对比度（1..=21）。
pub fn contrast_ratio(first: Color, second: Color) -> f32 {
    let (lighter, darker) = {
        let first = relative_luminance(first);
        let second = relative_luminance(second);
        (first.max(second), first.min(second))
    };
    (lighter + 0.05) / (darker + 0.05)
}

fn relative_luminance(color: Color) -> f32 {
    0.2126 * to_linear(color.r) + 0.7152 * to_linear(color.g) + 0.0722 * to_linear(color.b)
}

fn to_linear(channel: f32) -> f32 {
    if channel <= 0.04045 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

fn from_linear(channel: f32) -> f32 {
    let channel = channel.clamp(0.0, 1.0);
    if channel <= 0.003_130_8 {
        channel * 12.92
    } else {
        1.055 * channel.powf(1.0 / 2.4) - 0.055
    }
}

/// OKLab 色彩空间：L 是感知亮度，a / b 是两个色度轴。
struct Oklab {
    l: f32,
    a: f32,
    b: f32,
}

impl Oklab {
    fn from_color(color: Color) -> Self {
        let (r, g, b) = (to_linear(color.r), to_linear(color.g), to_linear(color.b));
        let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
        let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
        let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
        Self {
            l: 0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
            a: 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
            b: 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
        }
    }

    fn to_color(&self) -> Color {
        let l = (self.l + 0.396_337_78 * self.a + 0.215_803_76 * self.b).powi(3);
        let m = (self.l - 0.105_561_346 * self.a - 0.063_854_17 * self.b).powi(3);
        let s = (self.l - 0.089_484_18 * self.a - 1.291_485_5 * self.b).powi(3);
        Color::from_rgb(
            from_linear(4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s),
            from_linear(-1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s),
            from_linear(-0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(first: Color, second: Color) -> bool {
        [
            (first.r, second.r),
            (first.g, second.g),
            (first.b, second.b),
        ]
        .iter()
        .all(|(x, y)| (x - y).abs() < 0.002)
    }

    #[test]
    fn mixing_keeps_the_endpoints_and_round_trips_through_oklab() {
        let from = rgb(0x1e1e2e);
        let to = rgb(0xcdd6f4);
        assert!(close(mix(from, to, 0.0), from));
        assert!(close(mix(from, to, 1.0), to));
    }

    #[test]
    fn contrast_matches_the_wcag_reference_values() {
        assert!((contrast_ratio(Color::WHITE, Color::BLACK) - 21.0).abs() < 0.01);
        assert!((contrast_ratio(rgb(0x777777), Color::WHITE) - 4.48).abs() < 0.02);
    }

    #[test]
    fn a_faint_accent_is_pulled_toward_the_text_until_it_reads_on_the_background() {
        let background = rgb(0xfdf6e3);
        let faint = rgb(0xd5e0a0);
        let text = rgb(0x5c6a72);
        let readable = readable_on(faint, text, &[background], MIN_ACCENT_CONTRAST);
        assert!(contrast_ratio(readable, background) >= MIN_ACCENT_CONTRAST);
        let already_fine = rgb(0x3584e4);
        assert_eq!(
            readable_on(already_fine, text, &[rgb(0xfafafb)], MIN_ACCENT_CONTRAST),
            already_fine
        );
    }
}
