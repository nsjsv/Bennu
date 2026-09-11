use std::path::Path;
use std::sync::{Arc, OnceLock};

use desktop_linux::WaylandFileDragIcon;
use resvg::tiny_skia::Pixmap;
use resvg::usvg::{fontdb, Options, Tree};

use crate::icons::IconSymbol;

// Wayland 协议把拖拽图标位图的左上角钉在光标上;画布因此以光标为
// 原点向右下展开,最大边长压在协议位图 256 上限。组里条目若相对
// 光标在左上,整体平移使最靠左上的胶囊贴住原点(抓取点随之偏移,
// 这是协议下的最优还原)。
const DRAG_ICON_CANVAS_MAX_EDGE: u32 = 256;
// 淡出几何与窗口内预览(view.rs)保持同一观感。
const DRAG_ICON_FADE_RADIUS: f32 = 256.0;
const DRAG_ICON_FADE_SOLID_DISTANCE: f32 = 96.0;
const DRAG_ICON_MAX_PILLS: usize = 128;

// 胶囊几何:图标 24 + 文件名 12,高 24,与窗口内预览一致;无底板。
const PILL_HEIGHT: f32 = 24.0;
const PILL_ICON_SIZE: f32 = 24.0;
const PILL_ICON_LEFT: f32 = 0.0;
const PILL_ICON_TOP: f32 = 0.0;
const PILL_LABEL_LEFT: f32 = PILL_ICON_LEFT + PILL_ICON_SIZE + 6.0;
const PILL_LABEL_SIZE: f32 = 12.0;
const PILL_LABEL_BASELINE: f32 = 17.0;
const PILL_LABEL_MAX_WIDTH: f32 = 150.0;
const PILL_PADDING_RIGHT: f32 = 4.0;

/// 拖出软件的位图中的一个条目胶囊。
#[derive(Debug, Clone)]
pub(crate) struct FileDragIconEntry {
    pub(crate) symbol: IconSymbol,
    pub(crate) label: String,
    /// 相对按下点(提起瞬间的光标位置)的条目原点偏移。
    pub(crate) offset: iced::Vector,
}

/// 拖拽图标配色:取自当前主题,与窗口内 drag_preview_panel 同源。
#[derive(Debug, Clone, Copy)]
pub(crate) struct FileDragPillPalette {
    pub(crate) content: iced::Color,
}

/// 系统级拖拽图像必须一次性生成位图:按提起瞬间的相对位置摆开各
/// 选中条目的"图标 + 文件名"胶囊,离光标越远越淡,与窗口内预览
/// 保持同一几何与观感。
pub(crate) fn render_wayland_file_drag_icon(
    entries: &[FileDragIconEntry],
    palette: FileDragPillPalette,
) -> Result<WaylandFileDragIcon, String> {
    let entries = &entries[..entries.len().min(DRAG_ICON_MAX_PILLS)];
    let shift_x = -entries
        .iter()
        .map(|entry| entry.offset.x)
        .fold(0.0_f32, f32::min)
        .min(0.0);
    let shift_y = -entries
        .iter()
        .map(|entry| entry.offset.y)
        .fold(0.0_f32, f32::min)
        .min(0.0);

    let mut pills = Vec::with_capacity(entries.len());
    let mut canvas_width = 1.0_f32;
    let mut canvas_height = 1.0_f32;
    for entry in entries {
        let position = entry.offset + iced::Vector::new(shift_x, shift_y);
        let fade = drag_icon_fade(position);
        let Some(fade) = fade else {
            continue;
        };
        let label_width = fitted_label_width(&entry.label)?;
        let pill_width = PILL_LABEL_LEFT + label_width + PILL_PADDING_RIGHT;
        canvas_width = canvas_width.max(position.x + pill_width);
        canvas_height = canvas_height.max(position.y + PILL_HEIGHT);
        pills.push((position, pill_width, label_width, fade, entry));
    }
    let canvas_width = canvas_width
        .ceil()
        .min(DRAG_ICON_CANVAS_MAX_EDGE as f32) as u32;
    let canvas_height = canvas_height
        .ceil()
        .min(DRAG_ICON_CANVAS_MAX_EDGE as f32) as u32;

    let svg = pill_group_svg(canvas_width, canvas_height, &pills, palette)?;
    let pixmap = render_svg(&svg, canvas_width, canvas_height)?;
    WaylandFileDragIcon::new(canvas_width, canvas_height, pixmap.take())
        .map_err(|error| error.to_string())
}

/// 离光标(画布原点)越远越淡:核心距离内全浓,超出淡出半径不显示。
fn drag_icon_fade(position: iced::Vector) -> Option<f32> {
    let distance = (position.x * position.x + position.y * position.y).sqrt();
    let fade = (DRAG_ICON_FADE_RADIUS - distance)
        / (DRAG_ICON_FADE_RADIUS - DRAG_ICON_FADE_SOLID_DISTANCE);
    (fade > 0.0).then(|| fade.clamp(0.0, 1.0))
}

/// 整个拖拽图标一次矢量渲染:胶囊底板、嵌套文件类型图标与文件名
/// 都画进同一张 SVG,透明度随距离衰减。
fn pill_group_svg(
    canvas_width: u32,
    canvas_height: u32,
    pills: &[(iced::Vector, f32, f32, f32, &FileDragIconEntry)],
    palette: FileDragPillPalette,
) -> Result<String, String> {
    let content = hex_color(palette.content);
    let mut groups = String::new();
    for (position, _, label_width, fade, entry) in pills {
        let icon_x = position.x + PILL_ICON_LEFT;
        let icon_y = position.y + PILL_ICON_TOP;
        let label_x = position.x + PILL_LABEL_LEFT;
        let label = truncate_label_to_width(&entry.label, *label_width)?;
        groups.push_str(&format!(
            "<g opacity=\"{fade:.3}\">\
{nested_icon}\
<text x=\"{label_x}\" y=\"{label_y}\" font-family=\"sans-serif\" \
font-size=\"{PILL_LABEL_SIZE}\" fill=\"{content}\">{escaped_label}</text>\
</g>",
            label_y = position.y + PILL_LABEL_BASELINE,
            nested_icon = nested_icon_svg(entry.symbol, icon_x, icon_y, &content),
            escaped_label = escape_xml_text(&label),
        ));
    }
    Ok(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{canvas_width}\" \
height=\"{canvas_height}\">{groups}</svg>"
    ))
}

/// 嵌入条目图标:沿用原 SVG 的 viewBox 与形状定义,只注入画布内
/// 位置(图标源文件自带 24x24 尺寸)并把 currentColor 换成主题前景
/// 色,实心/线框两种风格都兼容。
fn nested_icon_svg(symbol: IconSymbol, x: f32, y: f32, content_hex: &str) -> String {
    let raw = String::from_utf8_lossy(symbol.bytes())
        .replace("currentColor", content_hex);
    raw.replacen("<svg ", &format!("<svg x=\"{x}\" y=\"{y}\" "), 1)
}

fn truncate_label_to_width(label: &str, width: f32) -> Result<String, String> {
    if measure_text_width(label)? <= width {
        return Ok(label.to_owned());
    }
    let mut candidate: String = label.chars().take(3).collect();
    for character in label.chars().skip(3) {
        let next = format!("{candidate}{character}");
        if measure_text_width(&next)? > width {
            break;
        }
        candidate = next;
    }
    Ok(candidate)
}

fn fitted_label_width(label: &str) -> Result<f32, String> {
    let mut width = measure_text_width(label)?;
    if width > PILL_LABEL_MAX_WIDTH {
        // 宽度上限由截断循环收紧,这里先按上限占位。
        width = PILL_LABEL_MAX_WIDTH;
    }
    Ok(width.ceil())
}

fn render_svg(svg: &str, width: u32, height: u32) -> Result<Pixmap, String> {
    let mut options = Options::default();
    options.fontdb = font_database();
    options.font_family = "sans-serif".to_owned();
    let tree = Tree::from_data(svg.as_bytes(), &options)
        .map_err(|error| format!("could not parse drag icon SVG: {error}"))?;
    let mut pixmap = Pixmap::new(width, height)
        .ok_or_else(|| "could not allocate drag icon pixmap".to_owned())?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::identity(),
        &mut pixmap.as_mut(),
    );
    Ok(pixmap)
}

/// 数字宽度按实测像素给出,保证角标/文件名能完整放进胶囊。
fn measure_text_width(text: &str) -> Result<f32, String> {
    const MEASURE_CANVAS_WIDTH: u32 = 512;
    const MEASURE_CANVAS_HEIGHT: u32 = 24;
    let content = hex_color(iced::Color::BLACK);
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{MEASURE_CANVAS_WIDTH}\" \
height=\"{MEASURE_CANVAS_HEIGHT}\">\
<text x=\"0\" y=\"{PILL_LABEL_BASELINE}\" font-family=\"sans-serif\" \
font-size=\"{PILL_LABEL_SIZE}\" fill=\"{content}\">{escaped}</text></svg>",
        escaped = escape_xml_text(text),
    );
    let pixmap = render_svg(&svg, MEASURE_CANVAS_WIDTH, MEASURE_CANVAS_HEIGHT)?;
    let (_, _, max_x, _) = opaque_bounding_box(&pixmap);
    Ok(max_x as f32)
}

/// 文件名显示名:与窗口内预览同一截断规则。
pub(crate) fn file_drag_display_name(path: &Path) -> String {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("?");
    crate::formatting::format_middle_ellipsized_text(name, 20)
}

fn color_channel_u8(channel: f32) -> u8 {
    (channel.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// 字体库只需扫描一次系统字体;sans-serif 别名绑定到实际存在、尽量带
/// CJK 字形的家族,保证文件名可渲染。
fn font_database() -> Arc<fontdb::Database> {
    static DATABASE: OnceLock<Arc<fontdb::Database>> = OnceLock::new();
    DATABASE
        .get_or_init(|| {
            let mut database = fontdb::Database::new();
            database.load_system_fonts();
            let preferred_families = [
                "Noto Sans CJK SC",
                "Noto Sans SC",
                "Source Han Sans SC",
                "Source Han Sans CN",
                "PingFang SC",
                "Microsoft YaHei",
                "Noto Sans",
                "DejaVu Sans",
            ];
            if let Some(family) = preferred_families.iter().find(|family| {
                database
                    .faces()
                    .any(|face| face.families.iter().any(|(name, _)| name == *family))
            }) {
                database.set_sans_serif_family(*family);
            }
            Arc::new(database)
        })
        .clone()
}

fn opaque_bounding_box(pixmap: &Pixmap) -> (u32, u32, u32, u32) {
    let width = pixmap.width();
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (u32::MAX, u32::MAX, 0u32, 0u32);
    for (index, pixel) in pixmap.pixels().iter().enumerate() {
        if pixel.alpha() == 0 {
            continue;
        }
        let x = (index % width as usize) as u32;
        let y = (index / width as usize) as u32;
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x + 1);
        max_y = max_y.max(y + 1);
    }
    (min_x, min_y, max_x, max_y)
}

fn escape_xml_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\'', "&apos;")
        .replace('"', "&quot;")
}

fn hex_color(color: iced::Color) -> String {
    format!(
        "#{:02x}{:02x}{:02x}",
        color_channel_u8(color.r),
        color_channel_u8(color.g),
        color_channel_u8(color.b),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::color;

    fn palette() -> FileDragPillPalette {
        FileDragPillPalette {
            content: color!(0x1f2421),
        }
    }

    fn entry(symbol: IconSymbol, label: &str, x: f32, y: f32) -> FileDragIconEntry {
        FileDragIconEntry {
            symbol,
            label: label.to_owned(),
            offset: iced::Vector::new(x, y),
        }
    }

    #[test]
    fn canvas_covers_all_pills_and_stays_within_protocol_limit() {
        let single = render_wayland_file_drag_icon(
            &[entry(IconSymbol::FolderSolid, "a.txt", 6.0, 6.0)],
            palette(),
        )
        .unwrap();
        let spread = render_wayland_file_drag_icon(
            &[
                entry(IconSymbol::FolderSolid, "a.txt", 6.0, 6.0),
                entry(IconSymbol::FileSolid, "b.txt", 80.0, 90.0),
            ],
            palette(),
        )
        .unwrap();

        assert!(single.width() > 1 && single.height() > 1);
        assert!(spread.width() > single.width());
        assert!(spread.height() > single.height());
        assert!(spread.width() <= DRAG_ICON_CANVAS_MAX_EDGE);
        assert!(spread.height() <= DRAG_ICON_CANVAS_MAX_EDGE);
    }

    #[test]
    fn negative_offsets_shift_into_canvas_without_clipping() {
        // 按住条目中心拖出时偏移为负:整体平移后胶囊必须仍然可见。
        let icon = render_wayland_file_drag_icon(
            &[entry(IconSymbol::FolderSolid, "a.txt", -30.0, -20.0)],
            palette(),
        )
        .unwrap();
        assert!(icon.width() > 1 && icon.height() > 1);
        assert!(icon
            .premultiplied_rgba()
            .chunks_exact(4)
            .any(|pixel| pixel[3] != 0));
    }

    #[test]
    fn distant_entries_fade_out_entirely() {
        let near = render_wayland_file_drag_icon(
            &[entry(IconSymbol::FolderSolid, "a.txt", 6.0, 6.0)],
            palette(),
        )
        .unwrap();
        let with_distant = render_wayland_file_drag_icon(
            &[
                entry(IconSymbol::FolderSolid, "a.txt", 6.0, 6.0),
                entry(IconSymbol::FileSolid, "far.txt", 400.0, 400.0),
            ],
            palette(),
        )
        .unwrap();

        assert_eq!(
            near.width(),
            with_distant.width(),
            "超出淡出半径的条目不参与画布"
        );
    }

    #[test]
    fn mixed_symbols_produce_distinct_bitmaps() {
        let file_first = render_wayland_file_drag_icon(
            &[
                entry(IconSymbol::FileSolid, "a.txt", 6.0, 6.0),
                entry(IconSymbol::FolderSolid, "b", 6.0, 50.0),
            ],
            palette(),
        )
        .unwrap();
        let folder_first = render_wayland_file_drag_icon(
            &[
                entry(IconSymbol::FolderSolid, "b", 6.0, 6.0),
                entry(IconSymbol::FileSolid, "a.txt", 6.0, 50.0),
            ],
            palette(),
        )
        .unwrap();

        assert_ne!(
            file_first.premultiplied_rgba(),
            folder_first.premultiplied_rgba()
        );
    }
}
