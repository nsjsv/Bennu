// 纯搬移：文案表与 UiLanguage 已下沉 bennu-localization（预览视图的
// readable_text 自动翻译要与视图组同源），显式 re-export 维持
// crate::localization::* 既有调用路径，与 bennu-preview 下沉同套路。
pub(crate) use bennu_localization::{
    current_language, current_language_is_chinese, current_trash_tracking_warning,
    detect_system_language, direct_send_progress_line, incoming_transfer_summary,
    set_current_language, transfer_progress_line, transfer_receive_finished_body, translate,
    translate_current, trash_additional_warning_count, trash_refresh_failed,
    trash_tracking_warning, trash_warning_summary,
};
