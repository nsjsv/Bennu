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
const INTEL_VENDOR_ID: &str = "0x8086";
const NVIDIA_VENDOR_ID: &str = "0x10de";
const AMD_VENDOR_ID: &str = "0x1002";
const AMD_APU_VENDOR_ID: &str = "0x1022";
const AMD_INTEGRATED_DEVICE_IDS: &[&str] = &[
    "0x15bf", "0x15d8", "0x15dd", "0x15e7", "0x1636", "0x1638", "0x164c", "0x164d", "0x1681",
    "0x1688",
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayRendererGpu {
    class: DisplayRendererGpuClass,
    vendor_id: String,
    device_id: String,
}

impl DisplayRendererGpu {
    pub fn from_drm_ids(
        class: DisplayRendererGpuClass,
        vendor_id: impl Into<String>,
        device_id: impl Into<String>,
    ) -> Self {
        Self {
            class,
            vendor_id: vendor_id.into().to_ascii_lowercase(),
            device_id: device_id.into().to_ascii_lowercase(),
        }
    }

    pub fn class(&self) -> DisplayRendererGpuClass {
        self.class
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

pub fn detect_display_renderer_gpu_class() -> Option<DisplayRendererGpuClass> {
    detect_display_renderer_gpu().map(|gpu| gpu.class())
}

pub fn detect_display_renderer_gpu() -> Option<DisplayRendererGpu> {
    detect_display_renderer_gpu_at(Path::new("/proc"), Path::new("/sys/class/drm"))
}

#[cfg(test)]
fn detect_display_renderer_gpu_class_at(
    proc_root: &Path,
    drm_class_root: &Path,
) -> Option<DisplayRendererGpuClass> {
    detect_display_renderer_gpu_at(proc_root, drm_class_root).map(|gpu| gpu.class())
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
    let vendor = read_trimmed(device_dir.join("vendor"))?.to_ascii_lowercase();
    let device = read_trimmed(device_dir.join("device"))?.to_ascii_lowercase();
    let card_node = card_node_for_drm_node(node, drm_class_root);
    let class = gpu_class_from_drm_ids(&vendor, &device, card_node.as_deref(), drm_class_root)?;

    Some(DisplayRendererGpu::from_drm_ids(class, vendor, device))
}

fn gpu_class_from_drm_ids(
    vendor: &str,
    device_id: &str,
    card_node: Option<&str>,
    drm_class_root: &Path,
) -> Option<DisplayRendererGpuClass> {
    if card_node.is_some_and(|card| has_integrated_panel_connector(card, drm_class_root)) {
        return Some(DisplayRendererGpuClass::Integrated);
    }

    match vendor {
        INTEL_VENDOR_ID => Some(DisplayRendererGpuClass::Integrated),
        NVIDIA_VENDOR_ID => Some(DisplayRendererGpuClass::Discrete),
        AMD_VENDOR_ID | AMD_APU_VENDOR_ID => amd_gpu_class(device_id),
        _ => None,
    }
}

fn amd_gpu_class(device_id: &str) -> Option<DisplayRendererGpuClass> {
    if AMD_INTEGRATED_DEVICE_IDS.contains(&device_id) {
        Some(DisplayRendererGpuClass::Integrated)
    } else {
        Some(DisplayRendererGpuClass::Discrete)
    }
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

    #[test]
    fn maps_intel_compositor_to_integrated_gpu() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let proc_root = temp_dir.path().join("proc");
        let drm_root = temp_dir.path().join("drm");
        create_compositor_fd(&proc_root, 100, "gnome-shell", "renderD128");
        create_drm_device(&drm_root, "renderD128", "card0", INTEL_VENDOR_ID, "0x9a49");

        assert_eq!(
            detect_display_renderer_gpu_class_at(&proc_root, &drm_root),
            Some(DisplayRendererGpuClass::Integrated)
        );
    }

    #[test]
    fn maps_nvidia_compositor_to_discrete_gpu() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let proc_root = temp_dir.path().join("proc");
        let drm_root = temp_dir.path().join("drm");
        create_compositor_fd(&proc_root, 100, "kwin_wayland", "renderD129");
        create_drm_device(&drm_root, "renderD129", "card1", NVIDIA_VENDOR_ID, "0x2484");

        assert_eq!(
            detect_display_renderer_gpu_class_at(&proc_root, &drm_root),
            Some(DisplayRendererGpuClass::Discrete)
        );
    }

    #[test]
    fn uses_panel_connector_before_vendor_fallback() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let proc_root = temp_dir.path().join("proc");
        let drm_root = temp_dir.path().join("drm");
        create_compositor_fd(&proc_root, 100, "gnome-shell", "renderD128");
        create_drm_device(&drm_root, "renderD128", "card0", AMD_VENDOR_ID, "0xffff");
        create_connected_connector(&drm_root, "card0-eDP-1");

        assert_eq!(
            detect_display_renderer_gpu_class_at(&proc_root, &drm_root),
            Some(DisplayRendererGpuClass::Integrated)
        );
    }

    #[test]
    fn prefers_compositor_over_xorg() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let proc_root = temp_dir.path().join("proc");
        let drm_root = temp_dir.path().join("drm");
        create_compositor_fd(&proc_root, 10, "Xorg", "renderD129");
        create_compositor_fd(&proc_root, 20, "gnome-shell", "renderD128");
        create_drm_device(&drm_root, "renderD128", "card0", INTEL_VENDOR_ID, "0x9a49");
        create_drm_device(&drm_root, "renderD129", "card1", NVIDIA_VENDOR_ID, "0x2484");

        assert_eq!(
            detect_display_renderer_gpu_class_at(&proc_root, &drm_root),
            Some(DisplayRendererGpuClass::Integrated)
        );
    }

    #[test]
    fn detects_niri_compositor_gpu() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let proc_root = temp_dir.path().join("proc");
        let drm_root = temp_dir.path().join("drm");
        create_compositor_fd(&proc_root, 100, "niri", "renderD128");
        create_drm_device(&drm_root, "renderD128", "card0", AMD_VENDOR_ID, "0x15bf");

        assert_eq!(
            detect_display_renderer_gpu_class_at(&proc_root, &drm_root),
            Some(DisplayRendererGpuClass::Integrated)
        );
    }

    #[test]
    fn prefers_integrated_panel_over_external_connector() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let proc_root = temp_dir.path().join("proc");
        let drm_root = temp_dir.path().join("drm");
        create_compositor_fds(&proc_root, 100, "niri", &["card0", "card1"]);
        create_drm_device(&drm_root, "card0", "card0", NVIDIA_VENDOR_ID, "0x28e0");
        create_drm_device(&drm_root, "card1", "card1", AMD_VENDOR_ID, "0x15bf");
        create_connected_connector(&drm_root, "card0-HDMI-A-1");
        create_connected_connector(&drm_root, "card1-eDP-1");

        let gpu = detect_display_renderer_gpu_at(&proc_root, &drm_root)
            .expect("detect display renderer gpu");

        assert_eq!(gpu.vendor_id, AMD_VENDOR_ID);
        assert_eq!(gpu.device_id, "0x15bf");
    }

    #[test]
    fn prefers_boot_vga_card_over_render_node() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let proc_root = temp_dir.path().join("proc");
        let drm_root = temp_dir.path().join("drm");
        create_compositor_fds(&proc_root, 100, "niri", &["renderD129", "card0", "card1"]);
        create_drm_device(&drm_root, "renderD129", "card0", NVIDIA_VENDOR_ID, "0x28e0");
        create_drm_device(&drm_root, "card0", "card0", NVIDIA_VENDOR_ID, "0x28e0");
        create_drm_device(&drm_root, "card1", "card1", AMD_VENDOR_ID, "0x15bf");
        create_boot_vga(&drm_root, "card0", "0");
        create_boot_vga(&drm_root, "card1", "1");

        let gpu = detect_display_renderer_gpu_at(&proc_root, &drm_root)
            .expect("detect display renderer gpu");

        assert_eq!(gpu.class(), DisplayRendererGpuClass::Integrated);
        assert_eq!(gpu.mesa_vulkan_device_select(), "1002:15bf!");
    }

    #[test]
    fn formats_mesa_vulkan_device_selection() {
        let gpu = DisplayRendererGpu::from_drm_ids(
            DisplayRendererGpuClass::Discrete,
            NVIDIA_VENDOR_ID,
            "0x28e0",
        );

        assert_eq!(gpu.mesa_vulkan_device_select(), "10de:28e0!");
    }

    #[test]
    fn maps_gpu_vendor_to_loader_driver_filter() {
        let nvidia =
            DisplayRendererGpu::from_drm_ids(DisplayRendererGpuClass::Discrete, "0x10de", "0x28e0");
        let amd = DisplayRendererGpu::from_drm_ids(
            DisplayRendererGpuClass::Integrated,
            "0x1002",
            "0x15bf",
        );
        let unknown = DisplayRendererGpu::from_drm_ids(
            DisplayRendererGpuClass::Integrated,
            "0x1234",
            "0x5678",
        );

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
