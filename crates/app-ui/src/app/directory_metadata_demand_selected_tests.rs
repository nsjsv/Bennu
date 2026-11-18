use std::path::Path;

use file_core::DirectoryDiscovery;
use file_core::{discover_directory_with_progress, DirectoryMetadataRequirement, ScanOptions};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::model::{BrowserLocation, DirectoryOrderPhase};

async fn discover(path: &Path) -> DirectoryDiscovery {
    discover_directory_with_progress(
        path,
        ScanOptions::default(),
        CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap()
}

/// 选中的条目在展开的子目录里：按那个目录自己的扫描结果补元数据，只补选中的那一个。
#[tokio::test]
async fn a_selected_entry_in_an_expanded_directory_demands_only_its_metadata() {
    let root = tempfile::tempdir().unwrap();
    let expanded_path = root.path().join("expanded");
    std::fs::create_dir(&expanded_path).unwrap();
    for index in 0..5 {
        std::fs::write(expanded_path.join(format!("child-{index}.dat")), [0_u8; 8]).unwrap();
    }
    let root_discovery = discover(root.path()).await;
    let expanded_discovery = discover(&expanded_path).await;
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    browser.tab_mut().choose_view_mode(BrowserViewMode::Columns);
    browser.tab_mut().location = BrowserLocation::Directory(root.path().to_path_buf());
    let request = browser.next_directory_load_request(root.path().to_path_buf());
    drop(browser.accept_directory_discovery(
        request,
        crate::model::PrebuiltDirectoryDiscovery::build(root_discovery),
    ));
    browser.tab_mut().expanded_directories.insert(
        expanded_path.clone(),
        ExpandedDirectory {
            entries: crate::model::display_entries_in_discovery_order(&expanded_discovery).into(),
            directory_discovery: Some(expanded_discovery),
            status: ExpandedDirectoryStatus::Loaded,
            is_expanded: true,
            is_collapsing: false,
            animation_progress: 1.0,
            load_generation: 3,
            load_context: None,
            load_cancel: None,
            directory_order_phase: DirectoryOrderPhase::Ready {
                field: SortField::Name,
                direction: file_core::SortDirection::Ascending,
            },
        },
    );
    let selected = expanded_path.join("child-3.dat");
    let selected_index = browser.tab().expanded_directories[&expanded_path]
        .entries
        .find(&selected)
        .and_then(|entry| entry.discovery_index())
        .expect("the selected child has a discovery index");
    browser.tab_mut().selected_paths.insert(selected);
    browser.directory_metadata_in_flight.clear();

    drop(browser.schedule_selected_metadata_for_all_panes());

    let demanded: Vec<_> = browser
        .directory_metadata_in_flight
        .iter()
        .map(|key| (key.context, key.requirement, key.index))
        .collect();
    assert_eq!(demanded.len(), 1, "{demanded:?}");
    let (context, requirement, index) = demanded[0];
    assert!(matches!(
        context,
        DirectoryMetadataLoadContext::Expanded { path, load_generation: 3, .. } if path == &expanded_path
    ));
    assert_eq!(requirement, DirectoryMetadataRequirement::Filesystem);
    assert_eq!(index, selected_index);
}
