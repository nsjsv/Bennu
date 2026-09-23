// 纯搬移：原图预览的解码管线（光栅/SVG）已下沉 bennu-preview；re-export
// 维持 crate::original_image_preview::* 既有调用路径（model.rs 的 Message
// 变体与 preview_state/original_image 经由本路径消费）。
pub(crate) use bennu_preview::original_image_preview::OriginalImagePreview;
