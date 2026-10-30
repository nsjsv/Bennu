use std::fs;

use tempfile::tempdir;

use super::*;
use crate::ops::scripted_pacer::ScriptedPacer;

type LinkHook = Box<dyn FnMut(&Path) -> io::Result<()> + Send>;

/// 真实文件系统；创建某个链接前先跑 `before_symlink`，它返回错误时这一步失败。
struct HookedLinks {
    before_symlink: LinkHook,
}

impl LinkFilesystem for HookedLinks {
    fn symlink(&mut self, target: &Path, link: &Path) -> io::Result<()> {
        (self.before_symlink)(link)?;
        SystemEntryFilesystem.symlink(target, link)
    }

    fn read_link(&mut self, link: &Path) -> io::Result<PathBuf> {
        SystemEntryFilesystem.read_link(link)
    }

    fn remove_link(&mut self, link: &Path) -> io::Result<()> {
        SystemEntryFilesystem.remove_link(link)
    }
}

fn failing_at(failing_link: PathBuf) -> HookedLinks {
    HookedLinks {
        before_symlink: Box::new(move |link| {
            if link == failing_link {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected failure",
                ))
            } else {
                Ok(())
            }
        }),
    }
}

fn names(directory: &Path) -> Vec<String> {
    let mut names = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    names.sort();
    names
}

/// links 目录里给 a.txt、b.txt 各建一个链接。
fn two_link_requests(root: &Path) -> Vec<SymbolicLinkRequest> {
    fs::create_dir(root.join("links")).unwrap();
    ["a.txt", "b.txt"]
        .into_iter()
        .map(|name| {
            fs::write(root.join(name), name).unwrap();
            SymbolicLinkRequest {
                link: root.join("links").join(name),
                target: root.join(name),
            }
        })
        .collect()
}

#[tokio::test]
async fn creates_every_requested_link() {
    let root = tempdir().unwrap();
    let requests = two_link_requests(root.path());
    let mut pacer = ScriptedPacer::default();

    create_symbolic_links_with(&mut SystemEntryFilesystem, &requests, &mut pacer)
        .await
        .unwrap();

    for request in &requests {
        assert_eq!(fs::read_link(&request.link).unwrap(), request.target);
    }
    assert_eq!(pacer.progress, [(1, 2), (2, 2)]);
}

#[tokio::test]
async fn a_failure_removes_the_links_already_created() {
    let root = tempdir().unwrap();
    let requests = two_link_requests(root.path());
    let mut filesystem = failing_at(requests[1].link.clone());

    let error =
        create_symbolic_links_with(&mut filesystem, &requests, &mut ScriptedPacer::default())
            .await
            .unwrap_err();

    assert!(
        matches!(&error, FileError::CreateSymbolicLink { link, .. } if *link == requests[1].link)
    );
    assert!(names(&root.path().join("links")).is_empty());
}

#[tokio::test]
async fn cancelling_midway_removes_the_links_already_created() {
    let root = tempdir().unwrap();
    let requests = two_link_requests(root.path());

    let error = create_symbolic_links_with(
        &mut SystemEntryFilesystem,
        &requests,
        &mut ScriptedPacer::cancelling_before(1),
    )
    .await
    .unwrap_err();

    assert!(matches!(error, FileError::Cancelled));
    assert!(names(&root.path().join("links")).is_empty());
}

#[tokio::test]
async fn an_existing_entry_at_the_link_path_is_not_replaced() {
    let root = tempdir().unwrap();
    let requests = two_link_requests(root.path());
    fs::write(&requests[1].link, b"existing").unwrap();

    let error = create_symbolic_links_with(
        &mut SystemEntryFilesystem,
        &requests,
        &mut ScriptedPacer::default(),
    )
    .await
    .unwrap_err();

    assert!(matches!(error, FileError::CreateSymbolicLink { .. }));
    assert_eq!(fs::read(&requests[1].link).unwrap(), b"existing");
    assert_eq!(names(&root.path().join("links")), ["b.txt"]);
}

#[tokio::test]
async fn a_link_replaced_during_the_operation_is_kept_and_reported() {
    let root = tempdir().unwrap();
    let requests = two_link_requests(root.path());
    let (replaced_link, failing_link) = (requests[0].link.clone(), requests[1].link.clone());
    let elsewhere = root.path().join("elsewhere");
    let mut filesystem = HookedLinks {
        // 建第二个链接之前，第一个链接被别人换成了指向别处的链接，然后这一步失败。
        before_symlink: Box::new(move |link| {
            if link != failing_link {
                return Ok(());
            }
            fs::remove_file(&replaced_link)?;
            std::os::unix::fs::symlink(&elsewhere, &replaced_link)?;
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected failure",
            ))
        }),
    };

    let error =
        create_symbolic_links_with(&mut filesystem, &requests, &mut ScriptedPacer::default())
            .await
            .unwrap_err();

    let FileError::EntryMovesIncomplete {
        leftover_entries, ..
    } = &error
    else {
        panic!("expected an incomplete rollback, got {error:?}");
    };
    assert_eq!(leftover_entries.len(), 1);
    assert_eq!(leftover_entries[0].path, requests[0].link);
    assert_eq!(
        fs::read_link(&requests[0].link).unwrap(),
        root.path().join("elsewhere")
    );
}
