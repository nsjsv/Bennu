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

/// 选了正文字号马上生效（不提示重启），记进设置文档、读回来不变；旧文档没有这个键用默认字号。
#[test]
fn choosing_a_text_size_scales_the_interface_at_once_and_is_saved() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    assert_eq!(browser.interface_scale(), 1.0);

    drop(browser.select_text_size(18));

    assert_eq!(browser.interface_scale(), 18.0 / 13.0);
    assert!(!browser.restart_notice_visible);
    let stored = browser.user_config.user_preferences().to_stored();
    let read = config::UserPreferences::from_stored(stored, &config::default_user_config());
    assert_eq!(read.preferences.interface_text_size, 18);

    let mut old_document = config::default_user_config().user_preferences().to_stored();
    old_document.interface_text_size = None;
    let read = config::UserPreferences::from_stored(old_document, &config::default_user_config());
    assert_eq!(
        read.preferences.interface_text_size,
        bennu_theme::typography::DEFAULT_TEXT_SIZE
    );
}

/// 合成器报的拖放位置是窗口表面坐标；界面放大后要换成布局坐标，落点才对得上量出来的位置。
#[test]
fn native_drop_positions_follow_the_interface_scale() {
    use desktop_linux::{
        WaylandDndDropOrigin, WaylandDndDropPosition, WaylandFileDropTargetEvent,
        WaylandFileDropTargetSessionId,
    };

    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    drop(browser.select_text_size(18));

    drop(
        browser.accept_wayland_target_event(WaylandFileDropTargetEvent::Entered {
            target_session_id: WaylandFileDropTargetSessionId::unique(),
            origin: WaylandDndDropOrigin::External,
            position: WaylandDndDropPosition { x: 180.0, y: 90.0 },
        }),
    );

    let position = browser
        .file_drop_session
        .as_ref()
        .and_then(|session| session.position)
        .expect("drop position");
    let scale = 18.0 / 13.0;
    assert!((position.x - 180.0 / scale).abs() < 0.001, "{position:?}");
    assert!((position.y - 90.0 / scale).abs() < 0.001, "{position:?}");
}

/// 主窗口的布局大小跟着界面缩放走，不靠窗口再发大小：启动读到的字号、在这里改的、别的窗口
/// 改的都一样（启动时按默认字号记下窗口大小，读到设置后不改，栏宽就一直按大了的宽度排，
/// 一页摆不下设的栏数）。
#[test]
fn the_main_window_layout_size_follows_the_interface_scale() {
    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    drop(browser.handle_auxiliary_window_resized(browser.main_window, 1282.0, 900.0));
    assert_eq!(browser.main_window_width(), 1282.0);

    browser.user_config.interface_text_size = 15;
    let scale = browser.interface_scale();
    assert!(scale > 1.0);
    assert!((browser.main_window_width() - 1282.0 / scale).abs() < 0.01);
    assert!((browser.main_window_height() - 900.0 / scale).abs() < 0.01);

    // 这之后窗口发来的大小是按新缩放折算的布局大小，记下来仍然一致。
    drop(browser.handle_auxiliary_window_resized(browser.main_window, 1000.0, 800.0));
    assert!((browser.main_window_width() - 1000.0).abs() < 0.01);
}

/// 预览窗口记的是布局大小：字号一换（这里选的或别的窗口改的）按新比例取回它的大小。
#[tokio::test]
async fn a_new_text_size_rereads_the_preview_window_size() {
    async fn asks_size_of(window: iced::window::Id, task: Task<crate::model::Message>) -> bool {
        use iced::futures::StreamExt;
        let Some(mut stream) = iced_runtime::task::into_stream(task) else {
            return false;
        };
        while let Some(action) = stream.next().await {
            if matches!(
                action,
                iced_runtime::Action::Window(iced_runtime::window::Action::GetSize(asked, _))
                    if asked == window
            ) {
                return true;
            }
        }
        false
    }
    let preview = iced::window::Id::unique();

    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.preview_engine.preview_window = Some(preview);
    assert!(asks_size_of(preview, browser.select_text_size(16)).await);

    let (mut browser, _) = FileBrowser::new(config::default_user_config());
    browser.preview_engine.preview_window = Some(preview);
    let mut saved_elsewhere = browser.user_config.user_preferences();
    saved_elsewhere.interface_text_size = 14;
    let adopted = browser.accept_stored_user_preferences(Ok(saved_elsewhere));
    assert_eq!(browser.user_config.interface_text_size, 14);
    assert!(asks_size_of(preview, adopted).await);
}
