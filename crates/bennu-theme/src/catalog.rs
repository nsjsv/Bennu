use super::AppearanceMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSchemePreset {
    Default,
    Claude,
    Catppuccin,
    CatppuccinFrappe,
    CatppuccinMacchiato,
    Dracula,
    Everforest,
    EverforestHard,
    EverforestSoft,
    GitHub,
    GitHubDimmed,
    GitHubHighContrast,
    GitHubColorblind,
    GitHubTritanopia,
    Gruvbox,
    GruvboxHard,
    GruvboxSoft,
    Kanagawa,
    KanagawaDragon,
    Nord,
    One,
    RosePine,
    RosePineMoon,
    Solarized,
    TokyoNight,
    TokyoNightStorm,
    TokyoNightMoon,
    Matugen,
    Custom,
}

impl ColorSchemePreset {
    pub const ALL: [Self; 29] = [
        Self::Default,
        Self::Claude,
        Self::Catppuccin,
        Self::CatppuccinFrappe,
        Self::CatppuccinMacchiato,
        Self::Dracula,
        Self::EverforestHard,
        Self::Everforest,
        Self::EverforestSoft,
        Self::GitHub,
        Self::GitHubDimmed,
        Self::GitHubHighContrast,
        Self::GitHubColorblind,
        Self::GitHubTritanopia,
        Self::GruvboxHard,
        Self::Gruvbox,
        Self::GruvboxSoft,
        Self::Kanagawa,
        Self::KanagawaDragon,
        Self::Nord,
        Self::One,
        Self::RosePine,
        Self::RosePineMoon,
        Self::Solarized,
        Self::TokyoNight,
        Self::TokyoNightStorm,
        Self::TokyoNightMoon,
        Self::Matugen,
        Self::Custom,
    ];

    pub fn from_config_value(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|preset| preset.config_value() == value)
    }

    pub const fn config_value(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Claude => "claude",
            Self::Catppuccin => "catppuccin",
            Self::CatppuccinFrappe => "catppuccin-frappe",
            Self::CatppuccinMacchiato => "catppuccin-macchiato",
            Self::Dracula => "dracula",
            Self::Everforest => "everforest",
            Self::EverforestHard => "everforest-hard",
            Self::EverforestSoft => "everforest-soft",
            Self::GitHub => "github",
            Self::GitHubDimmed => "github-dimmed",
            Self::GitHubHighContrast => "github-high-contrast",
            Self::GitHubColorblind => "github-colorblind",
            Self::GitHubTritanopia => "github-tritanopia",
            Self::Gruvbox => "gruvbox",
            Self::GruvboxHard => "gruvbox-hard",
            Self::GruvboxSoft => "gruvbox-soft",
            Self::Kanagawa => "kanagawa",
            Self::KanagawaDragon => "kanagawa-dragon",
            Self::Nord => "nord",
            Self::One => "one",
            Self::RosePine => "rose-pine",
            Self::RosePineMoon => "rose-pine-moon",
            Self::Solarized => "solarized",
            Self::TokyoNight => "tokyo-night",
            Self::TokyoNightStorm => "tokyo-night-storm",
            Self::TokyoNightMoon => "tokyo-night-moon",
            Self::Matugen => "matugen",
            Self::Custom => "custom",
        }
    }

    pub const fn label(self) -> &'static str {
        self.family().label()
    }

    pub const fn family(self) -> ColorSchemeFamily {
        match self {
            Self::Default => ColorSchemeFamily::Default,
            Self::Claude => ColorSchemeFamily::Claude,
            Self::Catppuccin | Self::CatppuccinFrappe | Self::CatppuccinMacchiato => {
                ColorSchemeFamily::Catppuccin
            }
            Self::Dracula => ColorSchemeFamily::Dracula,
            Self::Everforest | Self::EverforestHard | Self::EverforestSoft => {
                ColorSchemeFamily::Everforest
            }
            Self::GitHub
            | Self::GitHubDimmed
            | Self::GitHubHighContrast
            | Self::GitHubColorblind
            | Self::GitHubTritanopia => ColorSchemeFamily::GitHub,
            Self::Gruvbox | Self::GruvboxHard | Self::GruvboxSoft => ColorSchemeFamily::Gruvbox,
            Self::Kanagawa | Self::KanagawaDragon => ColorSchemeFamily::Kanagawa,
            Self::Nord => ColorSchemeFamily::Nord,
            Self::One => ColorSchemeFamily::One,
            Self::RosePine | Self::RosePineMoon => ColorSchemeFamily::RosePine,
            Self::Solarized => ColorSchemeFamily::Solarized,
            Self::TokyoNight | Self::TokyoNightStorm | Self::TokyoNightMoon => {
                ColorSchemeFamily::TokyoNight
            }
            Self::Matugen => ColorSchemeFamily::Matugen,
            Self::Custom => ColorSchemeFamily::Custom,
        }
    }

    pub const fn effective_for_mode(self, mode: AppearanceMode) -> Self {
        match (self, mode) {
            (Self::CatppuccinFrappe | Self::CatppuccinMacchiato, AppearanceMode::Light) => {
                Self::Catppuccin
            }
            (Self::GitHubDimmed, AppearanceMode::Light) => Self::GitHub,
            (Self::KanagawaDragon, AppearanceMode::Light) => Self::Kanagawa,
            (Self::RosePineMoon, AppearanceMode::Light) => Self::RosePine,
            (Self::TokyoNightStorm | Self::TokyoNightMoon, AppearanceMode::Light) => {
                Self::TokyoNight
            }
            _ => self,
        }
    }

    pub const fn style_label(self, mode: AppearanceMode) -> &'static str {
        match self {
            Self::Catppuccin => match mode {
                AppearanceMode::Light => "Latte",
                AppearanceMode::Dark => "Mocha",
            },
            Self::CatppuccinFrappe => "Frappé",
            Self::CatppuccinMacchiato => "Macchiato",
            Self::EverforestHard | Self::GruvboxHard => "Hard",
            Self::Everforest | Self::Gruvbox => "Medium",
            Self::EverforestSoft | Self::GruvboxSoft => "Soft",
            Self::GitHub => "Default",
            Self::GitHubDimmed => "Dimmed",
            Self::GitHubHighContrast => "High contrast",
            Self::GitHubColorblind => "Colorblind",
            Self::GitHubTritanopia => "Tritanopia",
            Self::Dracula => match mode {
                AppearanceMode::Light => "Alucard",
                AppearanceMode::Dark => "Dracula",
            },
            Self::Kanagawa => match mode {
                AppearanceMode::Light => "Lotus",
                AppearanceMode::Dark => "Wave",
            },
            Self::One => match mode {
                AppearanceMode::Light => "One Light",
                AppearanceMode::Dark => "One Dark",
            },
            Self::RosePine => match mode {
                AppearanceMode::Light => "Dawn",
                AppearanceMode::Dark => "Main",
            },
            Self::Solarized => match mode {
                AppearanceMode::Light => "Solarized Light",
                AppearanceMode::Dark => "Solarized Dark",
            },
            Self::TokyoNight => match mode {
                AppearanceMode::Light => "Day",
                AppearanceMode::Dark => "Night",
            },
            Self::KanagawaDragon => "Dragon",
            Self::RosePineMoon => "Moon",
            Self::TokyoNightStorm => "Storm",
            Self::TokyoNightMoon => "Moon",
            Self::Custom => "Custom",
            _ => self.family().label(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSchemeFamily {
    Default,
    Claude,
    Catppuccin,
    Dracula,
    Everforest,
    GitHub,
    Gruvbox,
    Kanagawa,
    Nord,
    One,
    RosePine,
    Solarized,
    TokyoNight,
    Matugen,
    Custom,
}

impl ColorSchemeFamily {
    pub const ALL: [Self; 15] = [
        Self::Default,
        Self::Claude,
        Self::Catppuccin,
        Self::Dracula,
        Self::Everforest,
        Self::GitHub,
        Self::Gruvbox,
        Self::Kanagawa,
        Self::Nord,
        Self::One,
        Self::RosePine,
        Self::Solarized,
        Self::TokyoNight,
        Self::Matugen,
        Self::Custom,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Claude => "Claude",
            Self::Catppuccin => "Catppuccin",
            Self::Dracula => "Dracula",
            Self::Everforest => "Everforest",
            Self::GitHub => "GitHub",
            Self::Gruvbox => "Gruvbox",
            Self::Kanagawa => "Kanagawa",
            Self::Nord => "Nord",
            Self::One => "One",
            Self::RosePine => "Rosé Pine",
            Self::Solarized => "Solarized",
            Self::TokyoNight => "Tokyo Night",
            Self::Matugen => "Matugen",
            Self::Custom => "Custom",
        }
    }

    pub const fn default_preset(self) -> ColorSchemePreset {
        match self {
            Self::Default => ColorSchemePreset::Default,
            Self::Claude => ColorSchemePreset::Claude,
            Self::Catppuccin => ColorSchemePreset::Catppuccin,
            Self::Dracula => ColorSchemePreset::Dracula,
            Self::Everforest => ColorSchemePreset::Everforest,
            Self::GitHub => ColorSchemePreset::GitHub,
            Self::Gruvbox => ColorSchemePreset::Gruvbox,
            Self::Kanagawa => ColorSchemePreset::Kanagawa,
            Self::Nord => ColorSchemePreset::Nord,
            Self::One => ColorSchemePreset::One,
            Self::RosePine => ColorSchemePreset::RosePine,
            Self::Solarized => ColorSchemePreset::Solarized,
            Self::TokyoNight => ColorSchemePreset::TokyoNight,
            Self::Matugen => ColorSchemePreset::Matugen,
            Self::Custom => ColorSchemePreset::Custom,
        }
    }

    pub const fn styles(self, mode: AppearanceMode) -> &'static [ColorSchemePreset] {
        match (self, mode) {
            (Self::Catppuccin, AppearanceMode::Dark) => &[
                ColorSchemePreset::CatppuccinFrappe,
                ColorSchemePreset::CatppuccinMacchiato,
                ColorSchemePreset::Catppuccin,
            ],
            (Self::Everforest, _) => &[
                ColorSchemePreset::EverforestHard,
                ColorSchemePreset::Everforest,
                ColorSchemePreset::EverforestSoft,
            ],
            (Self::GitHub, AppearanceMode::Light) => &[
                ColorSchemePreset::GitHub,
                ColorSchemePreset::GitHubHighContrast,
                ColorSchemePreset::GitHubColorblind,
                ColorSchemePreset::GitHubTritanopia,
            ],
            (Self::GitHub, AppearanceMode::Dark) => &[
                ColorSchemePreset::GitHub,
                ColorSchemePreset::GitHubDimmed,
                ColorSchemePreset::GitHubHighContrast,
                ColorSchemePreset::GitHubColorblind,
                ColorSchemePreset::GitHubTritanopia,
            ],
            (Self::Gruvbox, _) => &[
                ColorSchemePreset::GruvboxHard,
                ColorSchemePreset::Gruvbox,
                ColorSchemePreset::GruvboxSoft,
            ],
            (Self::Kanagawa, AppearanceMode::Dark) => &[
                ColorSchemePreset::Kanagawa,
                ColorSchemePreset::KanagawaDragon,
            ],
            (Self::RosePine, AppearanceMode::Dark) => {
                &[ColorSchemePreset::RosePine, ColorSchemePreset::RosePineMoon]
            }
            (Self::TokyoNight, AppearanceMode::Dark) => &[
                ColorSchemePreset::TokyoNight,
                ColorSchemePreset::TokyoNightStorm,
                ColorSchemePreset::TokyoNightMoon,
            ],
            (Self::Default, _) => &[ColorSchemePreset::Default],
            (Self::Claude, _) => &[ColorSchemePreset::Claude],
            (Self::Catppuccin, AppearanceMode::Light) => &[ColorSchemePreset::Catppuccin],
            (Self::Dracula, _) => &[ColorSchemePreset::Dracula],
            (Self::Kanagawa, AppearanceMode::Light) => &[ColorSchemePreset::Kanagawa],
            (Self::Nord, _) => &[ColorSchemePreset::Nord],
            (Self::One, _) => &[ColorSchemePreset::One],
            (Self::RosePine, AppearanceMode::Light) => &[ColorSchemePreset::RosePine],
            (Self::Solarized, _) => &[ColorSchemePreset::Solarized],
            (Self::TokyoNight, AppearanceMode::Light) => &[ColorSchemePreset::TokyoNight],
            (Self::Matugen, _) => &[ColorSchemePreset::Matugen],
            (Self::Custom, _) => &[ColorSchemePreset::Custom],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_expose_the_mode_specific_style_matrix() {
        let cases: &[(ColorSchemeFamily, AppearanceMode, &[ColorSchemePreset])] = &[
            (
                ColorSchemeFamily::Default,
                AppearanceMode::Light,
                &[ColorSchemePreset::Default],
            ),
            (
                ColorSchemeFamily::Default,
                AppearanceMode::Dark,
                &[ColorSchemePreset::Default],
            ),
            (
                ColorSchemeFamily::Claude,
                AppearanceMode::Light,
                &[ColorSchemePreset::Claude],
            ),
            (
                ColorSchemeFamily::Claude,
                AppearanceMode::Dark,
                &[ColorSchemePreset::Claude],
            ),
            (
                ColorSchemeFamily::Catppuccin,
                AppearanceMode::Light,
                &[ColorSchemePreset::Catppuccin],
            ),
            (
                ColorSchemeFamily::Catppuccin,
                AppearanceMode::Dark,
                &[
                    ColorSchemePreset::CatppuccinFrappe,
                    ColorSchemePreset::CatppuccinMacchiato,
                    ColorSchemePreset::Catppuccin,
                ],
            ),
            (
                ColorSchemeFamily::Dracula,
                AppearanceMode::Light,
                &[ColorSchemePreset::Dracula],
            ),
            (
                ColorSchemeFamily::Dracula,
                AppearanceMode::Dark,
                &[ColorSchemePreset::Dracula],
            ),
            (
                ColorSchemeFamily::Everforest,
                AppearanceMode::Light,
                &[
                    ColorSchemePreset::EverforestHard,
                    ColorSchemePreset::Everforest,
                    ColorSchemePreset::EverforestSoft,
                ],
            ),
            (
                ColorSchemeFamily::Everforest,
                AppearanceMode::Dark,
                &[
                    ColorSchemePreset::EverforestHard,
                    ColorSchemePreset::Everforest,
                    ColorSchemePreset::EverforestSoft,
                ],
            ),
            (
                ColorSchemeFamily::GitHub,
                AppearanceMode::Light,
                &[
                    ColorSchemePreset::GitHub,
                    ColorSchemePreset::GitHubHighContrast,
                    ColorSchemePreset::GitHubColorblind,
                    ColorSchemePreset::GitHubTritanopia,
                ],
            ),
            (
                ColorSchemeFamily::GitHub,
                AppearanceMode::Dark,
                &[
                    ColorSchemePreset::GitHub,
                    ColorSchemePreset::GitHubDimmed,
                    ColorSchemePreset::GitHubHighContrast,
                    ColorSchemePreset::GitHubColorblind,
                    ColorSchemePreset::GitHubTritanopia,
                ],
            ),
            (
                ColorSchemeFamily::Gruvbox,
                AppearanceMode::Light,
                &[
                    ColorSchemePreset::GruvboxHard,
                    ColorSchemePreset::Gruvbox,
                    ColorSchemePreset::GruvboxSoft,
                ],
            ),
            (
                ColorSchemeFamily::Gruvbox,
                AppearanceMode::Dark,
                &[
                    ColorSchemePreset::GruvboxHard,
                    ColorSchemePreset::Gruvbox,
                    ColorSchemePreset::GruvboxSoft,
                ],
            ),
            (
                ColorSchemeFamily::Kanagawa,
                AppearanceMode::Light,
                &[ColorSchemePreset::Kanagawa],
            ),
            (
                ColorSchemeFamily::Kanagawa,
                AppearanceMode::Dark,
                &[
                    ColorSchemePreset::Kanagawa,
                    ColorSchemePreset::KanagawaDragon,
                ],
            ),
            (
                ColorSchemeFamily::Nord,
                AppearanceMode::Light,
                &[ColorSchemePreset::Nord],
            ),
            (
                ColorSchemeFamily::Nord,
                AppearanceMode::Dark,
                &[ColorSchemePreset::Nord],
            ),
            (
                ColorSchemeFamily::One,
                AppearanceMode::Light,
                &[ColorSchemePreset::One],
            ),
            (
                ColorSchemeFamily::One,
                AppearanceMode::Dark,
                &[ColorSchemePreset::One],
            ),
            (
                ColorSchemeFamily::RosePine,
                AppearanceMode::Light,
                &[ColorSchemePreset::RosePine],
            ),
            (
                ColorSchemeFamily::RosePine,
                AppearanceMode::Dark,
                &[ColorSchemePreset::RosePine, ColorSchemePreset::RosePineMoon],
            ),
            (
                ColorSchemeFamily::Solarized,
                AppearanceMode::Light,
                &[ColorSchemePreset::Solarized],
            ),
            (
                ColorSchemeFamily::Solarized,
                AppearanceMode::Dark,
                &[ColorSchemePreset::Solarized],
            ),
            (
                ColorSchemeFamily::TokyoNight,
                AppearanceMode::Light,
                &[ColorSchemePreset::TokyoNight],
            ),
            (
                ColorSchemeFamily::TokyoNight,
                AppearanceMode::Dark,
                &[
                    ColorSchemePreset::TokyoNight,
                    ColorSchemePreset::TokyoNightStorm,
                    ColorSchemePreset::TokyoNightMoon,
                ],
            ),
            (
                ColorSchemeFamily::Matugen,
                AppearanceMode::Light,
                &[ColorSchemePreset::Matugen],
            ),
            (
                ColorSchemeFamily::Matugen,
                AppearanceMode::Dark,
                &[ColorSchemePreset::Matugen],
            ),
        ];

        for (family, mode, expected) in cases {
            assert_eq!(family.styles(*mode), *expected);
        }
    }

    #[test]
    fn presets_resolve_to_a_style_available_in_the_effective_mode() {
        let expected_light = [
            ColorSchemePreset::Default,
            ColorSchemePreset::Claude,
            ColorSchemePreset::Catppuccin,
            ColorSchemePreset::Catppuccin,
            ColorSchemePreset::Catppuccin,
            ColorSchemePreset::Dracula,
            ColorSchemePreset::EverforestHard,
            ColorSchemePreset::Everforest,
            ColorSchemePreset::EverforestSoft,
            ColorSchemePreset::GitHub,
            ColorSchemePreset::GitHub,
            ColorSchemePreset::GitHubHighContrast,
            ColorSchemePreset::GitHubColorblind,
            ColorSchemePreset::GitHubTritanopia,
            ColorSchemePreset::GruvboxHard,
            ColorSchemePreset::Gruvbox,
            ColorSchemePreset::GruvboxSoft,
            ColorSchemePreset::Kanagawa,
            ColorSchemePreset::Kanagawa,
            ColorSchemePreset::Nord,
            ColorSchemePreset::One,
            ColorSchemePreset::RosePine,
            ColorSchemePreset::RosePine,
            ColorSchemePreset::Solarized,
            ColorSchemePreset::TokyoNight,
            ColorSchemePreset::TokyoNight,
            ColorSchemePreset::TokyoNight,
            ColorSchemePreset::Matugen,
        ];

        for (preset, expected) in ColorSchemePreset::ALL.into_iter().zip(expected_light) {
            assert_eq!(preset.effective_for_mode(AppearanceMode::Light), expected);
            assert_eq!(preset.effective_for_mode(AppearanceMode::Dark), preset);
            assert!(preset
                .family()
                .styles(AppearanceMode::Light)
                .contains(&expected));
            assert!(preset
                .family()
                .styles(AppearanceMode::Dark)
                .contains(&preset));
        }
        for family in ColorSchemeFamily::ALL {
            assert_eq!(family.default_preset().family(), family);
        }
    }
}
