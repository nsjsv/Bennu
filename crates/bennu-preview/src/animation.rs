// WHY：preview.rs 的 chrome 淡入淡出沿用 crate::animation:: 调用路径；
// 缓动算法单源在 bennu-theme（app-ui 的 animation.rs 同样只是 re-export），
// 下沉时不复制实现、逐字节保真。
pub use bennu_theme::animation::{ease_out_cubic, elapsed_fraction_at};
