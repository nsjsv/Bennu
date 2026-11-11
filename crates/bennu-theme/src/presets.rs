//! 每套配色每种深浅只有底色、文字色、强调色三个值，其余层次都由 `scheme::derive` 算。

use super::scheme::{rgb, SchemeColors};
use super::{AppearanceMode, ColorSchemePreset};

pub(super) fn colors(preset: ColorSchemePreset, mode: AppearanceMode) -> SchemeColors {
    use AppearanceMode::{Dark, Light};
    use ColorSchemePreset as Preset;
    let (background, text, accent) = match (preset, mode) {
        // 默认：纯白 / 中性灰，强调色就是文字色——开关、选中、按钮都是黑白灰，不带任何
        // 色调，也不跟系统强调色。Matugen 还没生成文件时和它一样。
        (Preset::Default | Preset::Matugen, Light) => (0xffffff, 0x0d0d0d, 0x0d0d0d),
        (Preset::Default | Preset::Matugen, Dark) => (0x212121, 0xececec, 0xececec),
        (Preset::Claude, Light) => (0xfaf9f5, 0x3d3929, 0xc96442),
        (Preset::Claude, Dark) => (0x262624, 0xc3c0b6, 0xd97757),
        // Latte / Mocha
        (Preset::Catppuccin, Light) => (0xeff1f5, 0x4c4f69, 0x8839ef),
        (Preset::Catppuccin, Dark) => (0x1e1e2e, 0xcdd6f4, 0xcba6f7),
        // Medium
        (Preset::Everforest, Light) => (0xfdf6e3, 0x5c6a72, 0x8da101),
        (Preset::Everforest, Dark) => (0x2d353b, 0xd3c6aa, 0xa7c080),
        // Medium
        (Preset::Gruvbox, Light) => (0xfbf1c7, 0x3c3836, 0x076678),
        (Preset::Gruvbox, Dark) => (0x282828, 0xebdbb2, 0x83a598),
        (Preset::Nord, Light) => (0xeceff4, 0x2e3440, 0x5e81ac),
        (Preset::Nord, Dark) => (0x2e3440, 0xeceff4, 0x88c0d0),
        // Day / Night。Day 自带的文字色是中蓝，整个文件列表都会变蓝；文字改用藏青。
        (Preset::TokyoNight, Light) => (0xe1e2e7, 0x343b58, 0x2e7de9),
        (Preset::TokyoNight, Dark) => (0x1a1b26, 0xc0caf5, 0x7aa2f7),
    };
    SchemeColors {
        background: rgb(background),
        text: rgb(text),
        accent: rgb(accent),
    }
}
