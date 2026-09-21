use std::fs;
use std::path::{Path, PathBuf};

use desktop_linux::parse_file_uri_list;

use crate::config::SidebarFavoriteConfig;
use crate::model::{SidebarLocation, SidebarLocationKind};

pub(crate) fn home_sidebar_location(home: &Path) -> SidebarLocation {
    SidebarLocation {
        label: "Home".to_owned(),
        path: home.to_path_buf(),
        kind: SidebarLocationKind::Home,
    }
}

pub(crate) fn sidebar_locations(
    home: &Path,
    configured_favorites: Option<&[SidebarFavoriteConfig]>,
) -> Vec<SidebarLocation> {
    let mut locations = vec![home_sidebar_location(home)];

    let favorites = configured_favorites.map_or_else(
        || default_sidebar_favorite_locations(home),
        |favorites| configured_sidebar_favorite_locations(home, favorites),
    );
    locations.extend(favorites);

    locations
}

pub(crate) fn sidebar_favorite_configs(
    locations: &[SidebarLocation],
) -> Vec<SidebarFavoriteConfig> {
    locations
        .iter()
        .filter(|location| location.kind.is_user_favorite())
        .map(|location| SidebarFavoriteConfig {
            label: location.label.clone(),
            path: location.path.clone(),
        })
        .collect()
}

fn default_sidebar_favorite_locations(home: &Path) -> Vec<SidebarLocation> {
    let mut locations = Vec::new();
    for (label, kind, path) in default_user_directory_locations(home) {
        if path.is_dir() {
            push_sidebar_location(&mut locations, label, path, kind);
        }
    }

    for location in gtk_bookmark_locations(home) {
        push_sidebar_location(
            &mut locations,
            &location.label,
            location.path,
            location.kind,
        );
    }

    locations
}

fn configured_sidebar_favorite_locations(
    home: &Path,
    favorites: &[SidebarFavoriteConfig],
) -> Vec<SidebarLocation> {
    let mut locations = Vec::new();
    for favorite in favorites {
        if favorite.path.as_path() == home {
            continue;
        }
        let label = configured_sidebar_favorite_label(favorite);
        let kind = sidebar_favorite_kind(home, &favorite.path);
        push_sidebar_location(&mut locations, &label, favorite.path.clone(), kind);
    }
    locations
}

fn default_user_directory_locations(
    home: &Path,
) -> [(&'static str, SidebarLocationKind, PathBuf); 6] {
    [
        (
            "Desktop",
            SidebarLocationKind::Desktop,
            dirs::desktop_dir().unwrap_or_else(|| home.join("Desktop")),
        ),
        (
            "Documents",
            SidebarLocationKind::Documents,
            dirs::document_dir().unwrap_or_else(|| home.join("Documents")),
        ),
        (
            "Downloads",
            SidebarLocationKind::Downloads,
            dirs::download_dir().unwrap_or_else(|| home.join("Downloads")),
        ),
        (
            "Pictures",
            SidebarLocationKind::Pictures,
            dirs::picture_dir().unwrap_or_else(|| home.join("Pictures")),
        ),
        (
            "Music",
            SidebarLocationKind::Music,
            dirs::audio_dir().unwrap_or_else(|| home.join("Music")),
        ),
        (
            "Videos",
            SidebarLocationKind::Videos,
            dirs::video_dir().unwrap_or_else(|| home.join("Videos")),
        ),
    ]
}

fn gtk_bookmark_locations(home: &Path) -> Vec<SidebarLocation> {
    let mut locations = Vec::new();

    for version in ["gtk-3.0", "gtk-4.0"] {
        let path = home.join(".config").join(version).join("bookmarks");
        let Ok(content) = fs::read_to_string(path) else {
            continue;
        };

        for line in content
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
        {
            if let Some(location) = parse_gtk_bookmark(home, line) {
                if location.path.as_path() == home {
                    continue;
                }
                push_sidebar_location(
                    &mut locations,
                    &location.label,
                    location.path,
                    location.kind,
                );
            }
        }
    }

    locations
}

fn parse_gtk_bookmark(home: &Path, line: &str) -> Option<SidebarLocation> {
    let (uri, label) = line.split_once(' ').map_or((line, None), |(uri, label)| {
        (uri, Some(label.trim()).filter(|label| !label.is_empty()))
    });
    let path = parse_file_uri_list(uri).ok()?.into_iter().next()?;
    let label = label
        .map(ToOwned::to_owned)
        .or_else(|| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let kind = sidebar_favorite_kind(home, &path);

    Some(SidebarLocation { label, path, kind })
}

fn configured_sidebar_favorite_label(favorite: &SidebarFavoriteConfig) -> String {
    let label = favorite.label.trim();
    if label.is_empty() {
        sidebar_favorite_label_from_path(&favorite.path)
    } else {
        label.to_owned()
    }
}

fn sidebar_favorite_label_from_path(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn sidebar_favorite_kind(home: &Path, path: &Path) -> SidebarLocationKind {
    default_user_directory_locations(home)
        .into_iter()
        .find(|(label, _, default_path)| {
            path == default_path.as_path() || path == home.join(label).as_path()
        })
        .map(|(_, kind, _)| kind)
        .unwrap_or(SidebarLocationKind::Bookmark)
}

fn push_sidebar_location(
    locations: &mut Vec<SidebarLocation>,
    label: &str,
    path: PathBuf,
    kind: SidebarLocationKind,
) {
    if !locations.iter().any(|location| location.path == path) {
        locations.push(SidebarLocation {
            label: label.to_owned(),
            path,
            kind,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gtk_bookmark_label_and_encoded_path() {
        let bookmark = parse_gtk_bookmark(
            Path::new("/home/user"),
            "file:///home/user/My%20Folder Project Folder",
        )
        .unwrap();

        assert_eq!(bookmark.path, PathBuf::from("/home/user/My Folder"));
        assert_eq!(bookmark.label, "Project Folder");
        assert_eq!(bookmark.kind, SidebarLocationKind::Bookmark);
    }

    #[test]
    fn configured_favorites_control_user_location_order() {
        let home = Path::new("/home/user");
        let favorites = vec![
            SidebarFavoriteConfig {
                label: "Projects".to_owned(),
                path: PathBuf::from("/srv/projects"),
            },
            SidebarFavoriteConfig {
                label: "Downloads".to_owned(),
                path: home.join("Downloads"),
            },
        ];

        let locations = sidebar_locations(home, Some(&favorites));

        assert_eq!(locations[0].kind, SidebarLocationKind::Home);
        assert_eq!(locations[1].label, "Projects");
        assert_eq!(locations[1].kind, SidebarLocationKind::Bookmark);
        assert_eq!(locations[2].label, "Downloads");
        assert_eq!(locations[2].kind, SidebarLocationKind::Downloads);
    }

    #[test]
    fn sidebar_favorite_configs_exclude_home() {
        let home = PathBuf::from("/home/user");
        let downloads = home.join("Downloads");
        let configs = sidebar_favorite_configs(&[
            SidebarLocation {
                label: "Home".to_owned(),
                path: home,
                kind: SidebarLocationKind::Home,
            },
            SidebarLocation {
                label: "Downloads".to_owned(),
                path: downloads.clone(),
                kind: SidebarLocationKind::Downloads,
            },
        ]);

        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].path, downloads);
    }
}
