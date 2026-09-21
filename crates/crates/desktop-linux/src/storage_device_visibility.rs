use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Observed UDisks facts a visibility decision is made from. Kept free of
/// udisks/zbus types so the rules below stay unit-testable in isolation.
pub(super) struct UdisksVisibilityFacts {
    pub(super) hint_ignore: bool,
    pub(super) mount_points: Vec<PathBuf>,
    pub(super) fstab_entries: Vec<UdisksFstabEntry>,
    pub(super) loop_setup_by_uid: Option<u32>,
}

/// One `/etc/fstab` row tracked by UDisks (`Block.Configuration`), with the
/// `x-gvfs-show` / `x-gvfs-hide` options collapsed to a tri-state override.
pub(super) struct UdisksFstabEntry {
    pub(super) dir: PathBuf,
    pub(super) visibility_override: Option<bool>,
}

/// Identity of the desktop user running this session; mirrors what GVfs
/// resolves from `g_get_home_dir()` / `g_get_user_name()` / `getuid()`.
pub(super) struct DesktopUserContext {
    pub(super) home_dir: PathBuf,
    pub(super) user_name: String,
    pub(super) uid: Option<u32>,
}

impl DesktopUserContext {
    pub(super) fn from_process_environment() -> Self {
        Self {
            home_dir: std::env::var_os("HOME").unwrap_or_default().into(),
            user_name: std::env::var("USER").unwrap_or_default(),
            uid: current_uid(),
        }
    }
}

#[cfg(unix)]
fn current_uid() -> Option<u32> {
    fs::metadata("/proc/self").ok().map(|stat| stat.uid())
}

#[cfg(not(unix))]
fn current_uid() -> Option<u32> {
    None
}

/// UDisks-side counterpart of GVfs' `should_include_volume()` (issue #1):
/// mounted devices only surface when a mount point sits in a user-visible
/// location, so snap loops under `/snap/...` and mounts under `/mnt`, `/srv`
/// or a separate `/home` never leak into the sidebar.
pub(super) fn udisks_device_is_visible(
    facts: &UdisksVisibilityFacts,
    user: &DesktopUserContext,
) -> bool {
    if facts.hint_ignore {
        return false;
    }
    // Loop devices set up by another non-root user stay private to that user;
    // root-set-up loops (snapd, udisks-mounted images) fall through to the
    // mount point rules like every other device.
    if let Some(setup_uid) = facts.loop_setup_by_uid {
        if user
            .uid
            .is_some_and(|uid| setup_uid != 0 && setup_uid != uid)
        {
            return false;
        }
    }
    if facts.mount_points.is_empty() {
        // No fstab reference means an unmounted data volume, which stays
        // listed so it can be mounted from the sidebar.
        facts
            .fstab_entries
            .iter()
            .all(|entry| fstab_entry_is_visible(entry, user))
    } else {
        facts
            .mount_points
            .iter()
            .any(|path| mount_point_is_user_visible(path, user))
    }
}

fn fstab_entry_is_visible(entry: &UdisksFstabEntry, user: &DesktopUserContext) -> bool {
    match entry.visibility_override {
        Some(visible) => visible,
        None => mount_point_is_user_visible(&entry.dir, user),
    }
}

/// GVfs `should_include()`: everything outside the user's own locations is
/// hidden by default, with `$HOME`, `/media/<direct child>` and
/// `/run/media/$USER/...` as the allowlist.
pub(super) fn mount_point_is_user_visible(path: &Path, user: &DesktopUserContext) -> bool {
    let path = path.as_os_str().as_bytes();
    if mount_path_is_system_internal(path) {
        return false;
    }
    // Mounts inside a dot directory were hidden on purpose.
    if path.windows(2).any(|window| window == b"/.") {
        return false;
    }
    let home = user.home_dir.as_os_str().as_bytes();
    if !home.is_empty() && path.starts_with(home) && path.get(home.len()) == Some(&b'/') {
        return true;
    }
    // GVfs removes only the "/run" component (keeping the separator) so
    // /run/media/$USER/... shares the /media rule; a bare second path
    // segment (/media/usb0) is also accepted.
    let without_run = path.strip_prefix(b"/run").unwrap_or(path);
    let after_media = without_run.strip_prefix(b"/media/");
    if let Some(rest) = after_media {
        let user_name = user.user_name.as_bytes();
        if !rest.contains(&b'/') {
            return true;
        }
        if !user_name.is_empty()
            && rest.starts_with(user_name)
            && rest.get(user_name.len()) == Some(&b'/')
        {
            return true;
        }
    }
    false
}

/// GLib `g_unix_is_mount_path_system_internal()`: exact FHS matches plus
/// pseudo-filesystem parents and the legacy GVfs FUSE suffix.
fn mount_path_is_system_internal(path: &[u8]) -> bool {
    if SYSTEM_MOUNT_PATHS.binary_search(&path).is_ok() {
        return true;
    }
    for prefix in [b"/dev/" as &[u8], b"/proc/", b"/sys/"] {
        if path.starts_with(prefix) {
            return true;
        }
    }
    if path.ends_with(b"/.gvfs") {
        return true;
    }
    false
}

/// GLib `system_mount_paths` (gio/gunixmounts-private.h) with `/run` for
/// GLIB_RUNSTATEDIR; kept sorted for `binary_search`.
const SYSTEM_MOUNT_PATHS: &[&[u8]] = &[
    b"/",
    b"/bin",
    b"/boot",
    b"/compat/linux/proc",
    b"/compat/linux/sys",
    b"/dev",
    b"/etc",
    b"/home",
    b"/lib",
    b"/lib64",
    b"/libexec",
    b"/live/cow",
    b"/live/image",
    b"/media",
    b"/mnt",
    b"/net",
    b"/opt",
    b"/proc",
    b"/rescue",
    b"/root",
    b"/run",
    b"/sbin",
    b"/srv",
    b"/sys",
    b"/tmp",
    b"/usr",
    b"/usr/X11R6",
    b"/usr/local",
    b"/usr/obj",
    b"/usr/ports",
    b"/usr/src",
    b"/usr/xobj",
    b"/var",
    b"/var/crash",
    b"/var/local",
    b"/var/log",
    b"/var/log/audit",
    b"/var/mail",
    b"/var/run",
    b"/var/tmp",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn user() -> DesktopUserContext {
        DesktopUserContext {
            home_dir: PathBuf::from("/home/yuanming"),
            user_name: "yuanming".to_owned(),
            uid: Some(1000),
        }
    }

    fn mounted_facts(paths: &[&str], loop_setup_by_uid: Option<u32>) -> UdisksVisibilityFacts {
        UdisksVisibilityFacts {
            hint_ignore: false,
            mount_points: paths.iter().map(PathBuf::from).collect(),
            fstab_entries: Vec::new(),
            loop_setup_by_uid,
        }
    }

    fn unmounted_facts(fstab_entries: Vec<UdisksFstabEntry>) -> UdisksVisibilityFacts {
        UdisksVisibilityFacts {
            hint_ignore: false,
            mount_points: Vec::new(),
            fstab_entries,
            loop_setup_by_uid: None,
        }
    }

    #[test]
    fn system_mount_paths_stay_sorted_for_binary_search() {
        assert!(SYSTEM_MOUNT_PATHS.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn snap_loop_mount_is_hidden_by_system_path_rule() {
        let facts = mounted_facts(&["/snap/firefox/4921"], Some(0));

        assert!(!udisks_device_is_visible(&facts, &user()));
    }

    #[test]
    fn user_loop_image_under_home_stays_visible() {
        let facts = mounted_facts(&["/home/yuanming/images/backup.img"], Some(1000));

        assert!(udisks_device_is_visible(&facts, &user()));
    }

    #[test]
    fn loop_of_another_user_is_hidden_but_root_setup_is_not() {
        let by_other_user = mounted_facts(&["/media/shared-disk"], Some(1001));
        let by_root = mounted_facts(&["/media/shared-disk"], Some(0));

        assert!(!udisks_device_is_visible(&by_other_user, &user()));
        assert!(udisks_device_is_visible(&by_root, &user()));
    }

    #[test]
    fn internal_mount_locations_are_hidden() {
        for path in [
            "/",
            "/boot",
            "/home",
            "/var/log",
            "/mnt/data",
            "/srv/backup",
        ] {
            let facts = mounted_facts(&[path], None);
            assert!(
                !udisks_device_is_visible(&facts, &user()),
                "{path} must stay hidden"
            );
        }
    }

    #[test]
    fn pseudo_filesystem_parents_are_hidden() {
        for path in ["/dev/shm/data", "/proc/1/root", "/sys/kernel/debug"] {
            let facts = mounted_facts(&[path], None);
            assert!(
                !udisks_device_is_visible(&facts, &user()),
                "{path} must stay hidden"
            );
        }
    }

    #[test]
    fn media_locations_follow_gvfs_allowlist() {
        let visible = [
            "/media/usb0",
            "/media/yuanming/disk",
            "/run/media/yuanming/disk",
        ];
        for path in visible {
            let facts = mounted_facts(&[path], None);
            assert!(udisks_device_is_visible(&facts, &user()), "{path}");
        }

        let facts = mounted_facts(&["/media/otheruser/disk"], None);
        assert!(!udisks_device_is_visible(&facts, &user()));
    }

    #[test]
    fn dot_directory_mounts_are_hidden_even_under_home() {
        for path in [
            "/media/yuanming/.hidden-mount",
            "/home/yuanming/.cache/mount",
        ] {
            let facts = mounted_facts(&[path], None);
            assert!(
                !udisks_device_is_visible(&facts, &user()),
                "{path} must stay hidden"
            );
        }
    }

    #[test]
    fn mount_exactly_at_home_requires_trailing_separator_upstream() {
        let facts = mounted_facts(&["/home/yuanming"], None);

        assert!(!udisks_device_is_visible(&facts, &user()));
    }

    #[test]
    fn ignore_hint_hides_even_user_visible_locations() {
        let facts = UdisksVisibilityFacts {
            hint_ignore: true,
            ..mounted_facts(&["/media/usb0"], None)
        };

        assert!(!udisks_device_is_visible(&facts, &user()));
    }

    #[test]
    fn unmounted_volume_without_fstab_stays_listed() {
        let facts = unmounted_facts(Vec::new());

        assert!(udisks_device_is_visible(&facts, &user()));
    }

    #[test]
    fn unmounted_volume_with_system_fstab_entry_is_hidden() {
        let entry = |dir: &str, visibility_override: Option<bool>| UdisksFstabEntry {
            dir: PathBuf::from(dir),
            visibility_override,
        };
        let hidden = unmounted_facts(vec![entry("/boot", None)]);
        let hidden_in_mnt = unmounted_facts(vec![entry("/mnt/backup-disk", None)]);
        let shown = unmounted_facts(vec![entry("/media/backup-disk", None)]);
        let forced_show = unmounted_facts(vec![entry("/var/backups", Some(true))]);
        let forced_hide = unmounted_facts(vec![entry("/media/usb0", Some(false))]);

        assert!(!udisks_device_is_visible(&hidden, &user()));
        assert!(!udisks_device_is_visible(&hidden_in_mnt, &user()));
        assert!(udisks_device_is_visible(&shown, &user()));
        assert!(udisks_device_is_visible(&forced_show, &user()));
        assert!(!udisks_device_is_visible(&forced_hide, &user()));
    }

    #[test]
    fn any_visible_mount_point_surfaces_the_device() {
        let facts = mounted_facts(&["/var/lib/data", "/media/usb0"], None);

        assert!(udisks_device_is_visible(&facts, &user()));
    }
}
