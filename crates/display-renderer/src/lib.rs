use std::fs;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const COMPOSITOR_COMM_PRIORITY: &[&str] = &[
    "gnome-shell",
    "kwin_wayland",
    "kwin_x11",
    "mutter",
    "cinnamon",
    "niri",
    "sway",
    "hyprland",
    "wayfire",
    "weston",
    "labwc",
    "river",
    "xorg",
    "x",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayRendererGpuClass {
    Integrated,
    Discrete,
}

impl DisplayRendererGpuClass {
    pub fn wgpu_power_preference(self) -> &'static str {
        match self {
            Self::Integrated => "low",
            Self::Discrete => "high",
        }
    }
}

/// 合成器正在用的那块卡（DRM 报告的 PCI vendor/device）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayRendererGpu {
    vendor_id: String,
    device_id: String,
}

impl DisplayRendererGpu {
    pub fn from_drm_ids(vendor_id: impl Into<String>, device_id: impl Into<String>) -> Self {
        Self {
            vendor_id: vendor_id.into().to_ascii_lowercase(),
            device_id: device_id.into().to_ascii_lowercase(),
        }
    }

    /// 这块卡是集显还是独显，以 Vulkan 驱动报告的设备类型为准：驱动用的是
    /// 和 switcheroo-control 一样的内核事实（APU 标志、有没有独立显存），
    /// 不靠设备号表。调用前先把本卡的 `mesa_vulkan_device_select` 与
    /// `vulkan_loader_driver_select` 写进环境，Vulkan 只加载这块卡的驱动，
    /// 不会唤醒其它卡。拿不到（找不到这块卡、CPU/虚拟卡）时返回 `None`。
    pub fn vulkan_class(&self) -> Option<DisplayRendererGpuClass> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        self.class_among(
            instance
                .enumerate_adapters(wgpu::Backends::VULKAN)
                .iter()
                .map(|adapter| {
                    let info = adapter.get_info();
                    (info.vendor, info.device, info.device_type)
                }),
        )
    }

    fn class_among(
        &self,
        adapters: impl IntoIterator<Item = (u32, u32, wgpu::DeviceType)>,
    ) -> Option<DisplayRendererGpuClass> {
        let vendor = u32::from_str_radix(hex_id_without_prefix(&self.vendor_id), 16).ok()?;
        let device = u32::from_str_radix(hex_id_without_prefix(&self.device_id), 16).ok()?;
        adapters
            .into_iter()
            .find(|(adapter_vendor, adapter_device, _)| {
                (*adapter_vendor, *adapter_device) == (vendor, device)
            })
            .and_then(|(_, _, device_type)| match device_type {
                wgpu::DeviceType::IntegratedGpu => Some(DisplayRendererGpuClass::Integrated),
                wgpu::DeviceType::DiscreteGpu => Some(DisplayRendererGpuClass::Discrete),
                wgpu::DeviceType::Other | wgpu::DeviceType::VirtualGpu | wgpu::DeviceType::Cpu => {
                    None
                }
            })
    }

    pub fn mesa_vulkan_device_select(&self) -> String {
        format!(
            "{}:{}!",
            hex_id_without_prefix(&self.vendor_id),
            hex_id_without_prefix(&self.device_id)
        )
    }

    pub fn vulkan_loader_driver_select(&self) -> Option<&'static str> {
        match self.vendor_id.as_str() {
            "0x1002" | "0x1022" => Some("*amd*,*radeon*"),
            "0x10de" => Some("*nvidia*"),
            "0x8086" => Some("*intel*"),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CompositorCandidate {
    pid: u32,
    comm_rank: usize,
    same_user: bool,
}

pub fn detect_display_renderer_gpu() -> Option<DisplayRendererGpu> {
    detect_display_renderer_gpu_at(Path::new("/proc"), Path::new("/sys/class/drm"))
}

fn detect_display_renderer_gpu_at(
    proc_root: &Path,
    drm_class_root: &Path,
) -> Option<DisplayRendererGpu> {
    compositor_candidates(proc_root)
        .into_iter()
        .flat_map(|candidate| drm_nodes_for_pid(proc_root, candidate.pid, drm_class_root))
        .find_map(|node| gpu_from_drm_node(&node, drm_class_root))
}

fn compositor_candidates(proc_root: &Path) -> Vec<CompositorCandidate> {
    let current_uid = current_uid();
    let Ok(proc_entries) = fs::read_dir(proc_root) else {
        return Vec::new();
    };
    let mut candidates = Vec::new();

    for proc_entry in proc_entries.flatten() {
        let Some(pid) = proc_entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let proc_dir = proc_entry.path();
        let Some(comm_rank) = fs::read_to_string(proc_dir.join("comm"))
            .ok()
            .map(|content| content.trim().to_owned())
            .and_then(|comm| compositor_comm_rank(&comm))
        else {
            continue;
        };

        let same_user = current_uid
            .zip(proc_entry.metadata().ok().map(|stat| stat.uid()))
            .map(|(current, owner)| current == owner)
            .unwrap_or(false);

        candidates.push(CompositorCandidate {
            pid,
            comm_rank,
            same_user,
        });
    }

    candidates.sort_by(|left, right| {
        right
            .same_user
            .cmp(&left.same_user)
            .then_with(|| left.comm_rank.cmp(&right.comm_rank))
            .then_with(|| left.pid.cmp(&right.pid))
    });
    candidates
}

#[cfg(unix)]
fn current_uid() -> Option<u32> {
    fs::metadata("/proc/self").ok().map(|stat| stat.uid())
}

#[cfg(not(unix))]
fn current_uid() -> Option<u32> {
    None
}

fn compositor_comm_rank(comm: &str) -> Option<usize> {
    COMPOSITOR_COMM_PRIORITY
        .iter()
        .position(|known| known.eq_ignore_ascii_case(comm))
}

fn drm_nodes_for_pid(proc_root: &Path, pid: u32, drm_class_root: &Path) -> Vec<String> {
    let fd_dir = proc_root.join(pid.to_string()).join("fd");
    let Ok(fd_entries) = fs::read_dir(fd_dir) else {
        return Vec::new();
    };
    let mut nodes = fd_entries
        .flatten()
        .filter_map(|fd_entry| fs::read_link(fd_entry.path()).ok())
        .filter_map(|fd_target| drm_node_name(&fd_target))
        .collect::<Vec<_>>();

    nodes.sort_by(|left, right| {
        drm_node_rank(left, drm_class_root)
            .cmp(&drm_node_rank(right, drm_class_root))
            .then_with(|| left.cmp(right))
    });
    nodes.dedup();
    nodes
}

fn drm_node_name(fd_target: &Path) -> Option<String> {
    let target_text = fd_target.to_string_lossy();
    let node = target_text
        .strip_prefix("/dev/dri/")?
        .split_whitespace()
        .next()?;

    if node.starts_with("renderD") || node.starts_with("card") {
        Some(node.to_owned())
    } else {
        None
    }
}

fn drm_node_rank(node: &str, drm_class_root: &Path) -> u8 {
    let card_node = card_node_for_drm_node(node, drm_class_root);

    if card_node
        .as_deref()
        .is_some_and(|card| has_integrated_panel_connector(card, drm_class_root))
    {
        return 0;
    }

    if card_node
        .as_deref()
        .is_some_and(|card| has_connected_connector(card, drm_class_root))
    {
        return 1;
    }

    if node.starts_with("card")
        && card_node
            .as_deref()
            .is_some_and(|card| has_boot_vga(card, drm_class_root))
    {
        return 2;
    }

    if node.starts_with("card") {
        return 3;
    }

    if card_node
        .as_deref()
        .is_some_and(|card| has_boot_vga(card, drm_class_root))
    {
        return 4;
    }

    5
}

fn gpu_from_drm_node(node: &str, drm_class_root: &Path) -> Option<DisplayRendererGpu> {
    let device_dir = drm_class_root.join(node).join("device");
    let vendor = read_trimmed(device_dir.join("vendor"))?;
    let device = read_trimmed(device_dir.join("device"))?;
    Some(DisplayRendererGpu::from_drm_ids(vendor, device))
}

fn card_node_for_drm_node(node: &str, drm_class_root: &Path) -> Option<String> {
    if node.starts_with("card") {
        return Some(node.to_owned());
    }

    let drm_dir = drm_class_root.join(node).join("device").join("drm");
    fs::read_dir(drm_dir)
        .ok()?
        .flatten()
        .filter_map(|drm_entry| drm_entry.file_name().to_str().map(str::to_owned))
        .find(|entry_name| entry_name.starts_with("card"))
}

fn has_connected_connector(card_node: &str, drm_class_root: &Path) -> bool {
    let Ok(drm_entries) = fs::read_dir(drm_class_root) else {
        return false;
    };
    let connector_prefix = format!("{card_node}-");

    drm_entries.flatten().any(|drm_entry| {
        let entry_name = drm_entry.file_name();
        let Some(connector_name) = entry_name.to_str() else {
            return false;
        };
        connector_name.starts_with(&connector_prefix)
            && read_trimmed(drm_entry.path().join("status"))
                .is_some_and(|status| status == "connected")
    })
}

fn has_boot_vga(card_node: &str, drm_class_root: &Path) -> bool {
    read_trimmed(
        drm_class_root
            .join(card_node)
            .join("device")
            .join("boot_vga"),
    )
    .is_some_and(|boot_vga| boot_vga == "1")
}

fn has_integrated_panel_connector(card_node: &str, drm_class_root: &Path) -> bool {
    let Ok(drm_entries) = fs::read_dir(drm_class_root) else {
        return false;
    };
    let connector_prefix = format!("{card_node}-");

    drm_entries.flatten().any(|drm_entry| {
        let entry_name = drm_entry.file_name();
        let Some(connector_name) = entry_name.to_str() else {
            return false;
        };
        connector_name.starts_with(&connector_prefix)
            && (connector_name.contains("-eDP-") || connector_name.contains("-LVDS-"))
            && read_trimmed(drm_entry.path().join("status"))
                .is_some_and(|status| status == "connected")
    })
}

fn read_trimmed(path: PathBuf) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|content| content.trim().to_owned())
}

fn hex_id_without_prefix(value: &str) -> &str {
    value.strip_prefix("0x").unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn detected_device(proc_root: &Path, drm_root: &Path) -> Option<String> {
        detect_display_renderer_gpu_at(proc_root, drm_root)
            .map(|gpu| gpu.mesa_vulkan_device_select())
    }

    #[test]
    fn picks_the_card_behind_the_compositor_render_node() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let proc_root = temp_dir.path().join("proc");
        let drm_root = temp_dir.path().join("drm");
        create_compositor_fd(&proc_root, 100, "kwin_wayland", "renderD129");
        create_drm_device(&drm_root, "renderD129", "card1", "0x10de", "0x2484");

        assert_eq!(
            detected_device(&proc_root, &drm_root).as_deref(),
            Some("10de:2484!")
        );
    }

    #[test]
    fn prefers_compositor_over_xorg() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let proc_root = temp_dir.path().join("proc");
        let drm_root = temp_dir.path().join("drm");
        create_compositor_fd(&proc_root, 10, "Xorg", "renderD129");
        create_compositor_fd(&proc_root, 20, "gnome-shell", "renderD128");
        create_drm_device(&drm_root, "renderD128", "card0", "0x8086", "0x9a49");
        create_drm_device(&drm_root, "renderD129", "card1", "0x10de", "0x2484");

        assert_eq!(
            detected_device(&proc_root, &drm_root).as_deref(),
            Some("8086:9a49!")
        );
    }

    #[test]
    fn detects_niri_compositor_gpu() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let proc_root = temp_dir.path().join("proc");
        let drm_root = temp_dir.path().join("drm");
        create_compositor_fd(&proc_root, 100, "niri", "renderD128");
        create_drm_device(&drm_root, "renderD128", "card0", "0x1002", "0x15bf");

        assert_eq!(
            detected_device(&proc_root, &drm_root).as_deref(),
            Some("1002:15bf!")
        );
    }

    #[test]
    fn takes_the_class_the_vulkan_driver_reports_for_this_card() {
        use wgpu::DeviceType::{Cpu, DiscreteGpu, IntegratedGpu};
        // 同一台机器上 Vulkan 看到的卡：合成器那块按 vendor/device 找出来，
        // 类型照驱动说的，不看厂商。
        let adapters = [
            (0x8086, 0x9a49, IntegratedGpu), // Intel 核显
            (0x8086, 0x56a0, DiscreteGpu),   // Intel Arc A770
            (0x1002, 0x15bf, IntegratedGpu), // AMD APU（780M）
            (0x1002, 0x744c, DiscreteGpu),   // AMD RX 7900
            (0x10de, 0x2484, DiscreteGpu),   // NVIDIA RTX 3070
            (0x10005, 0x0000, Cpu),          // llvmpipe
        ];
        use DisplayRendererGpuClass::{Discrete, Integrated};

        for (vendor, device, expected) in [
            ("0x8086", "0x9a49", Some(Integrated)),
            ("0x8086", "0x56a0", Some(Discrete)),
            ("0x1002", "0x15bf", Some(Integrated)),
            ("0x1002", "0x744c", Some(Discrete)),
            ("0x10de", "0x2484", Some(Discrete)),
            ("0x10005", "0x0000", None),
            ("0x1234", "0x5678", None),
        ] {
            let gpu = DisplayRendererGpu::from_drm_ids(vendor, device);
            assert_eq!(gpu.class_among(adapters), expected, "{vendor}:{device}");
        }
    }

    #[test]
    fn prefers_integrated_panel_over_external_connector() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let proc_root = temp_dir.path().join("proc");
        let drm_root = temp_dir.path().join("drm");
        create_compositor_fds(&proc_root, 100, "niri", &["card0", "card1"]);
        create_drm_device(&drm_root, "card0", "card0", "0x10de", "0x28e0");
        create_drm_device(&drm_root, "card1", "card1", "0x1002", "0x15bf");
        create_connected_connector(&drm_root, "card0-HDMI-A-1");
        create_connected_connector(&drm_root, "card1-eDP-1");

        assert_eq!(
            detected_device(&proc_root, &drm_root).as_deref(),
            Some("1002:15bf!")
        );
    }

    #[test]
    fn prefers_boot_vga_card_over_render_node() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let proc_root = temp_dir.path().join("proc");
        let drm_root = temp_dir.path().join("drm");
        create_compositor_fds(&proc_root, 100, "niri", &["renderD129", "card0", "card1"]);
        create_drm_device(&drm_root, "renderD129", "card0", "0x10de", "0x28e0");
        create_drm_device(&drm_root, "card0", "card0", "0x10de", "0x28e0");
        create_drm_device(&drm_root, "card1", "card1", "0x1002", "0x15bf");
        create_boot_vga(&drm_root, "card0", "0");
        create_boot_vga(&drm_root, "card1", "1");

        assert_eq!(
            detected_device(&proc_root, &drm_root).as_deref(),
            Some("1002:15bf!")
        );
    }

    #[test]
    fn formats_mesa_vulkan_device_selection() {
        let gpu = DisplayRendererGpu::from_drm_ids("0x10DE", "0x28e0");

        assert_eq!(gpu.mesa_vulkan_device_select(), "10de:28e0!");
    }

    #[test]
    fn maps_gpu_vendor_to_loader_driver_filter() {
        let nvidia = DisplayRendererGpu::from_drm_ids("0x10de", "0x28e0");
        let amd = DisplayRendererGpu::from_drm_ids("0x1002", "0x15bf");
        let unknown = DisplayRendererGpu::from_drm_ids("0x1234", "0x5678");

        assert_eq!(nvidia.vulkan_loader_driver_select(), Some("*nvidia*"));
        assert_eq!(amd.vulkan_loader_driver_select(), Some("*amd*,*radeon*"));
        assert_eq!(unknown.vulkan_loader_driver_select(), None);
    }

    fn create_compositor_fd(proc_root: &Path, pid: u32, comm: &str, drm_node: &str) {
        create_compositor_fds(proc_root, pid, comm, &[drm_node]);
    }

    fn create_compositor_fds(proc_root: &Path, pid: u32, comm: &str, drm_nodes: &[&str]) {
        let proc_dir = proc_root.join(pid.to_string());
        let fd_dir = proc_dir.join("fd");
        fs::create_dir_all(&fd_dir).expect("create fd dir");
        fs::write(proc_dir.join("comm"), comm).expect("write comm");
        for (position, drm_node) in drm_nodes.iter().enumerate() {
            symlink(
                format!("/dev/dri/{drm_node}"),
                fd_dir.join((position + 3).to_string()),
            )
            .expect("create fd symlink");
        }
    }

    fn create_drm_device(
        drm_root: &Path,
        drm_node: &str,
        card_node: &str,
        vendor: &str,
        device: &str,
    ) {
        let device_dir = drm_root.join(drm_node).join("device");
        fs::create_dir_all(device_dir.join("drm").join(card_node)).expect("create drm device");
        fs::write(device_dir.join("vendor"), vendor).expect("write vendor");
        fs::write(device_dir.join("device"), device).expect("write device");
    }

    fn create_connected_connector(drm_root: &Path, connector_name: &str) {
        let connector_dir = drm_root.join(connector_name);
        fs::create_dir_all(&connector_dir).expect("create connector");
        fs::write(connector_dir.join("status"), "connected").expect("write connector status");
    }

    fn create_boot_vga(drm_root: &Path, card_node: &str, boot_vga: &str) {
        fs::write(
            drm_root.join(card_node).join("device").join("boot_vga"),
            boot_vga,
        )
        .expect("write boot_vga");
    }
}
