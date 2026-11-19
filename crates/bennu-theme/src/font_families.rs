//! 界面用哪些字体：跟随系统（fontconfig）或用户在设置里选的，以及汉字等的回退。

use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use iced::advanced::graphics::text::cosmic_text::{
    fontconfig_faces, fontdb, skrifa, Fallback, FontSystem, PlatformFallback,
};
use skrifa::MetadataProvider;
use unicode_script::Script;

/// fontconfig 首次运行可能要重建缓存；超过这个时间就放弃，界面继续用 cosmic-text 默认字体。
const FONTCONFIG_QUERY_TIMEOUT: Duration = Duration::from_millis(300);
const FONTCONFIG_POLL_INTERVAL: Duration = Duration::from_millis(2);

/// 让 `Font::DEFAULT` / `Font::MONOSPACE` 落到用户选的字体（空串 = 跟随系统）上，并按系统的设置
/// 给汉字找字体；必须在 iced 首次排版前调用。
///
/// cosmic-text 0.15 把 sans-serif 写死成 "Open Sans"、monospace 写死成 "Noto Sans Mono"，
/// 系统没装时逐字回退，中英文就会落到不同字体。这里改的是家族别名本身，所以所有
/// `Font::DEFAULT`、`..Font::DEFAULT` 构造的粗体、`Font::MONOSPACE` 调用点都跟着变。
pub fn adopt_font_families(interface: &str, monospace: &str) {
    let locale = font_system().raw().locale().to_owned();
    let region = HanRegion::of_locale(&locale);
    // 三个查询并行跑，启动只等最慢的那个。
    // 拉丁字母的字体按英文问：中文环境下 fontconfig 给 sans-serif 的是中文字体（没配置过时
    // 常是韩文版 Noto CJK），西文也会用它；汉字另走 `CjkFallback`。
    let family = |pattern| spawn_fontconfig(&["-f", "%{family[0]}", pattern]);
    let sans = family("sans-serif:lang=en");
    let mono = family("monospace:lang=en");
    let language = region.fontconfig_language();
    let han = spawn_fontconfig(&[
        "-s",
        "-f",
        "%{family[0]}\u{1f}%{lang}\u{1e}",
        &format!("sans-serif:lang={language}"),
    ]);
    let sans = sans
        .and_then(finish_fontconfig_query)
        .and_then(|stdout| parse_fontconfig_family(&stdout));
    let mono = mono
        .and_then(finish_fontconfig_query)
        .and_then(|stdout| parse_fontconfig_family(&stdout));
    let han = han
        .and_then(finish_fontconfig_query)
        .map(|stdout| families_supporting(&stdout, language))
        .unwrap_or_default();

    let mut font_system = font_system();
    let raw = font_system.raw();
    let (locale, mut database) = std::mem::replace(
        raw,
        FontSystem::new_with_locale_and_db(String::new(), fontdb::Database::new()),
    )
    .into_locale_and_db();
    let system =
        |family: Option<String>| family.map(|family| regional_variant(&database, &family, region));
    let sans = chosen_or_system(&database, interface, system(sans));
    let mono = chosen_or_system(&database, monospace, system(mono));
    let fallback = CjkFallback::new(region, &han, |family, region| {
        regional_variant(&database, family, region)
    });
    if let Some(family) = sans {
        database.set_sans_serif_family(family);
    }
    if let Some(family) = mono {
        database.set_monospace_family(family);
    }
    *raw = FontSystem::new_with_locale_and_db_and_fallback(locale, database, fallback);
}

fn font_system() -> std::sync::RwLockWriteGuard<'static, iced::advanced::graphics::text::FontSystem>
{
    iced::advanced::graphics::text::font_system()
        .write()
        .expect("font system lock poisoned")
}

/// 等宽字体一个字符多宽（单位 em），终端按它划格子：用户能换等宽字体，各款宽度不同
/// （0.5em～0.6em 都常见），写死的话字和光标、选区会错开。字体在启动时定下，量一次就够。
pub fn monospace_advance_em() -> f32 {
    static ADVANCE: OnceLock<f32> = OnceLock::new();
    *ADVANCE.get_or_init(|| measure_monospace_advance(font_system().raw().db()))
}

fn measure_monospace_advance(database: &fontdb::Database) -> f32 {
    let query = fontdb::Query {
        families: &[fontdb::Family::Monospace],
        ..fontdb::Query::default()
    };
    database
        .query(&query)
        .and_then(|id| latin_advances(database, id, ['0']))
        .map(|[zero]| zero)
        // 系统一个等宽字体都没有：取多数等宽字体的宽度，cosmic-text 回退到的字体也差不多。
        .unwrap_or(0.6)
}

/// 用户选的字体装着就用它；没选或已经卸掉了跟随系统。
fn chosen_or_system(
    database: &fontdb::Database,
    chosen: &str,
    system: Option<String>,
) -> Option<String> {
    let installed = database
        .faces()
        .any(|face| face.families.iter().any(|(name, _)| name == chosen));
    if chosen.is_empty() || !installed {
        system
    } else {
        Some(chosen.to_owned())
    }
}

/// 汉字按哪个地区的写法：同一个字在简中、繁中、日文、韩文字体里字形不同。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HanRegion {
    Simplified,
    Taiwan,
    HongKong,
    Japan,
    Korea,
}

impl HanRegion {
    /// 界面语言（`zh-CN`、`ja-JP` 这类）对应的写法；不是中日韩的语言按简中，与 cosmic-text 一致。
    fn of_locale(locale: &str) -> Self {
        let mut parts = locale.split(['-', '_']);
        let language = parts.next().unwrap_or_default().to_ascii_lowercase();
        let rest: Vec<String> = parts.map(str::to_ascii_lowercase).collect();
        let has = |tag: &str| rest.iter().any(|part| part == tag);
        match language.as_str() {
            "ja" => Self::Japan,
            "ko" => Self::Korea,
            "zh" if has("hk") || has("mo") => Self::HongKong,
            "zh" if has("tw") || has("hant") => Self::Taiwan,
            _ => Self::Simplified,
        }
    }

    fn noto_family(self) -> &'static str {
        match self {
            Self::Simplified => "Noto Sans CJK SC",
            Self::Taiwan => "Noto Sans CJK TC",
            Self::HongKong => "Noto Sans CJK HK",
            Self::Japan => "Noto Sans CJK JP",
            Self::Korea => "Noto Sans CJK KR",
        }
    }

    /// 中日韩字体按地区分版本时家族名末尾的标记（Noto Sans CJK SC、思源黑体 TW、更纱黑体 J）。
    fn family_suffixes(self) -> &'static [&'static str] {
        match self {
            Self::Simplified => &["SC", "CN"],
            Self::Taiwan => &["TC", "TW"],
            Self::HongKong => &["HK", "HC", "TC"],
            Self::Japan => &["JP", "J"],
            Self::Korea => &["KR", "K"],
        }
    }

    fn fontconfig_language(self) -> &'static str {
        match self {
            Self::Simplified => "zh-cn",
            Self::Taiwan => "zh-tw",
            Self::HongKong => "zh-hk",
            Self::Japan => "ja",
            Self::Korea => "ko",
        }
    }
}

/// 同一套中日韩字体换成 `region` 的版本：fontconfig 在几个地区版本字形覆盖一样时随便挑一个
/// （没配置过的系统给中文挑的是 Noto Sans CJK KR），汉字就成了别的地区的写法。装着对应版本才换，
/// 不是分地区命名的字体原样返回。
fn regional_variant(database: &fontdb::Database, family: &str, region: HanRegion) -> String {
    let base = [
        HanRegion::Simplified,
        HanRegion::Taiwan,
        HanRegion::HongKong,
        HanRegion::Japan,
        HanRegion::Korea,
    ]
    .into_iter()
    .flat_map(HanRegion::family_suffixes)
    .find_map(|suffix| family.strip_suffix(suffix)?.strip_suffix(' '));
    let installed = |name: &str| {
        database
            .faces()
            .any(|face| face.families.iter().any(|(known, _)| known == name))
    };
    base.into_iter()
        .flat_map(|base| {
            region
                .family_suffixes()
                .iter()
                .map(move |suffix| format!("{base} {suffix}"))
        })
        .find(|variant| installed(variant))
        .unwrap_or_else(|| family.to_owned())
}

/// 汉字、注音、假名、谚文的回退：先用 fontconfig 给界面语言排好的、支持这门语言的字体（换成
/// 对应地区的版本），再找对应写法的 Noto CJK，其余文字照 cosmic-text 的列表。
///
/// cosmic-text 只认得 Noto CJK；系统没装时按字体加载顺序挑第一个有这个字的，会在中文界面挑到
/// 日文宋体，一个词里还会因为缺字混进第二种字体。
struct CjkFallback {
    han: &'static [&'static str],
    kana: &'static [&'static str],
    hangul: &'static [&'static str],
}

impl CjkFallback {
    /// 每个进程只建一次，列表做成 `'static`（`Fallback` 要求的类型）。`variant` 把系统挑的字体
    /// 换成某个地区的版本（`regional_variant`）。
    fn new(
        region: HanRegion,
        system: &[String],
        variant: impl Fn(&str, HanRegion) -> String,
    ) -> Self {
        let list = |region: HanRegion| -> &'static [&'static str] {
            let mut families: Vec<&'static str> = Vec::new();
            for family in system {
                let family = variant(family, region);
                if !families.contains(&family.as_str()) {
                    families.push(Box::leak(family.into_boxed_str()));
                }
            }
            if !families.contains(&region.noto_family()) {
                families.push(region.noto_family());
            }
            Box::leak(families.into_boxed_slice())
        };
        Self {
            han: list(region),
            kana: list(HanRegion::Japan),
            hangul: list(HanRegion::Korea),
        }
    }
}

impl Fallback for CjkFallback {
    fn common_fallback(&self) -> &[&'static str] {
        PlatformFallback.common_fallback()
    }

    fn forbidden_fallback(&self) -> &[&'static str] {
        PlatformFallback.forbidden_fallback()
    }

    fn script_fallback(&self, script: Script, locale: &str) -> &[&'static str] {
        match script {
            Script::Han | Script::Bopomofo => self.han,
            Script::Hiragana | Script::Katakana => self.kana,
            Script::Hangul => self.hangul,
            _ => PlatformFallback.script_fallback(script, locale),
        }
    }
}

/// 系统里装了的字体家族，给设置里选。读字体文件，在后台线程调用。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstalledFonts {
    /// 全部家族，按名字排序。
    pub families: Vec<String>,
    /// 其中拉丁字母等宽的（「i」和「M」一样宽）：中日韩等宽字体的汉字占两格，也算。
    pub monospace: Vec<String>,
}

pub fn installed_fonts() -> InstalledFonts {
    let mut database = fontdb::Database::new();
    fontconfig_faces::load_system_faces(&mut database);
    let mut families: Vec<(String, fontdb::ID)> = Vec::new();
    for face in database.faces() {
        let Some((name, _)) = face.families.first() else {
            continue;
        };
        // 同一家族取常规体判断等宽：粗体、斜体的宽度与它一致。
        match families.iter_mut().find(|(known, _)| known == name) {
            Some(entry)
                if face.weight == fontdb::Weight::NORMAL && face.style == fontdb::Style::Normal =>
            {
                entry.1 = face.id;
            }
            Some(_) => {}
            None => families.push((name.clone(), face.id)),
        }
    }
    families.sort_by_cached_key(|(name, _)| name.to_lowercase());
    let monospace = families
        .iter()
        .filter(|(_, id)| has_monospace_latin(&database, *id))
        .map(|(name, _)| name.clone())
        .collect();
    InstalledFonts {
        families: families.into_iter().map(|(name, _)| name).collect(),
        monospace,
    }
}

/// 字体文件头里的「等宽」标记不可靠（中日韩等宽字体常不标），按字形宽度看。
fn has_monospace_latin(database: &fontdb::Database, id: fontdb::ID) -> bool {
    latin_advances(database, id, ['i', 'M']).is_some_and(|[i, m]| i == m)
}

/// 这款字体里几个字的 advance，单位 em；缺字返回 `None`。
fn latin_advances<const N: usize>(
    database: &fontdb::Database,
    id: fontdb::ID,
    characters: [char; N],
) -> Option<[f32; N]> {
    database
        .with_face_data(id, |data, index| {
            let font = skrifa::FontRef::from_index(data, index).ok()?;
            let charmap = font.charmap();
            let size = skrifa::instance::Size::unscaled();
            let location = skrifa::instance::LocationRef::default();
            // 用字体单位除以 em 大小，和 cosmic-text 排版的算法一致；按 1px 字号缩放有定点误差，
            // 一行几十格就和排出来的字差开了。
            let units_per_em = f32::from(font.metrics(size, location).units_per_em);
            let metrics = font.glyph_metrics(size, location);
            let mut advances = [0.0; N];
            for (advance, character) in advances.iter_mut().zip(characters) {
                *advance = metrics.advance_width(charmap.map(character)?)? / units_per_em;
            }
            Some(advances)
        })
        .flatten()
}

fn spawn_fontconfig(args: &[&str]) -> Option<Child> {
    Command::new("fc-match")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .inspect_err(|error| tracing::warn!(%error, ?args, "fc-match could not start"))
        .ok()
}

fn finish_fontconfig_query(mut child: Child) -> Option<Vec<u8>> {
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
    Some(child.wait_with_output().ok()?.stdout)
}

/// `fc-match -s` 排好的候选（每条「家族 US 支持的语言 RS」，语言用 `|` 隔开）里支持 `language`
/// 的家族，按 fontconfig 的先后、去重。只取第一个不行：常见的配置让 sans-serif 先用一款西文字体、
/// 再接中文字体，fontconfig 的头一个答案就没有汉字，汉字回退整个落空，排版库按字重挑随便哪个
/// 有这个字的字体——常规字挑到黑体，粗体挑到只有宋体才有的 Bold（论坛 17520 #10）。
fn families_supporting(stdout: &[u8], language: &str) -> Vec<String> {
    let mut families: Vec<String> = Vec::new();
    for record in String::from_utf8_lossy(stdout).split('\u{1e}') {
        let Some((family, languages)) = record.split_once('\u{1f}') else {
            continue;
        };
        let family = family.trim();
        if !family.is_empty()
            && languages.split('|').any(|supported| supported == language)
            && !families.iter().any(|known| known == family)
        {
            families.push(family.to_owned());
        }
    }
    families
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
#[path = "font_families_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "font_list_tests.rs"]
mod font_list_tests;
