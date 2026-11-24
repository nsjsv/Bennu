//! 终端颜色：前景、背景、光标取主题；16 色按主题深浅各一套（深浅背景上都看得清）；16–255 按
//! xterm 的 256 色公式；程序用 OSC 4/10/11 改过的颜色优先。画屏幕和回答颜色查询用同一套，
//! 程序查到的就是画出来的。

use alacritty_terminal::term::color::{Colors, COUNT};
use alacritty_terminal::vte::ansi::{Color as TermColor, NamedColor, Rgb};
use iced::{Color, Theme};

/// 深色背景上的 16 色：基本色不发暗，「黑」是看得见的深灰。
const DARK_ANSI: [u32; 16] = [
    0x4d4d4d, 0xf0605f, 0x5ccf86, 0xe8c14e, 0x5e9cf0, 0xc678dd, 0x4fc6d6, 0xd0d0d0, 0x7a7a7a,
    0xff7b7a, 0x7ee6a0, 0xf5d76e, 0x82b4ff, 0xdc95ef, 0x6fe0ee, 0xffffff,
];
/// 浅色背景上的 16 色：都压深到能当字色，「白」是浅灰，不至于完全看不见。
const LIGHT_ANSI: [u32; 16] = [
    0x2e2e2e, 0xc4262b, 0x1d8a3e, 0x9c6c00, 0x1f5fc4, 0x9332a8, 0x0f7f8c, 0xa8a8a8, 0x5c5c5c,
    0xe0403f, 0x2aa652, 0xb88600, 0x3a7ae0, 0xb04ec6, 0x1a9aa8, 0xd4d4d4,
];
/// 暗淡色（DECSET 的 faint、DimXxx 查询）是原色往背景混这么多。
const DIM_TOWARD_BACKGROUND: f32 = 0.34;

pub(crate) struct Palette {
    foreground: Rgb,
    background: Rgb,
    ansi: [Rgb; 16],
}

impl Palette {
    pub(crate) fn for_theme(theme: &Theme) -> Self {
        let palette = theme.palette();
        let ansi = if theme.extended_palette().is_dark {
            &DARK_ANSI
        } else {
            &LIGHT_ANSI
        };
        Self {
            foreground: rgb(palette.text),
            background: rgb(palette.background),
            ansi: ansi.map(|hex| Rgb {
                r: (hex >> 16) as u8,
                g: (hex >> 8) as u8,
                b: hex as u8,
            }),
        }
    }

    pub(crate) fn background(&self) -> Color {
        color(self.background)
    }

    pub(crate) fn color(&self, term_color: TermColor, overrides: &Colors) -> Color {
        color(match term_color {
            TermColor::Spec(spec) => spec,
            TermColor::Named(name) => self.rgb(name as usize, overrides),
            TermColor::Indexed(index) => self.rgb(usize::from(index), overrides),
        })
    }

    /// 颜色编号（0–255，或者 256 往后的前景、背景、光标、暗淡色）在这块屏幕上的实际颜色。
    pub(crate) fn rgb(&self, index: usize, overrides: &Colors) -> Rgb {
        // `Colors` 越界会 panic，程序问的编号不一定在范围里。
        if let Some(chosen) = (index < COUNT).then(|| overrides[index]).flatten() {
            return chosen;
        }
        const FOREGROUND: usize = NamedColor::Foreground as usize;
        const BACKGROUND: usize = NamedColor::Background as usize;
        const DIM_BLACK: usize = NamedColor::DimBlack as usize;
        const DIM_WHITE: usize = NamedColor::DimWhite as usize;
        const DIM_FOREGROUND: usize = NamedColor::DimForeground as usize;
        match index {
            0..=15 => self.ansi[index],
            16..=231 => cube(index - 16),
            232..=255 => {
                let level = 8 + (index - 232) as u8 * 10;
                Rgb {
                    r: level,
                    g: level,
                    b: level,
                }
            }
            BACKGROUND => self.background,
            DIM_BLACK..=DIM_WHITE => self.dim(self.rgb(index - DIM_BLACK, overrides)),
            DIM_FOREGROUND => self.dim(self.rgb(FOREGROUND, overrides)),
            // 前景、光标、亮前景，和程序乱问的编号。
            _ => self.foreground,
        }
    }

    fn dim(&self, original: Rgb) -> Rgb {
        let mix = |from: u8, to: u8| {
            (f32::from(from) + (f32::from(to) - f32::from(from)) * DIM_TOWARD_BACKGROUND).round()
                as u8
        };
        Rgb {
            r: mix(original.r, self.background.r),
            g: mix(original.g, self.background.g),
            b: mix(original.b, self.background.b),
        }
    }
}

/// 程序问颜色（OSC 4/10/11/12 `?`）时的回答。
pub(crate) fn color_for_index(index: usize, overrides: &Colors, theme: &Theme) -> Rgb {
    Palette::for_theme(theme).rgb(index, overrides)
}

fn cube(index: usize) -> Rgb {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    Rgb {
        r: LEVELS[index / 36],
        g: LEVELS[(index % 36) / 6],
        b: LEVELS[index % 6],
    }
}

fn rgb(color: Color) -> Rgb {
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    Rgb {
        r: channel(color.r),
        g: channel(color.g),
        b: channel(color.b),
    }
}

fn color(rgb: Rgb) -> Color {
    Color::from_rgb8(rgb.r, rgb.g, rgb.b)
}
