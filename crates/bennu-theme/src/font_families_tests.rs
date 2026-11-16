use super::*;
use iced::advanced::graphics::text::cosmic_text;

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

/// 界面语言决定汉字按哪个地区的写法；带地区的日文、韩文也要认出来（cosmic-text 只认光秃秃的 ja、ko）。
#[test]
fn the_interface_language_picks_the_han_region() {
    for (locale, region) in [
        ("zh-CN", HanRegion::Simplified),
        ("zh_CN", HanRegion::Simplified),
        ("zh-Hans-SG", HanRegion::Simplified),
        ("zh-TW", HanRegion::Taiwan),
        ("zh-Hant", HanRegion::Taiwan),
        ("zh-HK", HanRegion::HongKong),
        ("ja-JP", HanRegion::Japan),
        ("ja", HanRegion::Japan),
        ("ko-KR", HanRegion::Korea),
        ("en-US", HanRegion::Simplified),
        ("", HanRegion::Simplified),
    ] {
        assert_eq!(HanRegion::of_locale(locale), region, "{locale}");
    }
}

/// 汉字先用系统给的（换成对应地区的版本），再找对应写法的 Noto CJK；假名、谚文各按日、韩；
/// 别的文字照旧。
#[test]
fn cjk_scripts_fall_back_to_the_system_choice_then_noto() {
    let same = |family: &str, _region: HanRegion| family.to_owned();
    let fallback = CjkFallback::new(
        HanRegion::Taiwan,
        Some("WenQuanYi Zen Hei".to_owned()),
        same,
    );
    assert_eq!(
        fallback.script_fallback(Script::Han, "zh-TW"),
        ["WenQuanYi Zen Hei", "Noto Sans CJK TC"]
    );
    assert_eq!(
        fallback.script_fallback(Script::Katakana, "zh-TW"),
        ["WenQuanYi Zen Hei", "Noto Sans CJK JP"]
    );
    assert_eq!(
        fallback.script_fallback(Script::Hangul, "zh-TW"),
        ["WenQuanYi Zen Hei", "Noto Sans CJK KR"]
    );
    assert_eq!(
        fallback.script_fallback(Script::Arabic, "zh-TW"),
        PlatformFallback.script_fallback(Script::Arabic, "zh-TW")
    );

    let without_system = CjkFallback::new(HanRegion::Simplified, None, same);
    assert_eq!(
        without_system.script_fallback(Script::Han, "zh-CN"),
        ["Noto Sans CJK SC"]
    );

    // 系统挑的是韩文版 Noto CJK：换成简中版以后与 Noto 那一项相同，不重复。
    let database = database_with_families(&["Noto Sans CJK KR", "Noto Sans CJK SC"]);
    let korean = CjkFallback::new(
        HanRegion::Simplified,
        Some("Noto Sans CJK KR".to_owned()),
        |family, region| regional_variant(&database, family, region),
    );
    assert_eq!(
        korean.script_fallback(Script::Han, "zh-CN"),
        ["Noto Sans CJK SC"]
    );
    assert_eq!(
        korean.script_fallback(Script::Hangul, "zh-CN"),
        ["Noto Sans CJK KR"]
    );
}

/// 用 Fira Sans 的字形造出给定名字、字重的字体家族。
fn database_with_faces(faces: &[(&str, u16)]) -> fontdb::Database {
    let mut database = fontdb::Database::new();
    for &(family, weight) in faces {
        let ids = database.load_font_source(fontdb::Source::Binary(std::sync::Arc::new(
            iced::advanced::graphics::text::FIRA_SANS_REGULAR,
        )));
        for id in ids {
            let mut face = database.face(id).expect("loaded face").clone();
            database.remove_face(id);
            face.families = vec![(family.to_owned(), fontdb::Language::English_UnitedStates)];
            face.weight = fontdb::Weight(weight);
            database.push_face_info(face);
        }
    }
    database
}

fn database_with_families(families: &[&str]) -> fontdb::Database {
    database_with_faces(
        &families
            .iter()
            .map(|family| (*family, 400))
            .collect::<Vec<_>>(),
    )
}

/// 分地区命名的字体换成界面语言的版本（装着才换）；不分地区的原样。
#[test]
fn a_regional_cjk_family_is_swapped_for_the_interface_region() {
    let database = database_with_families(&[
        "Noto Sans CJK KR",
        "Noto Sans CJK SC",
        "Source Han Sans TW",
        "Sarasa UI J",
        "Sarasa UI SC",
    ]);
    let variant = |family, region| regional_variant(&database, family, region);
    assert_eq!(
        variant("Noto Sans CJK KR", HanRegion::Simplified),
        "Noto Sans CJK SC"
    );
    assert_eq!(
        variant("Sarasa UI J", HanRegion::Simplified),
        "Sarasa UI SC"
    );
    assert_eq!(
        variant("Noto Sans CJK SC", HanRegion::Korea),
        "Noto Sans CJK KR"
    );
    // 没装对应版本：保持原样。
    assert_eq!(
        variant("Source Han Sans TW", HanRegion::Simplified),
        "Source Han Sans TW"
    );
    assert_eq!(
        variant("WenQuanYi Zen Hei", HanRegion::Japan),
        "WenQuanYi Zen Hei"
    );
    assert_eq!(variant("Noto Sans", HanRegion::Simplified), "Noto Sans");
}

/// 选了没装的字体等于跟随系统；没选也跟随系统。
#[test]
fn a_missing_choice_follows_the_system() {
    let mut database = fontdb::Database::new();
    database.load_font_data(iced::advanced::graphics::text::FIRA_SANS_REGULAR.to_vec());
    let system = || Some("System Sans".to_owned());
    assert_eq!(
        chosen_or_system(&database, "Fira Sans", system()),
        Some("Fira Sans".to_owned())
    );
    assert_eq!(chosen_or_system(&database, "Gone Sans", system()), system());
    assert_eq!(chosen_or_system(&database, "", system()), system());
}

/// 比例字体（Fira Sans 的 i 比 M 窄）不进等宽列表。
#[test]
fn a_proportional_font_is_not_monospace() {
    let mut database = fontdb::Database::new();
    database.load_font_data(iced::advanced::graphics::text::FIRA_SANS_REGULAR.to_vec());
    let id = database.faces().next().expect("Fira Sans face").id;
    assert!(!has_monospace_latin(&database, id));
}

/// 终端的格子和排出来的字一样宽：一段 run 里第 n 个字正好落在第 n 格。
#[test]
fn a_terminal_cell_is_as_wide_as_a_laid_out_monospace_glyph() {
    let mut database = database_with_families(&["Grid Mono"]);
    database.set_monospace_family("Grid Mono");
    let advance = measure_monospace_advance(&database) * 14.0;
    let mut font_system = FontSystem::new_with_locale_and_db("en-US".to_owned(), database);
    let positions: Vec<f32> = glyphs_laid_out(&mut font_system, &"0".repeat(80), 14.0)
        .iter()
        .map(|glyph| glyph.x)
        .collect();
    assert_eq!(positions.len(), 80);
    for (column, x) in positions.into_iter().enumerate() {
        assert!(
            (x - column as f32 * advance).abs() < 1e-3,
            "{column}: {x} vs {advance}"
        );
    }
}

/// 同一个字形不管排在一行的哪里、缩放多少，都落在整像素上、只有一种点阵（亚像素档位为 0）。
#[test]
fn glyphs_land_on_whole_device_pixels() {
    let mut font_system = FontSystem::new_with_locale_and_db(
        "en-US".to_owned(),
        database_with_families(&["Grid Sans"]),
    );
    let glyphs = glyphs_laid_out(&mut font_system, "states states states", 13.0);
    for scale in [1.0, 1.25, 1.5, 1.75] {
        for glyph in &glyphs {
            let physical = glyph.physical((3.4, 0.0), scale);
            assert_eq!(physical.cache_key.x_bin, cosmic_text::SubpixelBin::Zero);
            let exact = glyph.x.mul_add(scale, 3.4);
            assert!((physical.x as f32 - exact).abs() <= 0.5, "{scale}: {exact}");
        }
    }
}

fn glyphs_laid_out(
    font_system: &mut FontSystem,
    text: &str,
    size: f32,
) -> Vec<cosmic_text::LayoutGlyph> {
    let mut buffer = cosmic_text::Buffer::new(font_system, cosmic_text::Metrics::new(size, size));
    buffer.set_text(
        font_system,
        text,
        &cosmic_text::Attrs::new().family(cosmic_text::Family::Monospace),
        cosmic_text::Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(font_system, false);
    buffer
        .layout_runs()
        .flat_map(|run| run.glyphs.to_vec())
        .collect()
}

fn faces_used(font_system: &mut FontSystem, family: cosmic_text::Family<'_>) -> Vec<String> {
    let mut buffer = cosmic_text::Buffer::new(font_system, cosmic_text::Metrics::new(13.0, 17.0));
    buffer.set_text(
        font_system,
        "abc",
        &cosmic_text::Attrs::new().family(family),
        cosmic_text::Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(font_system, false);
    let mut used: Vec<String> = Vec::new();
    for run in buffer.layout_runs() {
        for glyph in run.glyphs {
            let name = font_system.db().face(glyph.font_id).expect("face").families[0]
                .0
                .clone();
            if !used.contains(&name) {
                used.push(name);
            }
        }
    }
    used
}

/// 只有 500 字重的家族（「Weighted」，另有 400 的「Decoy」）照样用得上：选它当界面字体时直接用它，作为回退列表里的一项时也用它，
/// 不会被别的 400 字重字体顶掉（排版库只认字重正好相等时，文泉驿正黑就是这样被跳过的）。
#[test]
fn a_family_without_a_regular_weight_is_still_used() {
    let mut chosen = FontSystem::new_with_locale_and_db(
        "en-US".to_owned(),
        database_with_faces(&[("Weighted", 500), ("Decoy", 400)]),
    );
    assert_eq!(
        faces_used(&mut chosen, cosmic_text::Family::Name("Weighted")),
        ["Weighted"]
    );

    struct WeightedFallback;
    impl Fallback for WeightedFallback {
        fn common_fallback(&self) -> &[&'static str] {
            &["Weighted"]
        }
        fn forbidden_fallback(&self) -> &[&'static str] {
            &[]
        }
        fn script_fallback(&self, _script: Script, _locale: &str) -> &[&'static str] {
            &[]
        }
    }
    let mut fallback = FontSystem::new_with_locale_and_db_and_fallback(
        "en-US".to_owned(),
        database_with_faces(&[("Weighted", 500), ("Decoy", 400)]),
        WeightedFallback,
    );
    assert_eq!(
        faces_used(&mut fallback, cosmic_text::Family::Name("Missing")),
        ["Weighted"]
    );
}
