use std::os::unix::fs::symlink;

use super::*;

/// 假的 sysfs：几块 PCI 显卡，各自的驱动，和指向它们的 render 节点。
fn fake_sys(cards: &[(&str, &str, &str, &str)]) -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("temp dir");
    for (render_node, address, vendor, driver) in cards {
        let device = root.path().join("bus/pci/devices").join(address);
        std::fs::create_dir_all(&device).expect("pci device");
        std::fs::write(device.join("vendor"), vendor).expect("vendor");
        std::fs::write(device.join("class"), "0x030000").expect("class");
        let driver_dir = root.path().join("bus/pci/drivers").join(driver);
        std::fs::create_dir_all(&driver_dir).expect("driver");
        symlink(&driver_dir, device.join("driver")).expect("driver link");
        let drm = root.path().join("class/drm").join(render_node);
        std::fs::create_dir_all(&drm).expect("drm node");
        symlink(&device, drm.join("device")).expect("device link");
    }
    root
}

#[test]
fn mesa_cards_decode_with_vaapi_on_their_own_node() {
    let sys = fake_sys(&[
        ("renderD128", "0000:01:00.0", "0x10de", "nvidia"),
        ("renderD129", "0000:66:00.0", "0x1002", "amdgpu"),
    ]);
    assert_eq!(
        decoder_for(Path::new("/dev/dri/renderD129"), sys.path()),
        Some(HardwareDecoder::Vaapi {
            render_node: PathBuf::from("/dev/dri/renderD129")
        })
    );
    // 没见过的驱动也先试 VAAPI（探测失败就用 CPU）。
    let sys = fake_sys(&[("renderD128", "0000:00:02.0", "0x8086", "xe")]);
    assert!(matches!(
        decoder_for(Path::new("/dev/dri/renderD128"), sys.path()),
        Some(HardwareDecoder::Vaapi { .. })
    ));
}

#[test]
fn nvidia_cards_decode_with_cuda_counted_in_pci_order() {
    let sys = fake_sys(&[
        ("renderD128", "0000:01:00.0", "0x10de", "nvidia"),
        ("renderD129", "0000:21:00.0", "0x10de", "nvidia"),
        ("renderD130", "0000:66:00.0", "0x1002", "amdgpu"),
    ]);
    let second = decoder_for(Path::new("/dev/dri/renderD129"), sys.path()).expect("decoder");
    assert_eq!(second, HardwareDecoder::Cuda { device_index: 1 });
    assert_eq!(
        second.input_arguments(),
        [
            "-init_hw_device",
            "cuda=hw:1",
            "-hwaccel",
            "cuda",
            "-hwaccel_device",
            "hw"
        ]
    );
    assert_eq!(
        second.environment(),
        Some(("CUDA_DEVICE_ORDER", "PCI_BUS_ID"))
    );
    assert_eq!(
        decoder_for(Path::new("/dev/dri/renderD128"), sys.path()),
        Some(HardwareDecoder::Cuda { device_index: 0 })
    );
}

#[test]
fn unknown_nodes_get_no_hardware_decoder() {
    let sys = fake_sys(&[]);
    assert_eq!(
        decoder_for(Path::new("/dev/dri/renderD128"), sys.path()),
        None
    );
}
