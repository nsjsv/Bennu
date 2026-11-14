//! 包里存的名字 → 显示用的名字。中文 Windows 打的 zip 多半按 GBK 存名字又不标 UTF-8，7z 在
//! Linux 上原样吐出这些字节（`-mcp` 不起作用）。合法 UTF-8 的名字原样用；不是的把同一个包里
//! 所有这样的名字拼起来一起识别编码——单个名字只有几个字，谁也猜不准。

use encoding_rs::UTF_8;

use crate::text_encoding::sniff_text_encoding;

pub(crate) fn decode_member_names<'a>(
    stored_paths: impl Iterator<Item = &'a [u8]> + Clone,
) -> Vec<String> {
    let mut undecoded = stored_paths
        .clone()
        .filter(|stored| std::str::from_utf8(stored).is_err())
        .peekable();
    let encoding = match undecoded.peek() {
        None => UTF_8,
        Some(_) => {
            let sample = undecoded.collect::<Vec<_>>().join(&b'\n');
            sniff_text_encoding(&sample, true).map_or(UTF_8, |sniffed| sniffed.encoding)
        }
    };
    // 解不了的字节变成替换字符，名字至少能看、能点开。
    stored_paths
        .map(|stored| match std::str::from_utf8(stored) {
            Ok(text) => text.to_owned(),
            Err(_) => encoding.decode_without_bom_handling(stored).0.into_owned(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(paths: &[&[u8]]) -> Vec<String> {
        decode_member_names(paths.iter().copied())
    }

    #[test]
    fn utf8_names_stay_and_gbk_names_are_recognised_together() {
        let names = [
            "文档/",
            "文档/年度报告.txt",
            "照片/毕业合影.jpg",
            "说明.txt",
        ];
        let gbk = names
            .iter()
            .map(|name| encoding_rs::GBK.encode(name).0.into_owned())
            .collect::<Vec<_>>();
        let mut stored = vec![&b"readme.md"[..], "已是UTF-8.txt".as_bytes()];
        stored.extend(gbk.iter().map(Vec::as_slice));

        let decoded = decode(&stored);
        assert_eq!(decoded[0], "readme.md");
        assert_eq!(decoded[1], "已是UTF-8.txt");
        assert_eq!(decoded[2..], names.map(str::to_owned));
    }
}
