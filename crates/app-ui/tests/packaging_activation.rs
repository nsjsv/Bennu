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
    let desktop_entry =
        fs::read_to_string(root.join("packaging/linux/bennu.desktop")).expect("read desktop entry");
    assert!(desktop_entry.lines().any(|line| line == "Exec=bennu %F"));
    assert!(desktop_entry
        .lines()
        .any(|line| line == "MimeType=inode/directory;"));
    assert!(desktop_entry.lines().any(|line| line == "Icon=bennu"));

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
fn packaged_matugen_template_renders_to_a_file_the_app_reads() {
    let root = repository_root();
    let template = fs::read_to_string(root.join("packaging/matugen/bennu-colors.toml"))
        .expect("read Matugen template");
    // Matugen 把每个 {{ colors.<role>.<light|dark>.hex }} 换成 #RRGGBB；这里换成同一个色，
    // 只验证模板和读取端对得上（版本、两套深浅、三种颜色的键名）。
    let mut rendered = String::new();
    let mut rest = template.as_str();
    while let Some(start) = rest.find("{{") {
        let end = rest[start..]
            .find("}}")
            .expect("closed template expression")
            + start
            + 2;
        rendered.push_str(&rest[..start]);
        rendered.push_str("#123456");
        rest = &rest[end..];
    }
    rendered.push_str(rest);

    bennu_theme::parse_matugen_colors(&rendered).expect("rendered template must be readable");
    for mode in ["light", "dark"] {
        for (key, role) in [
            ("background", "surface"),
            ("text", "on_surface"),
            ("accent", "primary"),
        ] {
            assert!(
                template.contains(&format!("{key} = \"{{{{ colors.{role}.{mode}.hex }}}}\"")),
                "{mode}.{key}"
            );
        }
    }
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
