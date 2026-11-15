//! 引擎测试共用：按指定呈现面造一个引擎。

use super::{
    default_preview_size, PreviewEngine, PreviewEngineConfig, PreviewLoadSurface,
    PreviewWindowIdentity,
};
use crate::preview::PreviewWindowProfile;
use crate::preview_config::PreviewFileSizeLimits;

pub(crate) fn test_engine(surface: PreviewLoadSurface) -> PreviewEngine {
    let mut engine = PreviewEngine::new(
        PreviewEngineConfig {
            directory_expand_levels: 1,
            extension_rules: Default::default(),
            size_limits: PreviewFileSizeLimits::with_default_limits(),
        },
        default_preview_size(PreviewWindowProfile::Regular),
        PreviewWindowIdentity {
            app_id: "bennu-preview-tests".to_owned(),
            icon: None,
        },
    );
    engine.preview_load_surface = surface;
    engine
}
