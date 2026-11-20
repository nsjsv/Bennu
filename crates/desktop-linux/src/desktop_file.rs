//! Desktop Entry 格式（`.desktop`）的读取，交给 GLib 的 `GKeyFile`：转义、`;` 分隔的列表、
//! `Name[zh_CN]` 这样的本地化键都按规范处理。这里只管格式，不管某种文件里的键是什么意思。

use gio::glib::{KeyFile, KeyFileFlags};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct DesktopFileError(String);

pub struct DesktopFile(KeyFile);

impl DesktopFile {
    /// 格式不对时报错，原因用 GLib 的说法。
    pub fn parse(text: &str) -> Result<Self, DesktopFileError> {
        let file = KeyFile::new();
        // 留着翻译：界面语言可以和进程的语言不一样，查哪种语言由调用方说了算。
        file.load_from_data(text, KeyFileFlags::KEEP_TRANSLATIONS)
            .map_err(|error| DesktopFileError(error.message().to_owned()))?;
        Ok(Self(file))
    }

    pub fn has_group(&self, group: &str) -> bool {
        self.0.has_group(group)
    }

    /// 去掉转义后的值；没有这个键时为 None。
    pub fn value(&self, group: &str, key: &str) -> Option<String> {
        self.0.string(group, key).ok().map(String::from)
    }

    /// 照 KDE（KConfig）的读法取值，给 KDE 的文件（服务菜单）里写着命令的键用：`\s \t \n \r \\ \xHH`
    /// 照常转换，`\;` `\,` 原样保留；碰到别的转义（比如 Exec 里常见的 `\"`）就停止转换，
    /// 从那里起保持原样。GLib 会把这样的值整个当成错误，而这些文件是在 Dolphin 里写、在 Dolphin
    /// 里试过的。
    pub fn kde_value(&self, group: &str, key: &str) -> Option<String> {
        let raw = self.0.value(group, key).ok()?;
        Some(kconfig_unescape(raw.as_str()))
    }

    /// `locale` 是 `zh_CN` 这样的标签，`key[zh_CN]`、`key[zh]` 都找不到时退回 `key`。
    /// 不给标签时直接用 `key`：不按进程的语言猜。
    pub fn localized(&self, group: &str, key: &str, locale: Option<&str>) -> Option<String> {
        match locale {
            Some(locale) => self
                .0
                .locale_string(group, key, Some(locale))
                .ok()
                .map(String::from),
            None => self.value(group, key),
        }
    }

    /// `;` 分隔的列表，空项不要；没有这个键时是空的。
    pub fn list(&self, group: &str, key: &str) -> Vec<String> {
        self.0
            .string_list(group, key)
            .map(|items| {
                items
                    .iter()
                    .map(|item| item.to_gstr().as_str().trim().to_owned())
                    .filter(|item| !item.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// KConfig 的 `printableToString`，连它碰到无效转义时"停下、后面不截断"的结果也一样：
/// 原地改写的缓冲区里，写到哪算哪，后面留着原来的字节。
fn kconfig_unescape(raw: &str) -> String {
    if !raw.contains('\\') {
        return raw.to_owned();
    }
    let mut buffer = raw.trim().as_bytes().to_vec();
    let length = buffer.len();
    let (mut read, mut write) = (0, 0);
    while read < length {
        if buffer[read] != b'\\' {
            buffer[write] = buffer[read];
            read += 1;
            write += 1;
            continue;
        }
        read += 1;
        if read >= length {
            buffer[write] = b'\\';
            write += 1;
            break;
        }
        match buffer[read] {
            b's' => buffer[write] = b' ',
            b't' => buffer[write] = b'\t',
            b'n' => buffer[write] = b'\n',
            b'r' => buffer[write] = b'\r',
            b'\\' => buffer[write] = b'\\',
            escaped @ (b';' | b',') => {
                buffer[write] = b'\\';
                write += 1;
                buffer[write] = escaped;
            }
            b'x' if read + 2 < length => {
                let digits = std::str::from_utf8(&buffer[read + 1..read + 3]).unwrap_or("");
                buffer[write] = u8::from_str_radix(digits, 16).unwrap_or(0);
                read += 2;
            }
            b'x' => {
                buffer[write] = b'x';
                read = length - 1;
            }
            _ => {
                buffer[write] = b'\\';
                return String::from_utf8_lossy(&buffer).into_owned();
            }
        }
        read += 1;
        write += 1;
    }
    buffer.truncate(write);
    String::from_utf8_lossy(&buffer).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "[Desktop Entry]\n\
        Type=Service\n\
        MimeType=video/*;audio/mpeg;\n\
        Actions=one;two\n\
        \n\
        # 注释\n\
        [Desktop Action one]\n\
        Name=Convert\n\
        Name[zh_CN]=转换\n\
        Name[de]=Umwandeln\n\
        Exec=sh -c \"echo\\sa\\\\b\" %f\n";

    #[test]
    fn values_lists_and_escapes_follow_the_specification() {
        let file = DesktopFile::parse(SAMPLE).unwrap();
        assert!(file.has_group("Desktop Action one"));
        assert!(!file.has_group("Desktop Action two"));
        assert_eq!(
            file.list("Desktop Entry", "MimeType"),
            ["video/*", "audio/mpeg"]
        );
        assert_eq!(file.list("Desktop Entry", "Actions"), ["one", "two"]);
        assert!(file.list("Desktop Entry", "Missing").is_empty());
        assert_eq!(
            file.value("Desktop Action one", "Exec").as_deref(),
            Some("sh -c \"echo a\\b\" %f")
        );
        assert_eq!(file.value("Desktop Action one", "Icon"), None);
    }

    #[test]
    fn localized_values_fall_back_from_region_to_language_to_the_plain_key() {
        let file = DesktopFile::parse(SAMPLE).unwrap();
        let name = |locale| file.localized("Desktop Action one", "Name", locale);
        assert_eq!(name(Some("zh_CN")).as_deref(), Some("转换"));
        assert_eq!(name(Some("de_AT")).as_deref(), Some("Umwandeln"));
        assert_eq!(name(Some("fr_FR")).as_deref(), Some("Convert"));
        assert_eq!(name(None).as_deref(), Some("Convert"));
    }

    #[test]
    fn values_with_escapes_glib_rejects_are_read_the_way_kde_reads_them() {
        let file = DesktopFile::parse(
            "[Desktop Entry]\n\
             Quoted=sh -c \"echo \\\"hi\\\"\"\n\
             Spaced=a\\sb \\\\ c\n\
             Find=find . -exec rm {} \\;\n\
             Mixed=a\\sb \\\"c\\\" d\n",
        )
        .unwrap();
        assert_eq!(file.value("Desktop Entry", "Quoted"), None);
        assert_eq!(
            file.kde_value("Desktop Entry", "Quoted").as_deref(),
            Some(r#"sh -c "echo \"hi\"""#)
        );
        assert_eq!(
            file.kde_value("Desktop Entry", "Spaced").as_deref(),
            Some(r"a b \ c")
        );
        assert_eq!(
            file.kde_value("Desktop Entry", "Find").as_deref(),
            Some(r"find . -exec rm {} \;")
        );
        // 一个有效转义之后再碰到无效的：KConfig 写到哪算哪，后面是原来的字节。
        assert_eq!(
            file.kde_value("Desktop Entry", "Mixed").as_deref(),
            Some(r#"a b \\"c\" d"#)
        );
    }

    #[test]
    fn a_file_that_is_not_a_key_file_is_rejected() {
        assert!(DesktopFile::parse("this is not\na desktop file").is_err());
    }
}
