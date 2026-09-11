use std::path::Path;
use std::sync::{Arc, OnceLock};

use desktop_linux::WaylandFileDragIcon;
use resvg::tiny_skia::Pixmap;
use resvg::usvg::{fontdb, Options, Tree};

use crate::icons::IconSymbol;

// Wayland 协议把拖拽图标位图的左上角钉在光标上;画布因此以光标为
// 原点向右下展开。协议对 icon surface 没有硬性尺寸限制,512 是为
// 容纳约 15 个列表行而取的裕量(与 desktop-linux 侧校验值保持一致)。
// 组里条目若相对光标在左上,整体平移使最靠左上的图块贴住原点。
const DRAG_ICON_CANVAS_MAX_EDGE: u32 = 512;
// 淡出半径跟随画布尺度:核心距离内全浓,边缘渐隐,画布装不下时
// 由聚合兜底。数值与窗口内预览各自独立,语义保持同一观感。
const DRAG_ICON_FADE_RADIUS: f32 = 512.0;
const DRAG_ICON_FADE_SOLID_DISTANCE: f32 = 192.0;
const DRAG_ICON_MAX_PILLS: usize = 128;

// 条目行几何:24px 裸露图块(缩略图或类型图标)+ 文件名,与窗口内预览一致。
const TILE_SIZE: f32 = 24.0;
const TILE_ICON_LEFT: f32 = 0.0;
const TILE_ICON_TOP: f32 = 0.0;
const PILL_LABEL_LEFT: f32 = TILE_ICON_LEFT + TILE_SIZE + 6.0;
const PILL_LABEL_SIZE: f32 = 12.0;
const PILL_LABEL_BASELINE: f32 = 16.0;
const PILL_LABEL_MAX_WIDTH: f32 = 150.0;
const PILL_PADDING_RIGHT: f32 = 4.0;
// 聚合行:一行总数文字 + 胶囊底板。
const SUMMARY_TEXT_SIZE: f32 = 12.0;
const SUMMARY_TEXT_BASELINE: f32 = 16.0;
const SUMMARY_TEXT_HEIGHT: f32 = 20.0;
const SUMMARY_PAD_X: f32 = 6.0;
const SUMMARY_RADIUS: f32 = 9.0;

/// 拖出软件的位图中的一个条目:裸露的缩略图/类型图标 + 文件名。
#[derive(Debug, Clone)]
pub(crate) struct FileDragIconEntry {
    pub(crate) symbol: IconSymbol,
    pub(crate) label: String,
    /// 相对按下点(提起瞬间的光标位置)的条目原点偏移。
    pub(crate) offset: iced::Vector,
    /// 缩略图文件原始 PNG 字节;无缩略图的条目回退到类型图标。
    pub(crate) thumbnail_png: Option<Vec<u8>>,
}

/// 拖拽图标配色:取自当前主题,与窗口内 drag_preview_panel 同源。
#[derive(Debug, Clone, Copy)]
pub(crate) struct FileDragPillPalette {
    pub(crate) background: iced::Color,
    pub(crate) border: iced::Color,
    pub(crate) content: iced::Color,
}


/// 系统级拖拽图像必须一次性生成位图:按提起瞬间的相对位置摆开各
/// 选中条目的"图标 + 文件名"行,离光标越远越淡,与窗口内预览
/// 保持同一几何与观感。选中组大到画布(256)装不下时整组收拢为
/// `summary` 一行总数文字。
pub(crate) fn render_wayland_file_drag_icon(
    entries: &[FileDragIconEntry],
    summary: Option<&str>,
    palette: FileDragPillPalette,
) -> Result<WaylandFileDragIcon, String> {
    let entries = &entries[..entries.len().min(DRAG_ICON_MAX_PILLS)];
    // 组里条目相对光标可能在左上(负偏移):整体平移让最靠左上的
    // 图块贴住画布原点;全正时不需要平移。
    let min_x = entries
        .iter()
        .map(|entry| entry.offset.x)
        .fold(0.0_f32, f32::min);
    let min_y = entries
        .iter()
        .map(|entry| entry.offset.y)
        .fold(0.0_f32, f32::min);
    let shift_x = (0.0 - min_x).max(0.0);
    let shift_y = (0.0 - min_y).max(0.0);

    let mut tiles = Vec::with_capacity(entries.len());
    let mut canvas_width = 1.0_f32;
    let mut canvas_height = 1.0_f32;
    for entry in entries {
        let position = entry.offset + iced::Vector::new(shift_x, shift_y);
        let fade = drag_icon_fade(position);
        let Some(fade) = fade else {
            continue;
        };
        let label_width = fitted_label_width(&entry.label)?;
        let row_width = PILL_LABEL_LEFT + label_width + PILL_PADDING_RIGHT;
        canvas_width = canvas_width.max(position.x + row_width);
        canvas_height = canvas_height.max(position.y + TILE_SIZE);
        tiles.push((position, row_width, label_width, fade, entry));
    }
    // 画布装不下整组就整体收拢为总数行,而不是裁掉超出部分。
    if (canvas_width > DRAG_ICON_CANVAS_MAX_EDGE as f32
        || canvas_height > DRAG_ICON_CANVAS_MAX_EDGE as f32)
        && summary.is_some()
    {
        return summary_icon_bitmap(summary.expect("checked above"), palette);
    }
    let canvas_width = canvas_width
        .ceil()
        .min(DRAG_ICON_CANVAS_MAX_EDGE as f32) as u32;
    let canvas_height = canvas_height
        .ceil()
        .min(DRAG_ICON_CANVAS_MAX_EDGE as f32) as u32;

    let svg = entry_group_svg(canvas_width, canvas_height, &tiles, palette)?;
    let pixmap = render_svg(&svg, canvas_width, canvas_height)?;
    WaylandFileDragIcon::new(canvas_width, canvas_height, pixmap.take())
        .map_err(|error| error.to_string())
}

/// 聚合位图:一行"文件夹/文件总数"文字,替代铺不开的整组。
fn summary_icon_bitmap(
    summary: &str,
    palette: FileDragPillPalette,
) -> Result<WaylandFileDragIcon, String> {
    const SUMMARY_LEFT: f32 = 6.0;
    let text_width = measure_text_width(summary)?;
    let pill_width = (SUMMARY_LEFT + text_width + SUMMARY_PAD_X)
        .ceil()
        .min(DRAG_ICON_CANVAS_MAX_EDGE as f32);
    let width = pill_width as u32;
    let background = hex_color(palette.background);
    let border = hex_color(palette.border);
    let content = hex_color(palette.content);
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{SUMMARY_TEXT_HEIGHT}\">\
<rect x=\"0.5\" y=\"0.5\" width=\"{inner_width}\" height=\"{inner_height}\" \
rx=\"{SUMMARY_RADIUS}\" fill=\"{background}\" stroke=\"{border}\" stroke-width=\"1\"/>\
<text x=\"{SUMMARY_LEFT}\" y=\"{SUMMARY_TEXT_BASELINE}\" font-family=\"sans-serif\" \
font-size=\"{SUMMARY_TEXT_SIZE}\" fill=\"{content}\">{escaped}</text></svg>",
        inner_width = pill_width - 1.0,
        inner_height = SUMMARY_TEXT_HEIGHT - 1.0,
        escaped = escape_xml_text(summary),
    );
    let pixmap = render_svg(&svg, width, SUMMARY_TEXT_HEIGHT as u32)?;
    WaylandFileDragIcon::new(width, SUMMARY_TEXT_HEIGHT as u32, pixmap.take())
        .map_err(|error| error.to_string())
}

/// 聚合行文案,窗口内预览与拖出位图共用同一份措辞。
pub(crate) fn file_drag_group_summary_text(folder_count: usize, file_count: usize) -> String {
    if crate::localization::current_language_is_chinese() {
        match (folder_count, file_count) {
            (0, files) => format!("{files} 个文件"),
            (folders, 0) => format!("{folders} 个文件夹"),
            (folders, files) => format!("{folders} 个文件夹,{files} 个文件"),
        }
    } else {
        match (folder_count, file_count) {
            (0, files) => format!("{files} files"),
            (folders, 0) => format!("{folders} folders"),
            (folders, files) => format!("{folders} folders, {files} files"),
        }
    }
}

/// 离光标(画布原点)越远越淡:核心距离内全浓,超出淡出半径不显示。
fn drag_icon_fade(position: iced::Vector) -> Option<f32> {
    let distance = (position.x * position.x + position.y * position.y).sqrt();
    let fade = (DRAG_ICON_FADE_RADIUS - distance)
        / (DRAG_ICON_FADE_RADIUS - DRAG_ICON_FADE_SOLID_DISTANCE);
    (fade > 0.0).then(|| fade.clamp(0.0, 1.0))
}

/// 整个拖拽图标一次矢量渲染:各条目的缩略图/类型图标与文件名画进
/// 同一张 SVG(全部裸露,无底板),透明度随距离衰减。
fn entry_group_svg(
    canvas_width: u32,
    canvas_height: u32,
    tiles: &[(iced::Vector, f32, f32, f32, &FileDragIconEntry)],
    palette: FileDragPillPalette,
) -> Result<String, String> {
    let content = hex_color(palette.content);
    let mut groups = String::new();
    for (position, _, label_width, fade, entry) in tiles {
        let icon_x = position.x + TILE_ICON_LEFT;
        let icon_y = position.y + TILE_ICON_TOP;
        let label_x = position.x + PILL_LABEL_LEFT;
        let label = truncate_label_to_width(&entry.label, *label_width)?;
        let leading = match &entry.thumbnail_png {
            Some(png) => format!(
                "<image x=\"{icon_x}\" y=\"{icon_y}\" width=\"{TILE_SIZE}\" \
height=\"{TILE_SIZE}\" href=\"data:image/png;base64,{}\"/>",
                base64_encode(png)
            ),
            None => nested_icon_svg(entry.symbol, icon_x, icon_y, &content),
        };
        groups.push_str(&format!(
            "<g opacity=\"{fade:.3}\">{leading}\
<text x=\"{label_x}\" y=\"{label_y}\" font-family=\"sans-serif\" \
font-size=\"{PILL_LABEL_SIZE}\" fill=\"{content}\">{escaped_label}</text></g>",
            label_y = position.y + PILL_LABEL_BASELINE,
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
/// 文件名按实测像素截断,保证不超出条目行预留宽度。
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

/// 文件名显示名:与窗口内预览同一截断规则。
pub(crate) fn file_drag_display_name(path: &Path) -> String {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("?");
    crate::formatting::format_middle_ellipsized_text(name, 20)
}

fn nested_icon_svg(symbol: IconSymbol, x: f32, y: f32, content_hex: &str) -> String {
    let raw = String::from_utf8_lossy(symbol.bytes())
        .replace("currentColor", content_hex);
    raw.replacen("<svg ", &format!("<svg x=\"{x}\" y=\"{y}\" "), 1)
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
<text x=\"0\" y=\"{SUMMARY_TEXT_BASELINE}\" font-family=\"sans-serif\" \
font-size=\"{SUMMARY_TEXT_SIZE}\" fill=\"{content}\">{escaped}</text></svg>",
        escaped = escape_xml_text(text),
    );
    let pixmap = render_svg(&svg, MEASURE_CANVAS_WIDTH, MEASURE_CANVAS_HEIGHT)?;
    let (_, _, max_x, _) = opaque_bounding_box(&pixmap);
    Ok(max_x as f32)
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

/// 缩略图 PNG 以 data URI 嵌入 SVG,避免 resvg 读取外部文件。
fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let byte = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let group = (u32::from(byte[0]) << 16) | (u32::from(byte[1]) << 8) | u32::from(byte[2]);
        encoded.push(TABLE[(group >> 18) as usize & 63] as char);
        encoded.push(TABLE[(group >> 12) as usize & 63] as char);
        encoded.push(if chunk.len() > 1 {
            TABLE[(group >> 6) as usize & 63] as char
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            TABLE[group as usize & 63] as char
        } else {
            '='
        });
    }
    encoded
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
            background: color!(0xf8faf9),
            border: color!(0xd9ddd9),
            content: color!(0x1f2421),
        }
    }

    fn entry(symbol: IconSymbol, label: &str, x: f32, y: f32) -> FileDragIconEntry {
        FileDragIconEntry {
            symbol,
            label: label.to_owned(),
            offset: iced::Vector::new(x, y),
            thumbnail_png: None,
        }
    }

    #[test]
    fn canvas_covers_all_pills_and_stays_within_protocol_limit() {
        let single = render_wayland_file_drag_icon(
            &[entry(IconSymbol::FolderSolid, "a.txt", 6.0, 6.0)],
            None,
            palette(),
        )
        .unwrap();
        let spread = render_wayland_file_drag_icon(
            &[
                entry(IconSymbol::FolderSolid, "a.txt", 6.0, 6.0),
                entry(IconSymbol::FileSolid, "a.txt", 80.0, 90.0),
            ],
            None,
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
            None,
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
            None,
            palette(),
        )
        .unwrap();
        let with_distant = render_wayland_file_drag_icon(
            &[
                entry(IconSymbol::FolderSolid, "a.txt", 6.0, 6.0),
                entry(IconSymbol::FileSolid, "a.txt", 400.0, 400.0),
            ],
            Some("3 folders, 1 file"),
            palette(),
        )
        .unwrap();

        // 远处条目按淡出规则本就不参与画布,聚合兜底不该被触发。

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
                entry(IconSymbol::FolderSolid, "a.txt", 6.0, 50.0),
            ],
            None,
            palette(),
        )
        .unwrap();
        let folder_first = render_wayland_file_drag_icon(
            &[
                entry(IconSymbol::FolderSolid, "a.txt", 6.0, 6.0),
                entry(IconSymbol::FileSolid, "a.txt", 6.0, 50.0),
            ],
            None,
            palette(),
        )
        .unwrap();

        assert_ne!(
            file_first.premultiplied_rgba(),
            folder_first.premultiplied_rgba()
        );
    }

    #[test]
    fn oversized_group_collapses_into_summary_bitmap() {
        let mut entries = Vec::new();
        for index in 0..24 {
            entries.push(entry(IconSymbol::FolderSolid, "a.txt", 0.0, index as f32 * 24.0));
        }
        let collapsed =
            render_wayland_file_drag_icon(&entries, Some("3 folders, 9 files"), palette())
                .unwrap();
        let uncapped =
            render_wayland_file_drag_icon(&entries, None, palette()).unwrap();

        // 有 summary 时画布收拢为一行且不超协议上限;无 summary 才按
        // 上限裁剪(保持旧行为兜底)。
        assert_eq!(collapsed.height(), SUMMARY_TEXT_HEIGHT as u32);
    }
}
