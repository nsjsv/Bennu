// 面板文本辅助已单源下沉 bennu-preview::panel_text（translate_current
// 翻译 + Advanced shaping 规则；实现与测试随迁），这里 re-export 保持
// crate::typography::* 调用路径零改动（45 处消费者）。
pub(crate) use bennu_preview::panel_text::{localized_text, readable_text, ReadableTextContent};
