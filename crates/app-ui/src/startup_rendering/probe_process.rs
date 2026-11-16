//! 探针子进程：在这台机器上真画一帧，确认主程序要用的那条渲染路能用。
//!
//! 只建出设备和窗口不够：有的驱动（QEMU 的 DRM native context 下的 Intel 核显）设备建得出来，一分配
//! 缓冲区、一提交命令就失败，主程序第一帧就崩。这里用已知底色清屏、读回来逐像素核对；驱动在半路崩了，
//! 子进程非正常退出，父进程同样当作这条路不能用。

use std::io::Write;

use desktop_linux::display_renderer::HardwareRenderingBackend;
use iced::{theme, window, Color, Element, Size, Task};

use super::*;

/// 底色：三个通道各不相同，读回的像素错位、通道顺序不对都看得出来。
const PROBE_BACKGROUND: Color = Color::from_rgb8(0x24, 0x8a, 0x5c);
/// 每通道允许的误差：sRGB 与线性来回换算的舍入。
const PIXEL_TOLERANCE: u8 = 2;
/// 读回的不是底色：驱动报告画了，其实没画出来。
const RENDER_MISMATCH_EXIT_CODE: i32 = 3;

pub(crate) fn run_renderer_probe_process() -> iced::Result {
    std::env::set_var(ICED_BACKEND_ENV, "wgpu");

    let display_gpu = (std::env::var(WGPU_POWER_PREF_ENV).ok().as_deref() == Some("none"))
        .then(desktop_linux::detect_display_renderer_gpu)
        .flatten();
    if let Some(display_gpu) = display_gpu {
        std::env::set_var(
            MESA_VK_DEVICE_SELECT_ENV,
            display_gpu.mesa_vulkan_device_select(),
        );
        match display_gpu.vulkan_loader_driver_select() {
            Some(driver_select) => {
                std::env::set_var(VK_LOADER_DRIVERS_SELECT_ENV, driver_select);
            }
            None => std::env::remove_var(VK_LOADER_DRIVERS_SELECT_ENV),
        }
        // 设好设备选择和驱动过滤之后再问类型：Vulkan 只加载这块卡的驱动。
        let class = display_gpu.vulkan_class();
        emit_renderer_probe_line(
            &RendererProbeGpuSelection::from_display_gpu(&display_gpu, class).encode(),
        );
    }
    // 没有真显卡就不画：lavapipe、llvmpipe 上照样画得出来，主程序就会跑在 CPU 光栅器上。
    let (backend, wgpu_backend) =
        match desktop_linux::display_renderer::hardware_rendering_backend() {
            Some(HardwareRenderingBackend::Vulkan) => {
                (StartupRenderingBackend::Vulkan, VULKAN_BACKEND_VALUE)
            }
            Some(HardwareRenderingBackend::Gl) => (StartupRenderingBackend::Gl, GL_BACKEND_VALUE),
            None => {
                emit_renderer_probe_line(&encode_backend_line(StartupRenderingBackend::Software));
                return Ok(());
            }
        };
    std::env::set_var(WGPU_BACKEND_ENV, wgpu_backend);

    iced::daemon(move || probe_boot(backend), probe_update, probe_view)
        .style(|_, _| theme::Style {
            background_color: PROBE_BACKGROUND,
            text_color: Color::BLACK,
        })
        .run()
}

fn emit_renderer_probe_line(line: &str) {
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{line}");
    let _ = stdout.flush();
}

fn probe_boot(
    backend: StartupRenderingBackend,
) -> (StartupRenderingBackend, Task<window::Screenshot>) {
    let settings = window::Settings {
        // 不能改大小的窗口最小、最大都是这个尺寸；winit 在 Wayland 上把最小尺寸抬到 2×1，比它小的
        // 尺寸会让最大小于最小，wlroots 系合成器直接以协议错误断开连接。
        size: Size::new(16.0, 16.0),
        visible: false,
        resizable: false,
        closeable: false,
        minimizable: false,
        decorations: false,
        exit_on_close_request: false,
        ..window::Settings::default()
    };
    let (_, open_window) = window::open(settings);
    // window::open 的回执晚于 compositor、surface、adapter/device 和 renderer 创建；截图在这个
    // renderer 上清屏成底色、拷进可读缓冲区再映射读回。
    (backend, open_window.then(window::screenshot))
}

fn probe_update(
    backend: &mut StartupRenderingBackend,
    screenshot: window::Screenshot,
) -> Task<window::Screenshot> {
    if !renders_background(&screenshot.rgba, screenshot.size) {
        std::process::exit(RENDER_MISMATCH_EXIT_CODE);
    }
    // 没有后端行就是 Vulkan（见 `RendererProbeReport`）。
    if *backend == StartupRenderingBackend::Gl {
        emit_renderer_probe_line(&encode_backend_line(StartupRenderingBackend::Gl));
    }
    iced::exit()
}

fn probe_view(
    _backend: &StartupRenderingBackend,
    _window: window::Id,
) -> Element<'_, window::Screenshot> {
    iced::widget::Space::new().into()
}

/// 读回的整帧都是底色。
fn renders_background(rgba: &[u8], size: Size<u32>) -> bool {
    let expected = PROBE_BACKGROUND.into_rgba8();
    let pixel_count = size.width as usize * size.height as usize;
    let (pixels, _) = rgba.as_chunks::<4>();
    pixel_count > 0
        && rgba.len() == pixel_count * 4
        && pixels.iter().all(|pixel| {
            pixel
                .iter()
                .zip(expected)
                .all(|(&actual, expected)| actual.abs_diff(expected) <= PIXEL_TOLERANCE)
        })
}

#[cfg(test)]
#[path = "probe_process_tests.rs"]
mod tests;
