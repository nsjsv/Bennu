use bennu_localization::tr;

/// 设置里能选的配色。每套只有一深一浅，跟着深浅模式走，没有子版本。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSchemePreset {
    Default,
    Claude,
    Catppuccin,
    Everforest,
    Gruvbox,
    Nord,
    TokyoNight,
    Matugen,
}

impl ColorSchemePreset {
    /// 设置页的顺序：默认在前，出名配色按字母，壁纸取色最后。
    pub const ALL: [Self; 8] = [
        Self::Default,
        Self::Claude,
        Self::Catppuccin,
        Self::Everforest,
        Self::Gruvbox,
        Self::Nord,
        Self::TokyoNight,
        Self::Matugen,
    ];

    fn from_config_value(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|preset| preset.config_value() == value)
    }

    /// 读存储的配色值：除了现在的值，也认 2026-11-10 删掉的配色——变体读成同一家的
    /// 经典版，整家删掉的和“自定义”读成默认。它们不算“读不懂”，不提示用户，
    /// 下次保存时写成新值。
    pub fn from_stored_value(value: &str) -> Option<Self> {
        Self::from_config_value(value).or(match value {
            "catppuccin-frappe" | "catppuccin-macchiato" => Some(Self::Catppuccin),
            "everforest-hard" | "everforest-soft" => Some(Self::Everforest),
            "gruvbox-hard" | "gruvbox-soft" => Some(Self::Gruvbox),
            "tokyo-night-storm" | "tokyo-night-moon" => Some(Self::TokyoNight),
            "dracula"
            | "github"
            | "github-dimmed"
            | "github-high-contrast"
            | "github-colorblind"
            | "github-tritanopia"
            | "kanagawa"
            | "kanagawa-dragon"
            | "one"
            | "rose-pine"
            | "rose-pine-moon"
            | "solarized"
            | "custom" => Some(Self::Default),
            _ => None,
        })
    }

    pub const fn config_value(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Claude => "claude",
            Self::Catppuccin => "catppuccin",
            Self::Everforest => "everforest",
            Self::Gruvbox => "gruvbox",
            Self::Nord => "nord",
            Self::TokyoNight => "tokyo-night",
            Self::Matugen => "matugen",
        }
    }

    /// 主题的固定名，不随语言变（也用作 iced 主题名）。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Claude => "Claude",
            Self::Catppuccin => "Catppuccin",
            Self::Everforest => "Everforest",
            Self::Gruvbox => "Gruvbox",
            Self::Nord => "Nord",
            Self::TokyoNight => "Tokyo Night",
            Self::Matugen => "Matugen",
        }
    }

    /// 设置页里的名字：配色专名照原样，“默认”翻译。
    pub fn display_label(self) -> String {
        match self {
            Self::Default => tr!("Default"),
            preset => preset.label().to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_values_are_stable_and_round_trip() {
        assert_eq!(
            ColorSchemePreset::ALL.map(ColorSchemePreset::config_value),
            [
                "default",
                "claude",
                "catppuccin",
                "everforest",
                "gruvbox",
                "nord",
                "tokyo-night",
                "matugen",
            ]
        );
        for preset in ColorSchemePreset::ALL {
            assert_eq!(
                ColorSchemePreset::from_stored_value(preset.config_value()),
                Some(preset)
            );
        }
    }

    #[test]
    fn retired_values_map_to_the_classic_variant_or_the_default_scheme() {
        for (stored, expected) in [
            ("catppuccin-frappe", ColorSchemePreset::Catppuccin),
            ("catppuccin-macchiato", ColorSchemePreset::Catppuccin),
            ("everforest-hard", ColorSchemePreset::Everforest),
            ("gruvbox-soft", ColorSchemePreset::Gruvbox),
            ("tokyo-night-storm", ColorSchemePreset::TokyoNight),
            ("tokyo-night-moon", ColorSchemePreset::TokyoNight),
            ("dracula", ColorSchemePreset::Default),
            ("github-dimmed", ColorSchemePreset::Default),
            ("rose-pine", ColorSchemePreset::Default),
            ("custom", ColorSchemePreset::Default),
        ] {
            assert_eq!(
                ColorSchemePreset::from_stored_value(stored),
                Some(expected),
                "{stored}"
            );
            // 只有读存储的边界认退役值；新写入的值只能是现在的 8 个。
            assert_eq!(ColorSchemePreset::from_config_value(stored), None);
        }
        assert_eq!(ColorSchemePreset::from_stored_value("sepia"), None);
    }
}
