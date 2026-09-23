// 纯搬移：动图预览的类型、GIF 解码管线、首帧加载与流式订阅均已下沉
// bennu-preview；此处 re-export 维持 crate::animated_image_preview::*
// 既有调用路径（model.rs 的 Message 变体与 preview_state/测试经由本
// 路径消费）。流式订阅由 app.rs 直接引用 bennu-preview。
pub(crate) use bennu_preview::animated_image_preview::{AnimatedImageFrame, AnimatedImagePreview};
// 仅测试代码构造动图播放形态，非测试构建下按需门控避免未用告警。
#[cfg(test)]
pub(crate) use bennu_preview::animated_image_preview::AnimatedImagePlayback;
