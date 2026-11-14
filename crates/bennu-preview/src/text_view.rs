//! 文本预览的视图设置：字号缩放与自动换行。全局一份，主程序记住，文件选择对话框只读。

/// 100% 时的正文字号。
pub const TEXT_VIEW_BASE_FONT_SIZE: f32 = 14.0;
/// 行高系数：比界面正文宽松，长时间读日志、代码不累。
pub const TEXT_VIEW_LINE_HEIGHT: f32 = 1.5;
pub const TEXT_VIEW_MIN_ZOOM_PERCENT: u16 = 50;
pub const TEXT_VIEW_MAX_ZOOM_PERCENT: u16 = 300;
pub const TEXT_VIEW_ZOOM_STEP_PERCENT: u16 = 10;
pub const TEXT_VIEW_DEFAULT_ZOOM_PERCENT: u16 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextViewSettings {
    pub zoom_percent: u16,
    pub wrap: bool,
}

/// 用户的一次调整：Ctrl+滚轮 / Ctrl+加减 / Ctrl+0、Alt+Z、状态栏按钮。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextViewChange {
    ZoomIn,
    ZoomOut,
    ZoomReset,
    ToggleWrap,
}

impl Default for TextViewSettings {
    fn default() -> Self {
        Self {
            zoom_percent: TEXT_VIEW_DEFAULT_ZOOM_PERCENT,
            wrap: true,
        }
    }
}

impl TextViewSettings {
    /// 存储里读出的值：缩放越界时夹到范围内。
    pub fn from_stored(zoom_percent: Option<u16>, wrap: Option<bool>) -> Self {
        let defaults = Self::default();
        Self {
            zoom_percent: zoom_percent.map_or(defaults.zoom_percent, |zoom| {
                zoom.clamp(TEXT_VIEW_MIN_ZOOM_PERCENT, TEXT_VIEW_MAX_ZOOM_PERCENT)
            }),
            wrap: wrap.unwrap_or(defaults.wrap),
        }
    }

    pub fn font_size(self) -> f32 {
        TEXT_VIEW_BASE_FONT_SIZE * f32::from(self.zoom_percent) / 100.0
    }

    pub fn line_height(self) -> f32 {
        self.font_size() * TEXT_VIEW_LINE_HEIGHT
    }

    pub fn apply(self, change: TextViewChange) -> Self {
        let zoom = |zoom_percent: u16| Self {
            zoom_percent: zoom_percent
                .clamp(TEXT_VIEW_MIN_ZOOM_PERCENT, TEXT_VIEW_MAX_ZOOM_PERCENT),
            ..self
        };
        match change {
            TextViewChange::ZoomIn => zoom(
                self.zoom_percent
                    .saturating_add(TEXT_VIEW_ZOOM_STEP_PERCENT),
            ),
            TextViewChange::ZoomOut => zoom(
                self.zoom_percent
                    .saturating_sub(TEXT_VIEW_ZOOM_STEP_PERCENT),
            ),
            TextViewChange::ZoomReset => zoom(TEXT_VIEW_DEFAULT_ZOOM_PERCENT),
            TextViewChange::ToggleWrap => Self {
                wrap: !self.wrap,
                ..self
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_steps_by_ten_percent_within_bounds() {
        let mut settings = TextViewSettings::default();
        settings = settings.apply(TextViewChange::ZoomIn);
        assert_eq!(settings.zoom_percent, 110);
        assert!((settings.font_size() - 15.4).abs() < 0.001);
        for _ in 0..100 {
            settings = settings.apply(TextViewChange::ZoomIn);
        }
        assert_eq!(settings.zoom_percent, TEXT_VIEW_MAX_ZOOM_PERCENT);
        for _ in 0..100 {
            settings = settings.apply(TextViewChange::ZoomOut);
        }
        assert_eq!(settings.zoom_percent, TEXT_VIEW_MIN_ZOOM_PERCENT);
        assert_eq!(
            settings.apply(TextViewChange::ZoomReset).zoom_percent,
            TEXT_VIEW_DEFAULT_ZOOM_PERCENT
        );
    }

    #[test]
    fn wrap_toggles_and_stored_values_are_clamped() {
        let settings = TextViewSettings::default().apply(TextViewChange::ToggleWrap);
        assert!(!settings.wrap);
        assert!(settings.apply(TextViewChange::ToggleWrap).wrap);
        assert_eq!(
            TextViewSettings::from_stored(Some(900), Some(false)),
            TextViewSettings {
                zoom_percent: TEXT_VIEW_MAX_ZOOM_PERCENT,
                wrap: false
            }
        );
        assert_eq!(
            TextViewSettings::from_stored(None, None),
            TextViewSettings::default()
        );
    }
}
