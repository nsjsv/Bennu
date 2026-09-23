//! 渲染链环境设定：在任何 iced/wgpu 初始化前定下核显 → 独显 → 软件
//! 渲染的回退链。从 main.rs 拆出以守住其 800 行上限（与窗口/会话编
//! 排是彼此独立的关注点）。

/// iced fallback 候选链：GPU（wgpu）→ 软件渲染。GPU 首选由
/// [`apply_renderer_environment`] 钉在驱动显示器的 GPU（通常为核显）。
const PORTAL_ICED_BACKEND_CANDIDATES: &str = "wgpu,tiny-skia";

/// 在任何 iced/wgpu 初始化前定下渲染链：核显 → 独显 → 软件渲染。
///
/// - `ICED_BACKEND=wgpu,tiny-skia`：iced fallback 依次尝试 GPU、软件渲染。
/// - GPU 首选钉住驱动显示器的 GPU（与主软件 DisplayGpu 偏好同配方：
///   power pref + MESA 设备选择 + loader ICD 过滤）。笔记本上即核显，
///   且必然已上电；独显 NVIDIA 冷初始化实测 ~2.2s，必须排除在首选外。
/// - 检测失败时仅设 `WGPU_POWER_PREF=low`，让 wgpu 自行排序（有核显选核显）。
/// - 已存在的环境变量不覆盖：保留运维/实验入口（如强制软渲染排障）。
pub(crate) fn apply_renderer_environment() {
    if std::env::var_os("ICED_BACKEND").is_none() {
        std::env::set_var("ICED_BACKEND", PORTAL_ICED_BACKEND_CANDIDATES);
    }
    if std::env::var_os("WGPU_POWER_PREF").is_none() {
        match display_renderer::detect_display_renderer_gpu() {
            Some(gpu) => {
                std::env::set_var("WGPU_POWER_PREF", gpu.class().wgpu_power_preference());
                std::env::set_var("MESA_VK_DEVICE_SELECT", gpu.mesa_vulkan_device_select());
                if let Some(loader_select) = gpu.vulkan_loader_driver_select() {
                    std::env::set_var("VK_LOADER_DRIVERS_SELECT", loader_select);
                }
            }
            None => std::env::set_var("WGPU_POWER_PREF", "low"),
        }
    }
}
