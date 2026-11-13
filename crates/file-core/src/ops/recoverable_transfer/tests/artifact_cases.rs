//! Owned artifacts: only a directory proven ours by its marker is ever removed,
//! and one whose creation never reached the disk is treated as unclaimed.

use super::*;

#[tokio::test]
async fn owned_artifact_requires_matching_marker_and_identity() {
    let directory = tempdir().unwrap();
    let plan =
        plan_owned_artifact(directory.path(), OwnedArtifactKind::TargetStaging, owner()).unwrap();
    let artifact = create_owned_artifact(plan, &mut CommitBarrier::default())
        .await
        .unwrap();
    fs::write(artifact.plan.payload_path(), b"partial")
        .await
        .unwrap();

    validate_owned_artifact(&artifact).await.unwrap();
    remove_owned_artifact(&artifact, &mut CommitBarrier::default())
        .await
        .unwrap();
    assert!(fs::symlink_metadata(&artifact.plan.root).await.is_err());
}

#[tokio::test]
async fn owned_staging_cleanup_refuses_backup_entry() {
    let directory = tempdir().unwrap();
    let plan =
        plan_owned_artifact(directory.path(), OwnedArtifactKind::TargetStaging, owner()).unwrap();
    let artifact = create_owned_artifact(plan.clone(), &mut CommitBarrier::default())
        .await
        .unwrap();
    fs::create_dir(plan.payload_path()).await.unwrap();
    fs::write(plan.payload_path().join("partial"), b"source")
        .await
        .unwrap();
    fs::write(plan.backup_path(), b"old-target").await.unwrap();

    assert!(matches!(
        remove_owned_artifact(&artifact, &mut CommitBarrier::default()).await,
        Err(RecoverableTransferError::ArtifactOwnership { .. })
    ));
    assert!(fs::symlink_metadata(&plan.root).await.is_ok());
    assert_eq!(fs::read(plan.backup_path()).await.unwrap(), b"old-target");
    assert_eq!(
        fs::read(plan.payload_path().join("partial")).await.unwrap(),
        b"source"
    );
}

#[tokio::test]
async fn owned_cleanup_finishes_empty_root_after_owner_removal() {
    let directory = tempdir().unwrap();
    let plan =
        plan_owned_artifact(directory.path(), OwnedArtifactKind::TargetStaging, owner()).unwrap();
    let artifact = create_owned_artifact(plan.clone(), &mut CommitBarrier::default())
        .await
        .unwrap();
    fs::remove_file(plan.owner_path()).await.unwrap();

    remove_owned_artifact_if_exists(&artifact, &mut CommitBarrier::default())
        .await
        .unwrap();

    assert!(fs::symlink_metadata(&plan.root).await.is_err());
}

#[tokio::test]
async fn tampered_owner_marker_blocks_recursive_cleanup() {
    let directory = tempdir().unwrap();
    let plan =
        plan_owned_artifact(directory.path(), OwnedArtifactKind::TargetStaging, owner()).unwrap();
    let artifact = create_owned_artifact(plan, &mut CommitBarrier::default())
        .await
        .unwrap();
    let payload = artifact.plan.payload_path();
    fs::write(&payload, b"must-stay").await.unwrap();
    fs::write(artifact.plan.owner_path(), b"not-the-owner")
        .await
        .unwrap();

    assert!(matches!(
        remove_owned_artifact(&artifact, &mut CommitBarrier::default()).await,
        Err(RecoverableTransferError::ArtifactOwnership { .. })
    ));
    assert_eq!(fs::read(&payload).await.unwrap(), b"must-stay");
}

#[cfg(unix)]
#[tokio::test]
async fn symbolic_link_owner_marker_cannot_authorize_cleanup() {
    let directory = tempdir().unwrap();
    let plan =
        plan_owned_artifact(directory.path(), OwnedArtifactKind::TargetStaging, owner()).unwrap();
    let artifact = create_owned_artifact(plan, &mut CommitBarrier::default())
        .await
        .unwrap();
    let owner_path = artifact.plan.owner_path();
    let marker = fs::read(&owner_path).await.unwrap();
    let external_marker = directory.path().join("external-owner");
    fs::write(&external_marker, marker).await.unwrap();
    fs::remove_file(&owner_path).await.unwrap();
    std::os::unix::fs::symlink(&external_marker, &owner_path).unwrap();
    let payload = artifact.plan.payload_path();
    fs::write(&payload, b"must-stay").await.unwrap();

    assert!(
        remove_owned_artifact(&artifact, &mut CommitBarrier::default())
            .await
            .is_err()
    );
    assert_eq!(fs::read(&payload).await.unwrap(), b"must-stay");
}

#[tokio::test]
async fn unclaimed_artifact_cleanup_only_removes_empty_or_cut_short_marker() {
    let directory = tempdir().unwrap();
    let empty_plan = OwnedArtifactPlan {
        kind: OwnedArtifactKind::TargetStaging,
        root: directory.path().join("empty"),
        token: ArtifactToken::from_bytes([1; 16]),
        owner: owner(),
    };
    fs::create_dir(&empty_plan.root).await.unwrap();
    remove_unclaimed_artifact(&empty_plan, &mut CommitBarrier::default())
        .await
        .unwrap();
    assert!(fs::symlink_metadata(&empty_plan.root).await.is_err());

    // 崩溃停在本批落盘之前：marker 只写进去一截，目录还没被任何 checkpoint 认领。
    let cut_short_plan = OwnedArtifactPlan {
        kind: OwnedArtifactKind::TargetStaging,
        root: directory.path().join("cut-short"),
        token: ArtifactToken::from_bytes([3; 16]),
        owner: owner(),
    };
    fs::create_dir(&cut_short_plan.root).await.unwrap();
    let marker = cut_short_plan.marker_bytes();
    fs::write(cut_short_plan.owner_path(), &marker[..marker.len() / 2])
        .await
        .unwrap();
    assert!(!owner_marker_written(&cut_short_plan).await.unwrap());
    let recreated = recover_owned_artifact(cut_short_plan.clone(), &mut CommitBarrier::default())
        .await
        .unwrap();
    validate_owned_artifact(&recreated).await.unwrap();

    let foreign_plan = OwnedArtifactPlan {
        kind: OwnedArtifactKind::TargetStaging,
        root: directory.path().join("foreign-marker"),
        token: ArtifactToken::from_bytes([4; 16]),
        owner: owner(),
    };
    fs::create_dir(&foreign_plan.root).await.unwrap();
    fs::write(foreign_plan.owner_path(), b"not ours")
        .await
        .unwrap();
    assert!(matches!(
        remove_unclaimed_artifact(&foreign_plan, &mut CommitBarrier::default()).await,
        Err(RecoverableTransferError::ArtifactOwnership { .. })
    ));
    assert_eq!(
        fs::read(foreign_plan.owner_path()).await.unwrap(),
        b"not ours"
    );

    let occupied_plan = OwnedArtifactPlan {
        kind: OwnedArtifactKind::TargetStaging,
        root: directory.path().join("occupied"),
        token: ArtifactToken::from_bytes([2; 16]),
        owner: owner(),
    };
    fs::create_dir(&occupied_plan.root).await.unwrap();
    fs::write(occupied_plan.root.join("unknown"), b"keep")
        .await
        .unwrap();
    assert!(matches!(
        remove_unclaimed_artifact(&occupied_plan, &mut CommitBarrier::default()).await,
        Err(RecoverableTransferError::ArtifactOwnership { .. })
    ));
    assert_eq!(
        fs::read(occupied_plan.root.join("unknown")).await.unwrap(),
        b"keep"
    );
}

#[tokio::test]
async fn unexpected_artifact_entry_blocks_cleanup() {
    let directory = tempdir().unwrap();
    let plan = plan_owned_artifact(
        directory.path(),
        OwnedArtifactKind::SourceRetirement,
        owner(),
    )
    .unwrap();
    let artifact = create_owned_artifact(plan, &mut CommitBarrier::default())
        .await
        .unwrap();
    fs::write(artifact.plan.root.join("untracked"), b"keep")
        .await
        .unwrap();

    assert!(matches!(
        validate_owned_artifact(&artifact).await,
        Err(RecoverableTransferError::ArtifactOwnership { .. })
    ));
}
