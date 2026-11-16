use super::*;
use crate::config;

/// 选了别的字体：记进设置、提示重启；选回原来的不提示。
#[test]
fn choosing_a_font_saves_it_and_asks_for_a_restart() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());

    drop(browser.select_font(FontRole::Interface, String::new()));
    assert!(!browser.restart_notice_visible);

    drop(browser.select_font(FontRole::Interface, "Fira Sans".to_owned()));
    drop(browser.select_font(FontRole::Monospace, "Fira Mono".to_owned()));

    assert_eq!(browser.user_config.interface_font, "Fira Sans");
    assert_eq!(browser.user_config.monospace_font, "Fira Mono");
    assert!(browser.restart_notice_visible);
}

/// 字体列表只读一次：读着或读完了再打开外观页不再读。
#[test]
fn installed_fonts_are_read_once() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());

    drop(browser.read_installed_fonts());
    assert!(matches!(
        browser.installed_fonts,
        InstalledFontList::Reading
    ));

    let fonts = Arc::new(InstalledFonts {
        families: vec!["Fira Sans".to_owned()],
        monospace: Vec::new(),
    });
    drop(browser.accept_installed_fonts(fonts.clone()));
    drop(browser.read_installed_fonts());
    assert!(
        matches!(&browser.installed_fonts, InstalledFontList::Read(read) if Arc::ptr_eq(read, &fonts))
    );
}

/// 选的字体存进设置文档、读回来不变（文件选择对话框读的也是这两个键）。
#[test]
fn chosen_fonts_survive_the_settings_document() {
    let mut chosen = config::default_user_config();
    chosen.interface_font = "Fira Sans".to_owned();
    chosen.monospace_font = "Fira Mono".to_owned();

    let stored = chosen.user_preferences().to_stored();
    let read = config::UserPreferences::from_stored(stored, &config::default_user_config());

    assert_eq!(read.preferences.interface_font, "Fira Sans");
    assert_eq!(read.preferences.monospace_font, "Fira Mono");
}
