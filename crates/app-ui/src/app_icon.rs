//! 应用窗口图标：嵌入深浅两套品牌图标，启动时按存储的主题偏好选择一套。

use std::sync::LazyLock;

use iced::window;
use iced::widget::image::Handle;

use crate::config::default_state_database_path;
use crate::matugen_theme::ThemeMode;

const DARK_ICON_PNG: &[u8] = include_bytes!("../assets/app-icon-dark.png");
const LIGHT_ICON_PNG: &[u8] = include_bytes!("../assets/app-icon-light.png");
/// 界面展示（关于页半页大图标）用高清源；packaging hicolor 的 512px 是
/// 仓库里最大的品牌位图，放大场景必须用它而不是 128px 窗口图标。
const DISPLAY_ICON_PNG: &[u8] =
    include_bytes!("../../../packaging/linux/icons/hicolor/512x512/apps/bennu.png");

/// 启动期窗口图标。与 stored_launch_window_policy 一样独立于完整配置加载路径：
/// 在主实例声明 D-Bus 名之前调用，任何读取或解码失败都回退到无图标，绝不阻塞窗口创建。
pub(crate) fn startup_window_icon() -> Option<window::Icon> {
    static ICON: LazyLock<Option<window::Icon>> =
        LazyLock::new(|| decode_window_icon(preferred_icon_png()));
    ICON.clone()
}

fn preferred_icon_png() -> &'static [u8] {
    icon_png_for_mode(stored_theme_mode())
}

fn icon_png_for_mode(theme_mode: ThemeMode) -> &'static [u8] {
    match theme_mode {
        ThemeMode::Light => LIGHT_ICON_PNG,
        ThemeMode::Dark => DARK_ICON_PNG,
        // automatic 表示跟随系统深浅。dark_light 的门户查询依赖 Tokio reactor 上下文，
        // 同步线程（启动极早期、测试）没有 reactor，此时回退浅色，绝不让图标选择 panic。
        ThemeMode::Automatic => automatic_mode_png(),
    }
}

/// 界面内展示用的品牌图标 Handle（如关于页）。iced 以 Handle 的 id 作图片
/// 缓存键，from_bytes 每次 Id::unique()；视图每帧重建若现场造 Handle 会
/// 让缓存永远 miss、纹理反复异步上传导致闪烁，因此这里用静态句柄保证 id 稳定。
pub(crate) fn display_icon_handle() -> Handle {
    static DISPLAY_HANDLE: LazyLock<Handle> = LazyLock::new(|| Handle::from_bytes(DISPLAY_ICON_PNG));
    DISPLAY_HANDLE.clone()
}

fn automatic_mode_png() -> &'static [u8] {
    if tokio::runtime::Handle::try_current().is_err() {
        return LIGHT_ICON_PNG;
    }
    match dark_light::detect() {
        Ok(dark_light::Mode::Dark) => DARK_ICON_PNG,
        Ok(dark_light::Mode::Light) | Ok(dark_light::Mode::Unspecified) | Err(_) => LIGHT_ICON_PNG,
    }
}

fn stored_theme_mode() -> ThemeMode {
    let state_database_path = default_state_database_path();
    let Ok(store) = file_operation_store::TaskQueueStore::new(&state_database_path) else {
        return ThemeMode::Automatic;
    };
    store
        .read_user_preferences()
        .ok()
        .flatten()
        .and_then(|stored| ThemeMode::from_config_value(&stored.theme_mode))
        .unwrap_or(ThemeMode::Automatic)
}

fn decode_window_icon(png: &[u8]) -> Option<window::Icon> {
    let decoded = image::load_from_memory(png).ok()?.to_rgba8();
    let (width, height) = decoded.dimensions();
    window::icon::from_rgba(decoded.into_raw(), width, height).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_icons_decode_to_expected_square_dimensions() {
        for png in [DARK_ICON_PNG, LIGHT_ICON_PNG] {
            let decoded = image::load_from_memory(png).expect("embedded icon must decode");
            assert_eq!((decoded.width(), decoded.height()), (128, 128));
        }
    }

    #[test]
    fn explicit_theme_modes_select_matching_embedded_variant() {
        assert_eq!(icon_png_for_mode(ThemeMode::Light), LIGHT_ICON_PNG);
        assert_eq!(icon_png_for_mode(ThemeMode::Dark), DARK_ICON_PNG);
    }

    #[test]
    fn automatic_mode_without_reactor_falls_back_to_light() {
        // 同步测试线程没有 Tokio reactor，automatic 必须优雅回退而不是 panic。
        assert_eq!(automatic_mode_png(), LIGHT_ICON_PNG);
    }
}
