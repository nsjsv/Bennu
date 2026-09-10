use std::fs;
use std::path::Path;

fn repository_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("app-ui crate is nested under crates")
}

#[test]
fn desktop_and_brand_activation_files_expose_local_file_manager_contract() {
    let root = repository_root();
    let desktop_entry = fs::read_to_string(root.join("packaging/linux/bennu.desktop"))
        .expect("read desktop entry");
    assert!(desktop_entry
        .lines()
        .any(|line| line == "Exec=bennu %F"));
    assert!(desktop_entry
        .lines()
        .any(|line| line == "MimeType=inode/directory;"));
    assert!(desktop_entry
        .lines()
        .any(|line| line == "Icon=bennu"));

    let service_name = "io.github.nsjsv.Bennu.service";
    let activation_service = fs::read_to_string(root.join("packaging/linux").join(service_name))
        .expect("read activation service");
    assert!(activation_service
        .lines()
        .any(|line| line == "Name=io.github.nsjsv.Bennu"));
    assert!(activation_service
        .lines()
        .any(|line| line == "Exec=/usr/bin/bennu --activation-service"));

    // payload 布局的单一事实来源是 install-payload.sh，workflow 只调用它。
    let payload_installer = fs::read_to_string(root.join("packaging/common/install-payload.sh"))
        .expect("read install payload script");
    assert!(payload_installer.contains("usr/share/dbus-1/services/${ACTIVATION_SERVICE_FILE}"));
    assert!(payload_installer.contains("usr/share/${APP_NAME}/matugen/bennu-colors.toml"));
    assert!(payload_installer.contains("usr/share/doc/${APP_NAME}/matugen.md"));
    assert!(payload_installer.contains("icons/hicolor/512x512/apps/bennu.png"));
    assert!(payload_installer.contains("usr/share/icons/hicolor/512x512/apps/${APP_NAME}.png"));
}

#[test]
fn packaged_matugen_template_exports_every_runtime_color_role() {
    let root = repository_root();
    let template = fs::read_to_string(root.join("packaging/matugen/bennu-colors.toml"))
        .expect("read Matugen template");
    assert!(template.contains("mode = \"{{ mode }}\""));

    for role in [
        "background",
        "on_background",
        "surface",
        "surface_dim",
        "surface_bright",
        "surface_container_lowest",
        "surface_container_low",
        "surface_container",
        "surface_container_high",
        "surface_container_highest",
        "on_surface",
        "on_surface_variant",
        "outline",
        "outline_variant",
        "primary",
        "on_primary",
        "primary_container",
        "on_primary_container",
        "secondary",
        "on_secondary",
        "secondary_container",
        "on_secondary_container",
        "tertiary",
        "on_tertiary",
        "tertiary_container",
        "on_tertiary_container",
        "error",
        "on_error",
        "error_container",
        "on_error_container",
    ] {
        assert!(
            template.contains(&format!("{role} = \"{{{{ colors.{role}.default.hex }}}}\"")),
            "missing Matugen role {role}"
        );
    }

    let instructions = fs::read_to_string(root.join("packaging/matugen/README.md"))
        .expect("read Matugen instructions");
    assert!(instructions.contains("/usr/share/bennu/matugen/bennu-colors.toml"));
    assert!(instructions.contains("~/.config/bennu/matugen.toml"));
    assert!(instructions.contains("-m dark"));
    assert!(instructions.contains("-m light"));
}

#[test]
fn packaged_dbus_services_do_not_claim_the_standard_file_manager_name() {
    let service_directory = repository_root().join("packaging/linux");
    for entry in fs::read_dir(service_directory).expect("read Linux packaging directory") {
        let path = entry.expect("packaging entry").path();
        if path
            .extension()
            .is_some_and(|extension| extension == "service")
        {
            let service = fs::read_to_string(&path).expect("read service file");
            assert!(
                !service
                    .lines()
                    .any(|line| line == "Name=org.freedesktop.FileManager1"),
                "{} must not claim the standard FileManager1 name",
                path.display()
            );
        }
    }
}
