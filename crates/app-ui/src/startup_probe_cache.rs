//! 渲染探针缓存与失败兜底。字段格式(config::RendererProbeCacheRecord)归
//! config 层所有;这里负责语义校验、命中判定、写回和 `app::run` 失败后的
//! GL 兜底重启。缓存命中的不变量:版本、渲染偏好、显示 GPU 身份全部一致,
//! 且记录能还原出与探针等价的启动环境;任何一项不符都按无缓存处理。

use std::io;
use std::process::Command;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use desktop_linux::DisplayRendererGpu;

use crate::config::{self, AppConfig, RendererProbeCacheRecord, RenderingGpuPreference};
use crate::startup_rendering::{
    parse_vulkan_loader_driver_select_value, parse_wgpu_power_preference_value,
    valid_mesa_vulkan_device_select, RendererProbeGpuSelection, StartupRenderingBackend,
    StartupRenderingEnvironment,
};

const RENDERER_PROBE_CACHE_VERSION: i64 = 1;
pub(crate) const RENDERER_FALLBACK_GUARD_ENV: &str = "FILE_MANAGER_RENDERER_FALLBACK";

/// 缓存命中的完整判定。`display_gpu` 由调用方在 DisplayGpu 偏好下预先检测;
/// HighPerformanceGpu 偏好不涉及显示 GPU,传 None 即跳过身份对比。
pub(crate) fn cached_probe_environment(
    app_config: &AppConfig,
    display_gpu: Option<&DisplayRendererGpu>,
) -> Option<StartupRenderingEnvironment> {
    let record = app_config.renderer_probe_cache.as_ref()?;
    let preference = RenderingGpuPreference::from_config_value(&record.rendering_gpu_preference)?;
    if record.version != RENDERER_PROBE_CACHE_VERSION
        || preference != app_config.rendering_gpu_preference
    {
        return None;
    }
    if preference == RenderingGpuPreference::DisplayGpu
        && record.display_gpu_device_select.as_deref()
            != display_gpu
                .map(DisplayRendererGpu::mesa_vulkan_device_select)
                .as_deref()
    {
        return None;
    }
    let backend = StartupRenderingBackend::from_environment_value(&record.backend)?;
    let gpu_selection = gpu_selection_from_record(record)?;
    Some(StartupRenderingEnvironment::from_probe_selection(
        preference,
        gpu_selection.as_ref(),
        backend,
    ))
}

/// 探针完成后把结果写进 AppConfig;保存由调用方执行(save_app_config)。
pub(crate) fn store_probe_completion(
    app_config: &mut AppConfig,
    probe_gpu_selection: Option<&RendererProbeGpuSelection>,
    backend: StartupRenderingBackend,
    display_gpu: Option<&DisplayRendererGpu>,
) {
    // power 偏好镜像 from_probe_selection 的取值规则;等价性由
    // cached_environment_matches_probe_environment 系列测试钉住。
    let wgpu_power_preference = match probe_gpu_selection {
        Some(selection) => Some(selection.wgpu_power_preference),
        None => match app_config.rendering_gpu_preference {
            RenderingGpuPreference::DisplayGpu => Some("none"),
            RenderingGpuPreference::HighPerformanceGpu => Some("high"),
        },
    };
    app_config.renderer_probe_cache = Some(RendererProbeCacheRecord {
        version: RENDERER_PROBE_CACHE_VERSION,
        backend: backend.environment_value().to_owned(),
        rendering_gpu_preference: app_config
            .rendering_gpu_preference
            .config_value()
            .to_owned(),
        wgpu_power_preference: wgpu_power_preference.map(str::to_owned),
        mesa_vulkan_device_select: probe_gpu_selection.map(|selection| {
            selection.mesa_vulkan_device_select.clone()
        }),
        vulkan_loader_driver_select: probe_gpu_selection
            .and_then(|selection| selection.vulkan_loader_driver_select)
            .map(str::to_owned),
        display_gpu_device_select: display_gpu
            .filter(|_| {
                probe_gpu_selection.is_some()
                    || app_config.rendering_gpu_preference == RenderingGpuPreference::DisplayGpu
            })
            .map(DisplayRendererGpu::mesa_vulkan_device_select),
    });
}

pub(crate) fn clear_stored_probe_cache() -> io::Result<()> {
    let mut app_config = config::load_app_config();
    app_config.renderer_probe_cache = None;
    config::save_app_config(&app_config)
}

/// `app::run` 失败后的兜底:删缓存段并 exec 换 GL 环境重启一次。重启后的
/// 进程无缓存会重新探针并把真实结果写回;兜底标记防止 GL 再失败时死循环。
/// `Ok` 表示 exec 已发起(进程被替换,不再返回);`Err` 表示未重启,
/// 调用方沿用原有报错退出。
#[cfg(unix)]
pub(crate) fn gl_fallback_restart_after_renderer_failure() -> Result<(), String> {
    if std::env::var_os(RENDERER_FALLBACK_GUARD_ENV).is_some() {
        return Err("renderer GL fallback has already been attempted".to_owned());
    }
    let preference = config::load_app_config().rendering_gpu_preference;
    let _ = clear_stored_probe_cache();
    let environment =
        StartupRenderingEnvironment::without_display_probe(preference, StartupRenderingBackend::Gl);
    let Ok(current_exe) = std::env::current_exe() else {
        return Err("failed to locate current executable".to_owned());
    };
    let mut command = Command::new(current_exe);
    command.args(std::env::args_os().skip(1));
    environment.apply_to_command(&mut command);
    command.env(RENDERER_FALLBACK_GUARD_ENV, "1");
    tracing::warn!(
        target: "app_ui::startup",
        event = "renderer_gl_fallback_restart",
        "renderer initialization failed; restarting with GL fallback"
    );
    let error = command.exec();
    Err(format!(
        "failed to restart Bennu with GL fallback: {error}"
    ))
}

#[cfg(not(unix))]
pub(crate) fn gl_fallback_restart_after_renderer_failure() -> Result<(), String> {
    Err("restart is not supported on this platform".to_owned())
}

/// 记录还原为探针 GPU 选择。mesa 字段存在即视为检测成功过的 Vulkan 选择,
/// power 必须是 low/high;mesa 缺失时只接受 power none/high(检测失败或
/// 高性能偏好),low 无来源,视为损坏。
fn gpu_selection_from_record(
    record: &RendererProbeCacheRecord,
) -> Option<Option<RendererProbeGpuSelection>> {
    let Some(power) = record
        .wgpu_power_preference
        .as_deref()
        .and_then(parse_wgpu_power_preference_value)
    else {
        return None;
    };
    let Some(mesa) = record.mesa_vulkan_device_select.as_deref() else {
        return match power {
            "none" | "high" => Some(None),
            _ => None,
        };
    };
    if !matches!(power, "low" | "high") || !valid_mesa_vulkan_device_select(mesa) {
        return None;
    }
    let vulkan_loader_driver_select =
        match record.vulkan_loader_driver_select.as_deref() {
            None => None,
            Some(value) => Some(parse_vulkan_loader_driver_select_value(value)?),
        };
    Some(Some(RendererProbeGpuSelection {
        wgpu_power_preference: power,
        mesa_vulkan_device_select: mesa.to_owned(),
        vulkan_loader_driver_select,
    }))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use desktop_linux::DisplayRendererGpuClass;

    use super::*;
    use crate::startup_rendering::{
        environment_matches, ICED_BACKEND_ENV, MESA_VK_DEVICE_SELECT_ENV,
        VK_LOADER_DRIVERS_SELECT_ENV, WGPU_BACKEND_ENV, WGPU_POWER_PREF_ENV,
    };

    fn display_gpu(vendor: &str, device: &str) -> desktop_linux::DisplayRendererGpu {
        desktop_linux::DisplayRendererGpu::from_drm_ids(
            DisplayRendererGpuClass::Integrated,
            vendor,
            device,
        )
    }

    fn amd_gpu() -> desktop_linux::DisplayRendererGpu {
        display_gpu("0x1002", "0x15bf")
    }

    fn amd_selection() -> RendererProbeGpuSelection {
        RendererProbeGpuSelection {
            wgpu_power_preference: "low",
            mesa_vulkan_device_select: amd_gpu().mesa_vulkan_device_select(),
            vulkan_loader_driver_select: Some("*amd*,*radeon*"),
        }
    }

    fn app_config(preference: RenderingGpuPreference) -> AppConfig {
        let mut app_config = config::default_app_config();
        app_config.rendering_gpu_preference = preference;
        app_config
    }

    fn expected_environment_map(
        backend: StartupRenderingBackend,
        power: &str,
    ) -> HashMap<&'static str, Option<&str>> {
        HashMap::from([
            (ICED_BACKEND_ENV, Some("wgpu")),
            (WGPU_BACKEND_ENV, Some(backend.environment_value())),
            (WGPU_POWER_PREF_ENV, Some(power)),
            (MESA_VK_DEVICE_SELECT_ENV, Some("1002:15bf!")),
            (VK_LOADER_DRIVERS_SELECT_ENV, Some("*amd*,*radeon*")),
        ])
    }

    #[test]
    fn cached_environment_matches_probe_environment_for_display_gpu_vulkan() {
        let gpu = amd_gpu();
        let selection = amd_selection();
        let mut app_config = app_config(RenderingGpuPreference::DisplayGpu);
        store_probe_completion(
            &mut app_config,
            Some(&selection),
            StartupRenderingBackend::Vulkan,
            Some(&gpu),
        );

        let cached =
            cached_probe_environment(&app_config, Some(&gpu)).expect("cache hit for matching gpu");

        assert!(environment_matches(
            &cached,
            &expected_environment_map(StartupRenderingBackend::Vulkan, "low")
        ));
        assert_eq!(
            cached,
            StartupRenderingEnvironment::from_probe_selection(
                RenderingGpuPreference::DisplayGpu,
                Some(&selection),
                StartupRenderingBackend::Vulkan,
            )
        );
    }

    #[test]
    fn cached_gl_environment_without_gpu_detection_round_trips() {
        let mut app_config = app_config(RenderingGpuPreference::HighPerformanceGpu);
        store_probe_completion(&mut app_config, None, StartupRenderingBackend::Gl, None);
        let record = app_config
            .renderer_probe_cache
            .clone()
            .expect("stored record");
        assert_eq!(record.backend, "gl");
        assert_eq!(record.wgpu_power_preference.as_deref(), Some("high"));
        assert_eq!(record.mesa_vulkan_device_select, None);

        let cached = cached_probe_environment(&app_config, None).expect("cache hit");

        assert_eq!(cached.backend(), StartupRenderingBackend::Gl);
        assert_eq!(
            cached,
            StartupRenderingEnvironment::without_display_probe(
                RenderingGpuPreference::HighPerformanceGpu,
                StartupRenderingBackend::Gl,
            )
        );
    }

    #[test]
    fn cached_display_gpu_vulkan_without_detection_uses_unpinned_power() {
        let mut app_config = app_config(RenderingGpuPreference::DisplayGpu);
        store_probe_completion(&mut app_config, None, StartupRenderingBackend::Vulkan, None);

        let cached = cached_probe_environment(&app_config, None).expect("cache hit");

        assert!(environment_matches(
            &cached,
            &HashMap::from([
                (ICED_BACKEND_ENV, Some("wgpu")),
                (WGPU_BACKEND_ENV, Some("vulkan")),
                (WGPU_POWER_PREF_ENV, Some("none")),
                (MESA_VK_DEVICE_SELECT_ENV, None),
                (VK_LOADER_DRIVERS_SELECT_ENV, None),
            ])
        ));
    }

    #[test]
    fn cache_is_rejected_when_version_preference_or_gpu_changes() {
        let gpu = amd_gpu();
        let other_gpu = display_gpu("0x8086", "0x1234");
        let selection = amd_selection();
        let mut display_config = app_config(RenderingGpuPreference::DisplayGpu);
        store_probe_completion(
            &mut display_config,
            Some(&selection),
            StartupRenderingBackend::Vulkan,
            Some(&gpu),
        );

        let mut stale_version = display_config.clone();
        stale_version.renderer_probe_cache.as_mut().unwrap().version += 1;
        assert!(cached_probe_environment(&stale_version, Some(&gpu)).is_none());

        let mut other_preference = display_config.clone();
        other_preference.rendering_gpu_preference = RenderingGpuPreference::HighPerformanceGpu;
        assert!(cached_probe_environment(&other_preference, None).is_none());

        assert!(cached_probe_environment(&display_config, Some(&other_gpu)).is_none());
        assert!(cached_probe_environment(&display_config, None).is_none());
    }

    #[test]
    fn cache_is_rejected_for_damaged_fields() {
        let gpu = amd_gpu();
        let selection = amd_selection();
        let mut app_config = app_config(RenderingGpuPreference::DisplayGpu);
        store_probe_completion(
            &mut app_config,
            Some(&selection),
            StartupRenderingBackend::Vulkan,
            Some(&gpu),
        );

        let damage = |mutate: &dyn Fn(&mut RendererProbeCacheRecord)| {
            let mut damaged = app_config.clone();
            mutate(damaged.renderer_probe_cache.as_mut().unwrap());
            assert!(cached_probe_environment(&damaged, Some(&gpu)).is_none());
        };
        damage(&|record| record.backend = "dx12".to_owned());
        damage(&|record| {
            record.mesa_vulkan_device_select = Some("$LD_PRELOAD!".to_owned())
        });
        damage(&|record| record.wgpu_power_preference = Some("turbo".to_owned()));
        damage(&|record| {
            record.vulkan_loader_driver_select = Some("*everything*".to_owned())
        });
        damage(&|record| record.wgpu_power_preference = None);
        damage(&|record| record.rendering_gpu_preference = "cloud".to_owned());
    }

    #[test]
    fn gl_fallback_guard_blocks_a_second_restart() {
        std::env::set_var(RENDERER_FALLBACK_GUARD_ENV, "1");
        let outcome = gl_fallback_restart_after_renderer_failure();
        std::env::remove_var(RENDERER_FALLBACK_GUARD_ENV);
        assert_eq!(
            outcome,
            Err("renderer GL fallback has already been attempted".to_owned())
        );
    }
}
