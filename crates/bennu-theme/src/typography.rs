//! 界面字体与字号档的唯一来源。
//!
//! 字号只按“这段文字是什么”取档；正文是应用默认字号，调用方不写 `.size()`。
//! 终端格子、文本预览正文这类内容字号不属于这里。

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use iced::font::Weight;
use iced::Font;

/// 页面、窗口标题，配 [`BOLD`]。
pub const TITLE_SIZE: f32 = 18.0;
/// 组标题，配 [`BOLD`]。
pub const HEADING_SIZE: f32 = 13.0;
/// 正文：iced `default_text_size`，所以正文不写 `.size()`。
pub const BODY_SIZE: f32 = 13.0;
/// 说明文字，一般配灰色。
pub const CAPTION_SIZE: f32 = 12.0;
/// 角标、计数。
pub const SMALL_SIZE: f32 = 11.0;
/// 行高系数：与 iced `LineHeight::default()` 一致，几何计算按它推行高。
pub const LINE_HEIGHT: f32 = 1.3;

pub const BOLD: Font = Font {
    weight: Weight::Bold,
    ..Font::DEFAULT
};

/// fontconfig 首次运行可能要重建缓存；超过这个时间就放弃，界面继续用 cosmic-text 默认字体。
const FONTCONFIG_QUERY_TIMEOUT: Duration = Duration::from_millis(300);
const FONTCONFIG_POLL_INTERVAL: Duration = Duration::from_millis(2);

/// 让 `Font::DEFAULT` / `Font::MONOSPACE` 落到系统设置的字体上；必须在 iced 首次排版前调用。
///
/// cosmic-text 0.15 把 sans-serif 写死成 "Open Sans"、monospace 写死成 "Noto Sans Mono"，
/// 系统没装时逐字回退，中英文就会落到不同字体。这里改的是家族别名本身，所以所有
/// `Font::DEFAULT`、`..Font::DEFAULT` 构造的粗体、`Font::MONOSPACE` 调用点都跟着变。
/// 没选 `default_font(Font::with_name(..))`：等宽和粗体仍会落回写死的家族。
pub fn adopt_system_font_families() {
    // 两个查询并行跑，启动只等较慢的那个。
    let sans = spawn_fontconfig_query("sans-serif");
    let monospace = spawn_fontconfig_query("monospace");
    let sans = sans.and_then(finish_fontconfig_query);
    let monospace = monospace.and_then(finish_fontconfig_query);

    let mut font_system = iced::advanced::graphics::text::font_system()
        .write()
        .expect("font system lock poisoned");
    let database = font_system.raw().db_mut();
    if let Some(family) = sans {
        database.set_sans_serif_family(family);
    }
    if let Some(family) = monospace {
        database.set_monospace_family(family);
    }
}

fn spawn_fontconfig_query(pattern: &str) -> Option<Child> {
    Command::new("fc-match")
        .args(["-f", "%{family[0]}", pattern])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .inspect_err(|error| tracing::warn!(%error, pattern, "fc-match could not start"))
        .ok()
}

fn finish_fontconfig_query(mut child: Child) -> Option<String> {
    let deadline = Instant::now() + FONTCONFIG_QUERY_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(FONTCONFIG_POLL_INTERVAL),
            Ok(None) | Err(_) => {
                tracing::warn!("fc-match did not finish in time; keeping default fonts");
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let output = child.wait_with_output().ok()?;
    parse_fontconfig_family(&output.stdout)
}

fn parse_fontconfig_family(stdout: &[u8]) -> Option<String> {
    let family = String::from_utf8_lossy(stdout)
        .lines()
        .next()?
        .trim()
        .to_owned();
    (!family.is_empty()).then_some(family)
}

#[cfg(test)]
mod tests {
    use super::parse_fontconfig_family;

    #[test]
    fn takes_the_first_family_line() {
        assert_eq!(
            parse_fontconfig_family(b"Noto Sans\n"),
            Some("Noto Sans".to_owned())
        );
        assert_eq!(
            parse_fontconfig_family(b"  Noto Sans CJK SC \nignored"),
            Some("Noto Sans CJK SC".to_owned())
        );
    }

    #[test]
    fn rejects_empty_output() {
        assert_eq!(parse_fontconfig_family(b""), None);
        assert_eq!(parse_fontconfig_family(b"   \n"), None);
    }
}
