//! Matugen 生成文件（schema v2）：深浅两套，每套只有底色、文字色、强调色，和其它配色
//! 走同一条层次计算。模板见 `packaging/matugen/bennu-colors.toml`。

use std::path::Path;

use toml::Table;

use super::scheme::SchemeColors;
use super::{parse_hex_color, AppearanceMode};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MatugenColors {
    light: SchemeColors,
    dark: SchemeColors,
}

impl MatugenColors {
    pub(super) fn for_mode(&self, mode: AppearanceMode) -> SchemeColors {
        match mode {
            AppearanceMode::Light => self.light,
            AppearanceMode::Dark => self.dark,
        }
    }
}

pub fn parse_matugen_colors(document: &str) -> Result<MatugenColors, String> {
    let document = document.parse::<Table>().map_err(|error| {
        error
            .span()
            .map(|span| format!("TOML syntax error at bytes {}..{}", span.start, span.end))
            .unwrap_or_else(|| "TOML syntax error".to_owned())
    })?;
    match document.get("version").and_then(toml::Value::as_integer) {
        Some(2) => {}
        // v1 每个文件只有一种深浅、31 个角色；新模板一次生成深浅两套。
        Some(1) => {
            return Err(
                "old format (version 1): regenerate it with the current Bennu template".to_owned(),
            )
        }
        _ => return Err("version must be 2".to_owned()),
    }
    Ok(MatugenColors {
        light: scheme_colors(&document, "light")?,
        dark: scheme_colors(&document, "dark")?,
    })
}

fn scheme_colors(document: &Table, mode: &str) -> Result<SchemeColors, String> {
    let table = document
        .get(mode)
        .and_then(toml::Value::as_table)
        .ok_or_else(|| format!("{mode} must be a table"))?;
    let color = |key: &str| {
        let field = format!("{mode}.{key}");
        let value = table
            .get(key)
            .and_then(toml::Value::as_str)
            .ok_or_else(|| format!("{field} must be a string"))?;
        parse_hex_color(value, &field)
    };
    Ok(SchemeColors {
        background: color("background")?,
        text: color("text")?,
        accent: color("accent")?,
    })
}

/// 文件不存在 = `Ok(None)`（没用 Matugen）；读到但格式不对 = `Err`。
pub async fn read_matugen_colors_file(path: &Path) -> Result<Option<MatugenColors>, String> {
    let document = match tokio::fs::read_to_string(path).await {
        Ok(document) => document,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "could not read matugen colors {}: {error}",
                path.display()
            ));
        }
    };
    parse_matugen_colors(&document)
        .map(Some)
        .map_err(|error| format!("invalid matugen colors {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheme::rgb;

    const GENERATED: &str = include_str!("../test-data/matugen.toml");

    #[test]
    fn generated_file_gives_one_set_of_three_colors_per_mode() {
        let colors = parse_matugen_colors(GENERATED).expect("generated matugen colors");
        assert_eq!(
            colors.for_mode(AppearanceMode::Light),
            SchemeColors {
                background: rgb(0xfdf7ff),
                text: rgb(0x1d1b20),
                accent: rgb(0x65558f),
            }
        );
        assert_eq!(
            colors.for_mode(AppearanceMode::Dark),
            SchemeColors {
                background: rgb(0x141218),
                text: rgb(0xe6e0e9),
                accent: rgb(0xcfbdfe),
            }
        );
    }

    #[test]
    fn malformed_and_old_format_documents_are_rejected_at_the_boundary() {
        let old_format = "version = 1\nmode = \"light\"\n[colors]\nbackground = \"#fdf7ff\"\n";
        assert!(parse_matugen_colors(old_format)
            .unwrap_err()
            .contains("regenerate"));
        for malformed in [
            GENERATED.replacen("version = 2", "version = 3", 1),
            GENERATED.replacen("background = \"#fdf7ff\"", "background = \"#fff\"", 1),
            GENERATED.replacen("[dark]", "[night]", 1),
            GENERATED.replacen("accent = ", "missing_accent = ", 1),
        ] {
            assert!(parse_matugen_colors(&malformed).is_err());
        }
    }

    #[tokio::test]
    async fn missing_file_is_distinct_from_an_invalid_document() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("matugen.toml");

        assert!(read_matugen_colors_file(&path)
            .await
            .expect("missing file is optional")
            .is_none());

        tokio::fs::write(&path, "version = 2\n[light]\nbackground = ???")
            .await
            .expect("write invalid colors");
        assert!(read_matugen_colors_file(&path).await.is_err());
    }
}
