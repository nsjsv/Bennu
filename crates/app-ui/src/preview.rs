// 纯搬移：空格预览的路径分类与内容加载管线已下沉 bennu-preview
// （preview_loading 模块）；re-export 维持 crate::preview::* 既有调用
// 路径（app.rs 与键盘导航的分类判定经由本路径消费）。
pub(crate) use bennu_preview::preview_loading::{classify_preview_path, PreviewPathKind};
