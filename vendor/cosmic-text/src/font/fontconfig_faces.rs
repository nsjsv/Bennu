// SPDX-License-Identifier: MIT OR Apache-2.0

//! Bennu 补丁：系统字体的列表从 fontconfig 的缓存里取，不在启动时解析每个字体文件。
//!
//! 上游用 fontdb 的 `load_system_fonts` 打开并解析每一个字体文件：装了几百个字体就要读上百 MB，
//! 开机后第一次启动、盘又慢的机器上光这一步要等近 10 秒。fontconfig 已经把每个字体的家族名、
//! 字重、样式、等宽这些信息缓存好了（桌面上别的程序都靠它），`fc-list` 读缓存只要十几毫秒；
//! 字体文件等真正用到时再映射进内存（`FontSystem::get_font`）。

use std::path::Path;
use std::process::{Command, Stdio};

/// 一个字体一条记录（RS 结尾），字段用 US 分开；家族名和它的语言成对列出（GS 分开、FS 结尾）。
/// 这几个控制字符不会出现在路径和字体名里。
pub const FONT_LIST_FORMAT: &str = "%{file}\u{1f}%{index}\u{1f}%{variable}\u{1f}%{slant}\u{1f}\
%{weight}\u{1f}%{width}\u{1f}%{spacing}\u{1f}%{postscriptname}\u{1f}\
%{[]family,familylang{%{family}\u{1d}%{familylang}\u{1c}}}\u{1e}";

/// 把系统字体加进 `db`：按 fontconfig 的缓存；没有 fontconfig 时退回逐个解析字体文件。
pub fn load_system_faces(db: &mut fontdb::Database) {
    let Some(faces) = faces_from_fontconfig() else {
        db.load_system_fonts();
        return;
    };
    for face in faces {
        db.push_face_info(face);
    }
}

fn faces_from_fontconfig() -> Option<Vec<fontdb::FaceInfo>> {
    let output = Command::new("fc-list")
        .arg("--format")
        .arg(FONT_LIST_FORMAT)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .inspect_err(|error| log::warn!("fc-list could not run: {error}"))
        .ok()?;
    output
        .status
        .success()
        .then(|| parse_font_list(&String::from_utf8_lossy(&output.stdout)))
}

/// 按 `FONT_LIST_FORMAT` 列出的字体，换成 fontdb 解析字体文件会得到的样子。
pub fn parse_font_list(list: &str) -> Vec<fontdb::FaceInfo> {
    let mut faces = Vec::new();
    for record in list.split('\u{1e}') {
        let fields: Vec<&str> = record.split('\u{1f}').collect();
        let [file, index, variable, slant, weight, width, spacing, post_script_name, families] =
            fields[..]
        else {
            continue;
        };
        let path = Path::new(file);
        // 和 fontdb 扫字体目录时收的扩展名一样：PCF、Type 1 这类它解析不了。
        if !matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("ttf" | "ttc" | "otf" | "otc" | "TTF" | "TTC" | "OTF" | "OTC")
        ) {
            continue;
        }
        let Ok(index) = index.parse::<u32>() else {
            continue;
        };
        // 可变字体 fontconfig 列三种：默认那一款（不标可变，字重、名字与 fontdb 读到的一样）、
        // 标可变的整体（字重是个范围）、各命名实例（高 16 位是实例号）。fontdb 只认默认那一款，
        // 排版时按字重调轴，所以只留它。
        if variable == "True" || index >> 16 != 0 {
            continue;
        }
        let (Ok(weight), Ok(width)) = (weight.parse::<f64>(), width.parse::<f64>()) else {
            continue;
        };
        let families = families_by_language(families);
        if families.is_empty() {
            continue;
        }
        faces.push(fontdb::FaceInfo {
            id: fontdb::ID::dummy(),
            source: fontdb::Source::File(path.to_owned()),
            index,
            families,
            post_script_name: post_script_name.to_owned(),
            // fontconfig 按样式名把「Oblique」记成倾斜（110）；fontdb 读字体头时这类字体
            // （DejaVu 的 Oblique）标的是斜体，算 Italic。排版按样式精确匹配，要 Italic 的字
            // 得能找到它们，所以都算 Italic；字体头专门标「倾斜」的极少。
            style: match slant {
                "100" | "110" => fontdb::Style::Italic,
                _ => fontdb::Style::Normal,
            },
            weight: fontdb::Weight(opentype_weight(weight)),
            stretch: stretch(width),
            // 100 是等宽，110 是字符单元；90 是中日韩等宽字体那种双宽。fontconfig 除了看字体头的
            // 等宽标记，还看字宽是不是全一样，没标记的等宽字体（一些 Nerd Font）也算等宽。
            monospaced: matches!(spacing, "100" | "110"),
        });
    }
    faces
}

/// 家族名全留下（去掉重复），英文的放最前面：fontdb 读字体名表时第一个是英文名，别处拿它当
/// 显示名。fontconfig 还会列出旧式家族名（「JetBrainsMono NF」「Noto Sans Medium」），留着只是多
/// 几个能匹配上的名字。语言只分英文与其他：没有地方按语言查。
fn families_by_language(list: &str) -> Vec<(String, fontdb::Language)> {
    let mut families: Vec<(String, fontdb::Language)> = Vec::new();
    for (family, language) in list
        .split('\u{1c}')
        .filter_map(|pair| pair.split_once('\u{1d}'))
    {
        if family.is_empty() || families.iter().any(|(known, _)| known == family) {
            continue;
        }
        let language = if language == "en" {
            fontdb::Language::English_UnitedStates
        } else {
            fontdb::Language::Unknown
        };
        families.push((family.to_owned(), language));
    }
    if let Some(english) = families
        .iter()
        .position(|(_, language)| *language == fontdb::Language::English_UnitedStates)
    {
        families[..=english].rotate_right(1);
    }
    families
}

/// fontconfig 的字重刻度换回字体里的 OpenType 字重：fontconfig 读字体时用同一张分段线性表换过去。
fn opentype_weight(fontconfig: f64) -> u16 {
    const SCALE: [(f64, f64); 12] = [
        (0.0, 100.0),
        (40.0, 200.0),
        (50.0, 300.0),
        (55.0, 350.0),
        (75.0, 380.0),
        (80.0, 400.0),
        (100.0, 500.0),
        (180.0, 600.0),
        (200.0, 700.0),
        (205.0, 800.0),
        (210.0, 900.0),
        (215.0, 1000.0),
    ];
    let fontconfig = fontconfig.clamp(0.0, 215.0);
    let upper = SCALE
        .iter()
        .position(|&(scale, _)| fontconfig <= scale)
        .unwrap_or(SCALE.len() - 1)
        .max(1);
    let ((low_fc, low_ot), (high_fc, high_ot)) = (SCALE[upper - 1], SCALE[upper]);
    let opentype = low_ot + (fontconfig - low_fc) * (high_ot - low_ot) / (high_fc - low_fc);
    opentype.round() as u16
}

/// fontconfig 的宽度是百分比；取最接近的一档。
fn stretch(width: f64) -> fontdb::Stretch {
    use fontdb::Stretch;
    [
        (50.0, Stretch::UltraCondensed),
        (62.5, Stretch::ExtraCondensed),
        (75.0, Stretch::Condensed),
        (87.5, Stretch::SemiCondensed),
        (100.0, Stretch::Normal),
        (112.5, Stretch::SemiExpanded),
        (125.0, Stretch::Expanded),
        (150.0, Stretch::ExtraExpanded),
        (200.0, Stretch::UltraExpanded),
    ]
    .into_iter()
    .min_by(|(a, _), (b, _)| (a - width).abs().total_cmp(&(b - width).abs()))
    .map_or(Stretch::Normal, |(_, stretch)| stretch)
}
