use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use desktop_linux::DisplayRendererGpu;
use iced::{window, Element, Size, Task};

#[cfg(test)]
use crate::config::DEFAULT_RENDERING_GPU_PREFERENCE;
use crate::config::{self, RenderingGpuPreference};
use crate::startup_trace;

pub(crate) const ICED_BACKEND_ENV: &str = "ICED_BACKEND";
pub(crate) const MESA_VK_DEVICE_SELECT_ENV: &str = "MESA_VK_DEVICE_SELECT";
pub(crate) const WGPU_BACKEND_ENV: &str = "WGPU_BACKEND";
pub(crate) const WGPU_POWER_PREF_ENV: &str = "WGPU_POWER_PREF";
pub(crate) const VK_LOADER_DRIVERS_SELECT_ENV: &str = "VK_LOADER_DRIVERS_SELECT";
const VULKAN_BACKEND_VALUE: &str = "vulkan";
const GL_BACKEND_VALUE: &str = "gl";
const RENDERER_PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const RENDERER_PROBE_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartupRenderingBackend {
    Vulkan,
    Gl,
}

impl StartupRenderingBackend {
    pub(crate) fn environment_value(self) -> &'static str {
        match self {
            Self::Vulkan => VULKAN_BACKEND_VALUE,
            Self::Gl => GL_BACKEND_VALUE,
        }
    }

    pub(crate) fn from_environment_value(value: &str) -> Option<Self> {
        match value {
            VULKAN_BACKEND_VALUE => Some(Self::Vulkan),
            GL_BACKEND_VALUE => Some(Self::Gl),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartupRenderingEnvironment {
    preference: RenderingGpuPreference,
    backend: StartupRenderingBackend,
    variables: Vec<StartupRenderingVariable>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartupRenderingEnvironmentStatus {
    pub(crate) environment: StartupRenderingEnvironment,
    pub(crate) restart_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RendererProbeGpuSelection {
    pub(crate) wgpu_power_preference: &'static str,
    pub(crate) mesa_vulkan_device_select: String,
    pub(crate) vulkan_loader_driver_select: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StartupRenderingVariable {
    key: &'static str,
    value: Option<String>,
}

impl StartupRenderingEnvironment {
    #[cfg(test)]
    pub(crate) fn fast_default(backend: StartupRenderingBackend) -> Self {
        Self::without_display_probe(DEFAULT_RENDERING_GPU_PREFERENCE, backend)
    }

    pub(crate) fn without_display_probe(
        preference: RenderingGpuPreference,
        backend: StartupRenderingBackend,
    ) -> Self {
        Self::from_preference(preference, None, backend)
    }

    pub(crate) fn from_preference(
        preference: RenderingGpuPreference,
        display_renderer_gpu: Option<&DisplayRendererGpu>,
        backend: StartupRenderingBackend,
    ) -> Self {
        Self {
            preference,
            backend,
            variables: vec![
                StartupRenderingVariable {
                    key: ICED_BACKEND_ENV,
                    value: Some(preference.iced_backend_candidates().to_owned()),
                },
                StartupRenderingVariable {
                    key: MESA_VK_DEVICE_SELECT_ENV,
                    value: preference.mesa_vulkan_device_select(display_renderer_gpu),
                },
                StartupRenderingVariable {
                    key: WGPU_BACKEND_ENV,
                    value: Some(backend.environment_value().to_owned()),
                },
                StartupRenderingVariable {
                    key: WGPU_POWER_PREF_ENV,
                    value: preference
                        .wgpu_power_preference(display_renderer_gpu)
                        .map(str::to_owned),
                },
                StartupRenderingVariable {
                    key: VK_LOADER_DRIVERS_SELECT_ENV,
                    value: None,
                },
            ],
        }
    }

    pub(crate) fn from_probe_selection(
        preference: RenderingGpuPreference,
        gpu_selection: Option<&RendererProbeGpuSelection>,
        backend: StartupRenderingBackend,
    ) -> Self {
        let (mesa_vulkan_device_select, wgpu_power_preference, vulkan_loader_driver_select) =
            match preference {
                RenderingGpuPreference::DisplayGpu => gpu_selection
                    .map(|selection| {
                        (
                            Some(selection.mesa_vulkan_device_select.clone()),
                            Some(selection.wgpu_power_preference.to_owned()),
                            selection.vulkan_loader_driver_select.map(str::to_owned),
                        )
                    })
                    .unwrap_or((None, Some("none".to_owned()), None)),
                RenderingGpuPreference::HighPerformanceGpu => (None, Some("high".to_owned()), None),
            };

        Self {
            preference,
            backend,
            variables: vec![
                StartupRenderingVariable {
                    key: ICED_BACKEND_ENV,
                    value: Some(preference.iced_backend_candidates().to_owned()),
                },
                StartupRenderingVariable {
                    key: MESA_VK_DEVICE_SELECT_ENV,
                    value: mesa_vulkan_device_select,
                },
                StartupRenderingVariable {
                    key: WGPU_BACKEND_ENV,
                    value: Some(backend.environment_value().to_owned()),
                },
                StartupRenderingVariable {
                    key: WGPU_POWER_PREF_ENV,
                    value: wgpu_power_preference,
                },
                StartupRenderingVariable {
                    key: VK_LOADER_DRIVERS_SELECT_ENV,
                    value: vulkan_loader_driver_select,
                },
            ],
        }
    }

    pub(crate) fn backend(&self) -> StartupRenderingBackend {
        self.backend
    }

    pub(crate) fn matches_current_process(&self) -> bool {
        self.variables.iter().all(|variable| match &variable.value {
            Some(value) => std::env::var(variable.key).is_ok_and(|current| current == *value),
            None => std::env::var_os(variable.key).is_none(),
        })
    }

    fn apply_to_current_process(&self) {
        for variable in &self.variables {
            match &variable.value {
                Some(value) => std::env::set_var(variable.key, value),
                None => std::env::remove_var(variable.key),
            }
        }
    }

    pub(crate) fn apply_to_command(&self, command: &mut Command) {
        for variable in &self.variables {
            match &variable.value {
                Some(value) => {
                    command.env(variable.key, value);
                }
                None => {
                    command.env_remove(variable.key);
                }
            }
        }
    }

    #[cfg(test)]
    fn variable_value(&self, key: &'static str) -> Option<&str> {
        self.variables
            .iter()
            .find(|variable| variable.key == key)
            .and_then(|variable| variable.value.as_deref())
    }
}

impl StartupRenderingEnvironmentStatus {
    pub(crate) fn ready(environment: StartupRenderingEnvironment) -> Self {
        Self {
            environment,
            restart_required: false,
        }
    }

    pub(crate) fn for_loaded_config_with_runtime(
        preference: RenderingGpuPreference,
        runtime_environment: &StartupRenderingEnvironment,
    ) -> Self {
        let environment = if runtime_environment.preference == preference {
            runtime_environment.clone()
        } else {
            StartupRenderingEnvironment::without_display_probe(
                preference,
                runtime_environment.backend,
            )
        };
        let restart_required = !environment.matches_current_process();

        Self {
            environment,
            restart_required,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_loaded_config(
        preference: RenderingGpuPreference,
        backend: StartupRenderingBackend,
    ) -> Self {
        Self::for_loaded_config_with_runtime(
            preference,
            &StartupRenderingEnvironment::without_display_probe(preference, backend),
        )
    }
}

const RENDERER_PROBE_SELECTION_PREFIX: &str = "file-manager-renderer-gpu-v1";

impl RendererProbeGpuSelection {
    fn from_display_gpu(display_renderer_gpu: &DisplayRendererGpu) -> Self {
        Self {
            wgpu_power_preference: display_renderer_gpu.class().wgpu_power_preference(),
            mesa_vulkan_device_select: display_renderer_gpu.mesa_vulkan_device_select(),
            vulkan_loader_driver_select: display_renderer_gpu.vulkan_loader_driver_select(),
        }
    }

    fn encode(&self) -> String {
        format!(
            "{RENDERER_PROBE_SELECTION_PREFIX}\t{}\t{}\t{}",
            self.wgpu_power_preference,
            self.mesa_vulkan_device_select,
            self.vulkan_loader_driver_select.unwrap_or("none")
        )
    }

    fn decode(output: &[u8]) -> Option<Self> {
        let mut lines = std::str::from_utf8(output).ok()?.lines();
        let line = lines.next()?;
        if lines.next().is_some() {
            return None;
        }
        let mut fields = line.split('\t');
        if fields.next()? != RENDERER_PROBE_SELECTION_PREFIX {
            return None;
        }
        let wgpu_power_preference = match fields.next()? {
            "low" => "low",
            "high" => "high",
            _ => return None,
        };
        let mesa_vulkan_device_select = fields.next()?.to_owned();
        let vulkan_loader_driver_select = match fields.next()? {
            "none" => None,
            value => Some(parse_vulkan_loader_driver_select_value(value)?),
        };
        if fields.next().is_some() || !valid_mesa_vulkan_device_select(&mesa_vulkan_device_select) {
            return None;
        }
        Some(Self {
            wgpu_power_preference,
            mesa_vulkan_device_select,
            vulkan_loader_driver_select,
        })
    }
}

pub(crate) fn valid_mesa_vulkan_device_select(value: &str) -> bool {
    let Some(value) = value.strip_suffix('!') else {
        return false;
    };
    let Some((vendor_id, device_id)) = value.split_once(':') else {
        return false;
    };
    !vendor_id.is_empty()
        && !device_id.is_empty()
        && vendor_id.bytes().all(|byte| byte.is_ascii_hexdigit())
        && device_id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// wgpu 电源偏好字面量域。缓存里会出现 "none"(探针未能识别显示 GPU 或 GL 回退),
/// 探针 stdout 只产 "low"/"high"。
pub(crate) fn parse_wgpu_power_preference_value(value: &str) -> Option<&'static str> {
    match value {
        "low" => Some("low"),
        "high" => Some("high"),
        "none" => Some("none"),
        _ => None,
    }
}

pub(crate) fn parse_vulkan_loader_driver_select_value(value: &str) -> Option<&'static str> {
    match value {
        "*amd*,*radeon*" => Some("*amd*,*radeon*"),
        "*nvidia*" => Some("*nvidia*"),
        "*intel*" => Some("*intel*"),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RendererProbeCompletion {
    backend: StartupRenderingBackend,
    gpu_selection: Option<RendererProbeGpuSelection>,
}

impl RendererProbeCompletion {
    fn gl_fallback() -> Self {
        Self {
            backend: StartupRenderingBackend::Gl,
            gpu_selection: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RendererProbeOutcome {
    Succeeded,
    Failed,
    TimedOut,
}

impl RendererProbeOutcome {
    fn backend(self) -> StartupRenderingBackend {
        match self {
            Self::Succeeded => StartupRenderingBackend::Vulkan,
            Self::Failed | Self::TimedOut => StartupRenderingBackend::Gl,
        }
    }
}

fn renderer_probe_command(preference: RenderingGpuPreference) -> std::io::Result<Command> {
    let current_exe = std::env::current_exe()?;
    Ok(renderer_probe_command_for_executable(
        current_exe,
        preference,
    ))
}

fn renderer_probe_command_for_executable(
    executable: std::path::PathBuf,
    preference: RenderingGpuPreference,
) -> Command {
    let mut command = Command::new(executable);
    command
        .arg(crate::command_line::RENDERER_PROBE_ARGUMENT)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    StartupRenderingEnvironment::without_display_probe(preference, StartupRenderingBackend::Vulkan)
        .apply_to_command(&mut command);
    command
}

fn run_renderer_probe(mut command: Command, timeout: Duration) -> RendererProbeCompletion {
    let completion = match command.spawn() {
        Ok(mut child) => {
            let outcome = wait_for_renderer_probe(&mut child, timeout);
            let gpu_selection = (outcome == RendererProbeOutcome::Succeeded)
                .then(|| read_renderer_probe_selection(&mut child))
                .flatten();
            RendererProbeCompletion {
                backend: outcome.backend(),
                gpu_selection,
            }
        }
        Err(_) => RendererProbeCompletion::gl_fallback(),
    };
    completion
}

fn read_renderer_probe_selection(child: &mut Child) -> Option<RendererProbeGpuSelection> {
    let mut output = Vec::new();
    child.stdout.as_mut()?.read_to_end(&mut output).ok()?;
    RendererProbeGpuSelection::decode(&output)
}

fn wait_for_renderer_probe(child: &mut Child, timeout: Duration) -> RendererProbeOutcome {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return if status.success() {
                    RendererProbeOutcome::Succeeded
                } else {
                    RendererProbeOutcome::Failed
                };
            }
            Ok(None) => {}
            Err(_) => {
                terminate_and_reap(child);
                return RendererProbeOutcome::Failed;
            }
        }

        let elapsed = started.elapsed();
        if elapsed >= timeout {
            terminate_and_reap(child);
            return RendererProbeOutcome::TimedOut;
        }
        std::thread::sleep(std::cmp::min(
            RENDERER_PROBE_POLL_INTERVAL,
            timeout.saturating_sub(elapsed),
        ));
    }
}

fn terminate_and_reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

pub(crate) fn run_vulkan_renderer_probe() -> iced::Result {
    std::env::set_var(ICED_BACKEND_ENV, "wgpu");
    std::env::set_var(WGPU_BACKEND_ENV, VULKAN_BACKEND_VALUE);

    if let Some(gpu_selection) = renderer_probe_gpu_selection() {
        std::env::set_var(
            MESA_VK_DEVICE_SELECT_ENV,
            &gpu_selection.mesa_vulkan_device_select,
        );
        match gpu_selection.vulkan_loader_driver_select {
            Some(driver_select) => {
                std::env::set_var(VK_LOADER_DRIVERS_SELECT_ENV, driver_select);
            }
            None => std::env::remove_var(VK_LOADER_DRIVERS_SELECT_ENV),
        }
        emit_renderer_probe_selection(&gpu_selection);
    }

    iced::daemon(
        renderer_probe_boot,
        renderer_probe_update,
        renderer_probe_view,
    )
    .run()
}

fn renderer_probe_gpu_selection() -> Option<RendererProbeGpuSelection> {
    (std::env::var(WGPU_POWER_PREF_ENV).ok().as_deref() == Some("none"))
        .then(desktop_linux::detect_display_renderer_gpu)
        .flatten()
        .map(|display_renderer_gpu| {
            RendererProbeGpuSelection::from_display_gpu(&display_renderer_gpu)
        })
}

fn emit_renderer_probe_selection(gpu_selection: &RendererProbeGpuSelection) {
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{}", gpu_selection.encode());
    let _ = stdout.flush();
}

fn renderer_probe_boot() -> ((), Task<()>) {
    let settings = window::Settings {
        size: Size::new(1.0, 1.0),
        visible: false,
        resizable: false,
        closeable: false,
        minimizable: false,
        decorations: false,
        exit_on_close_request: false,
        ..window::Settings::default()
    };
    let (_, open_window) = window::open(settings);
    // window::open 的回执晚于 compositor、surface、adapter/device 和 renderer 创建。
    ((), open_window.then(|_| iced::exit()))
}

fn renderer_probe_update(_state: &mut (), _message: ()) -> Task<()> {
    Task::none()
}

fn renderer_probe_view(_state: &(), _window: window::Id) -> Element<'_, ()> {
    iced::widget::Space::new().into()
}

pub(crate) fn apply_fast_startup_environment() -> StartupRenderingEnvironment {
    let mut app_config = config::load_app_config();
    let preference = app_config.rendering_gpu_preference;
    // DisplayGpu 偏好需要显示 GPU 身份参与缓存判定与写回;sysfs 枚举毫秒级。
    let display_gpu = (preference == RenderingGpuPreference::DisplayGpu)
        .then(desktop_linux::detect_display_renderer_gpu);
    let display_gpu = display_gpu.flatten();
    let cached_environment =
        crate::startup_probe_cache::cached_probe_environment(&app_config, display_gpu.as_ref());
    let environment = match cached_environment {
        Some(environment) => {
            startup_trace::mark("startup_probe_cache_hit");
            environment
        }
        None => {
            startup_trace::mark("startup_probe_cache_miss");
            let probe_completion = renderer_probe_command(preference)
                .map(|command| run_renderer_probe(command, RENDERER_PROBE_TIMEOUT))
                .unwrap_or_else(|_| RendererProbeCompletion::gl_fallback());
            crate::startup_probe_cache::store_probe_completion(
                &mut app_config,
                probe_completion.gpu_selection.as_ref(),
                probe_completion.backend,
                display_gpu.as_ref(),
            );
            if let Err(error) = config::save_app_config(&app_config) {
                tracing::warn!(
                    target: "app_ui::startup",
                    event = "probe_cache_write_failed",
                    error = %error,
                    "failed to persist renderer probe cache"
                );
            }
            StartupRenderingEnvironment::from_probe_selection(
                preference,
                probe_completion.gpu_selection.as_ref(),
                probe_completion.backend,
            )
        }
    };
    environment.apply_to_current_process();
    startup_trace::record_rendering_backend_selected(environment.backend().environment_value());
    startup_trace::mark("startup_rendering_environment_ready");
    environment
}

pub(crate) fn restart_current_process(
    environment: &StartupRenderingEnvironment,
) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        let current_exe = std::env::current_exe()
            .map_err(|error| format!("failed to locate current executable: {error}"))?;
        let mut command = Command::new(current_exe);
        command.args(std::env::args_os().skip(1));
        environment.apply_to_command(&mut command);

        let error = command.exec();
        Err(format!("failed to restart File Manager: {error}"))
    }

    #[cfg(not(unix))]
    {
        let _ = environment;
        Err("restart is not supported on this platform".to_owned())
    }
}

#[cfg(test)]
use std::collections::HashMap;

#[cfg(test)]
pub(crate) fn environment_matches(
    environment: &StartupRenderingEnvironment,
    current_environment: &HashMap<&'static str, Option<&str>>,
) -> bool {
    environment.variables.iter().all(|variable| {
        let current_value = current_environment
            .get(variable.key)
            .copied()
            .flatten()
            .map(str::to_owned);
        current_value == variable.value
    })
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    use super::*;
    use desktop_linux::DisplayRendererGpuClass;

    const PROBE_TEST_MODE_ENV: &str = "FILE_MANAGER_RENDERER_PROBE_TEST_MODE";

    #[test]
    fn renderer_probe_test_child() {
        match std::env::var(PROBE_TEST_MODE_ENV).as_deref() {
            Ok("failure") => std::process::exit(7),
            Ok("timeout") => std::thread::sleep(Duration::from_secs(10)),
            _ => {}
        }
    }

    fn test_probe_command(mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().expect("locate test executable"));
        command
            .args([
                "--exact",
                "startup_rendering::tests::renderer_probe_test_child",
                "--nocapture",
            ])
            .env(PROBE_TEST_MODE_ENV, mode)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    #[test]
    fn renderer_probe_command_fixes_the_hidden_action_and_vulkan_environment() {
        let executable = PathBuf::from("/tmp/file-manager-renderer-probe-test");
        let command = renderer_probe_command_for_executable(
            executable.clone(),
            RenderingGpuPreference::HighPerformanceGpu,
        );

        assert_eq!(command.get_program(), executable.as_os_str());
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![OsStr::new(crate::command_line::RENDERER_PROBE_ARGUMENT)]
        );
        let environment = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect::<HashMap<_, _>>();
        assert_eq!(
            environment,
            HashMap::from([
                (ICED_BACKEND_ENV.to_owned(), Some("wgpu".to_owned())),
                (MESA_VK_DEVICE_SELECT_ENV.to_owned(), None),
                (
                    WGPU_BACKEND_ENV.to_owned(),
                    Some(VULKAN_BACKEND_VALUE.to_owned()),
                ),
                (WGPU_POWER_PREF_ENV.to_owned(), Some("high".to_owned()),),
                (VK_LOADER_DRIVERS_SELECT_ENV.to_owned(), None),
            ])
        );
    }

    #[test]
    fn fast_default_uses_a_single_selected_backend() {
        for backend in [StartupRenderingBackend::Vulkan, StartupRenderingBackend::Gl] {
            let environment = StartupRenderingEnvironment::fast_default(backend);

            assert_eq!(environment.variable_value(ICED_BACKEND_ENV), Some("wgpu"));
            assert_eq!(
                environment.variable_value(WGPU_BACKEND_ENV),
                Some(backend.environment_value())
            );
            assert!(!environment
                .variable_value(WGPU_BACKEND_ENV)
                .expect("backend value")
                .contains(','));
            assert_eq!(
                environment.variable_value(WGPU_POWER_PREF_ENV),
                Some("none")
            );
            assert_eq!(environment.variable_value(MESA_VK_DEVICE_SELECT_ENV), None);
        }
    }

    #[test]
    fn display_gpu_environment_uses_detected_gpu_when_available() {
        let gpu = DisplayRendererGpu::from_drm_ids(
            DisplayRendererGpuClass::Integrated,
            "0x1002",
            "0x15bf",
        );

        let environment = StartupRenderingEnvironment::from_preference(
            RenderingGpuPreference::DisplayGpu,
            Some(&gpu),
            StartupRenderingBackend::Vulkan,
        );

        assert_eq!(environment.variable_value(ICED_BACKEND_ENV), Some("wgpu"));
        assert_eq!(environment.variable_value(WGPU_BACKEND_ENV), Some("vulkan"));
        assert_eq!(environment.variable_value(WGPU_POWER_PREF_ENV), Some("low"));
        assert_eq!(
            environment.variable_value(MESA_VK_DEVICE_SELECT_ENV),
            Some("1002:15bf!")
        );
    }

    #[test]
    fn probe_gpu_selection_round_trips_and_rejects_untrusted_values() {
        let gpu_selection = RendererProbeGpuSelection {
            wgpu_power_preference: "low",
            mesa_vulkan_device_select: String::from("1002:15bf!"),
            vulkan_loader_driver_select: Some("*amd*,*radeon*"),
        };

        assert_eq!(
            RendererProbeGpuSelection::decode(gpu_selection.encode().as_bytes()),
            Some(gpu_selection.clone())
        );
        assert!(RendererProbeGpuSelection::decode(
            b"unexpected\nfile-manager-renderer-gpu-v1\tlow\t1002:15bf!\n"
        )
        .is_none());
        assert!(RendererProbeGpuSelection::decode(
            b"file-manager-renderer-gpu-v1\tlow\t$LD_PRELOAD!\t*nvidia*"
        )
        .is_none());
    }

    #[test]
    fn probe_gpu_selection_applies_to_display_preference_only() {
        let gpu_selection = RendererProbeGpuSelection {
            wgpu_power_preference: "low",
            mesa_vulkan_device_select: String::from("1002:15bf!"),
            vulkan_loader_driver_select: Some("*amd*,*radeon*"),
        };
        let display_environment = StartupRenderingEnvironment::from_probe_selection(
            RenderingGpuPreference::DisplayGpu,
            Some(&gpu_selection),
            StartupRenderingBackend::Vulkan,
        );
        let high_performance_environment = StartupRenderingEnvironment::from_probe_selection(
            RenderingGpuPreference::HighPerformanceGpu,
            Some(&gpu_selection),
            StartupRenderingBackend::Vulkan,
        );

        assert_eq!(
            display_environment.variable_value(MESA_VK_DEVICE_SELECT_ENV),
            Some("1002:15bf!")
        );
        assert_eq!(
            display_environment.variable_value(WGPU_POWER_PREF_ENV),
            Some("low")
        );
        assert_eq!(
            display_environment.variable_value(VK_LOADER_DRIVERS_SELECT_ENV),
            Some("*amd*,*radeon*")
        );
        assert_eq!(
            high_performance_environment.variable_value(MESA_VK_DEVICE_SELECT_ENV),
            None
        );
        assert_eq!(
            high_performance_environment.variable_value(VK_LOADER_DRIVERS_SELECT_ENV),
            None
        );
        assert_eq!(
            high_performance_environment.variable_value(WGPU_POWER_PREF_ENV),
            Some("high")
        );
    }
    #[test]
    fn high_performance_environment_avoids_display_gpu_selection() {
        let gpu = DisplayRendererGpu::from_drm_ids(
            DisplayRendererGpuClass::Integrated,
            "0x1002",
            "0x15bf",
        );

        let environment = StartupRenderingEnvironment::from_preference(
            RenderingGpuPreference::HighPerformanceGpu,
            Some(&gpu),
            StartupRenderingBackend::Gl,
        );

        assert_eq!(environment.variable_value(ICED_BACKEND_ENV), Some("wgpu"));
        assert_eq!(environment.variable_value(WGPU_BACKEND_ENV), Some("gl"));
        assert_eq!(
            environment.variable_value(WGPU_POWER_PREF_ENV),
            Some("high")
        );
        assert_eq!(environment.variable_value(MESA_VK_DEVICE_SELECT_ENV), None);
    }

    #[test]
    fn high_performance_fast_startup_honors_saved_preference_without_probe() {
        let environment = StartupRenderingEnvironment::without_display_probe(
            RenderingGpuPreference::HighPerformanceGpu,
            StartupRenderingBackend::Gl,
        );

        assert_eq!(environment.variable_value(ICED_BACKEND_ENV), Some("wgpu"));
        assert_eq!(environment.variable_value(WGPU_BACKEND_ENV), Some("gl"));
        assert_eq!(
            environment.variable_value(WGPU_POWER_PREF_ENV),
            Some("high")
        );
        assert_eq!(environment.variable_value(MESA_VK_DEVICE_SELECT_ENV), None);
    }

    #[test]
    fn loaded_config_reuses_probe_gpu_selection_in_status() {
        let gpu_selection = RendererProbeGpuSelection {
            wgpu_power_preference: "low",
            mesa_vulkan_device_select: String::from("1002:15bf!"),
            vulkan_loader_driver_select: Some("*amd*,*radeon*"),
        };
        let runtime_environment = StartupRenderingEnvironment::from_probe_selection(
            RenderingGpuPreference::DisplayGpu,
            Some(&gpu_selection),
            StartupRenderingBackend::Vulkan,
        );

        let status = StartupRenderingEnvironmentStatus::for_loaded_config_with_runtime(
            RenderingGpuPreference::DisplayGpu,
            &runtime_environment,
        );

        assert_eq!(status.environment, runtime_environment);
    }

    #[test]
    fn loaded_config_reuses_the_selected_backend() {
        let status = StartupRenderingEnvironmentStatus::for_loaded_config(
            RenderingGpuPreference::DisplayGpu,
            StartupRenderingBackend::Vulkan,
        );

        assert_eq!(
            status.environment.backend(),
            StartupRenderingBackend::Vulkan
        );
        assert_eq!(
            status.environment.variable_value(WGPU_BACKEND_ENV),
            Some("vulkan")
        );
    }

    #[test]
    fn ready_status_never_requests_restart() {
        let environment = StartupRenderingEnvironment::fast_default(StartupRenderingBackend::Gl);
        let status = StartupRenderingEnvironmentStatus::ready(environment.clone());

        assert_eq!(status.environment, environment);
        assert!(!status.restart_required);
    }

    #[test]
    fn matching_contract_distinguishes_missing_and_different_variables() {
        let environment =
            StartupRenderingEnvironment::fast_default(StartupRenderingBackend::Vulkan);
        let mut current_environment = HashMap::new();
        current_environment.insert(ICED_BACKEND_ENV, Some("wgpu"));
        current_environment.insert(WGPU_BACKEND_ENV, Some("vulkan"));
        current_environment.insert(WGPU_POWER_PREF_ENV, Some("none"));
        current_environment.insert(MESA_VK_DEVICE_SELECT_ENV, None);

        assert!(environment_matches(&environment, &current_environment));

        current_environment.insert(WGPU_BACKEND_ENV, Some("gl"));

        assert!(!environment_matches(&environment, &current_environment));

        current_environment.insert(WGPU_BACKEND_ENV, Some("vulkan"));
        current_environment.insert(WGPU_POWER_PREF_ENV, Some("high"));

        assert!(!environment_matches(&environment, &current_environment));
    }

    #[test]
    fn probe_success_selects_vulkan() {
        assert_eq!(
            run_renderer_probe(test_probe_command("success"), Duration::from_secs(5),).backend,
            StartupRenderingBackend::Vulkan
        );
    }

    #[test]
    fn probe_failure_selects_gl() {
        assert_eq!(
            run_renderer_probe(test_probe_command("failure"), Duration::from_secs(5),).backend,
            StartupRenderingBackend::Gl
        );
    }

    #[test]
    fn probe_timeout_kills_and_reaps_child_before_selecting_gl() {
        let mut child = test_probe_command("timeout")
            .spawn()
            .expect("spawn timeout probe child");

        assert_eq!(
            wait_for_renderer_probe(&mut child, Duration::from_millis(30)),
            RendererProbeOutcome::TimedOut
        );
        assert!(child.try_wait().expect("check reaped child").is_some());
    }

    #[test]
    fn probe_spawn_failure_selects_gl() {
        let command = Command::new("/path/that/does/not/exist");
        assert_eq!(
            run_renderer_probe(command, Duration::from_secs(1)).backend,
            StartupRenderingBackend::Gl
        );
    }
}
