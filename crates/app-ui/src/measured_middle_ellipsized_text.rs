// 中间省略文本 widget（含 tooltip 变体）与自然宽度测量已整体下沉
// bennu-theme 共享（portal FileChooser 复用同一份）；这里仅 re-export
// 保持 crate 内引用路径不变。
pub(crate) use bennu_theme::measured_text::{
    measured_middle_ellipsized_centered_text, measured_middle_ellipsized_text,
    measured_middle_ellipsized_wrapped_text_with_tooltip,
};
