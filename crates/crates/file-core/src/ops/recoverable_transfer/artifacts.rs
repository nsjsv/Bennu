use std::path::{Path, PathBuf};

use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{
    inspect_file_identity, sync_parent_blocking, FileIdentity, FileObjectKind,
    RecoverableTransferError,
};

const OWNER_FILE_NAME: &str = "owner";
const PAYLOAD_NAME: &str = "payload";
const BACKUP_NAME: &str = "backup";
const OWNER_PROTOCOL: &str = "file-manager-transfer-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnedArtifactKind {
    TargetStaging,
    SourceRetirement,
}

impl OwnedArtifactKind {
    fn path_prefix(self) -> &'static str {
        match self {
            Self::TargetStaging => ".file-manager-transfer-",
            Self::SourceRetirement => ".file-manager-source-retirement-",
        }
    }

    fn marker_value(self) -> &'static str {
        match self {
            Self::TargetStaging => "target_staging",
            Self::SourceRetirement => "source_retirement",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ArtifactOwner {
    pub task_id: u64,
    pub transfer_index: u64,
    pub work_index: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ArtifactToken([u8; 16]);

impl ArtifactToken {
    pub fn random() -> Result<Self, RecoverableTransferError> {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes)
            .map_err(|error| RecoverableTransferError::RandomToken(error.to_string()))?;
        Ok(Self(bytes))
    }

    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    pub fn into_bytes(self) -> [u8; 16] {
        self.0
    }

    fn hex(self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut encoded = String::with_capacity(32);
        for byte in self.0 {
            encoded.push(HEX[usize::from(byte >> 4)] as char);
            encoded.push(HEX[usize::from(byte & 0x0f)] as char);
        }
        encoded
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OwnedArtifactPlan {
    pub kind: OwnedArtifactKind,
    #[serde(with = "super::path_codec")]
    pub root: PathBuf,
    pub token: ArtifactToken,
    pub owner: ArtifactOwner,
}

impl OwnedArtifactPlan {
    pub fn owner_path(&self) -> PathBuf {
        self.root.join(OWNER_FILE_NAME)
    }

    pub fn payload_path(&self) -> PathBuf {
        self.root.join(PAYLOAD_NAME)
    }

    pub fn backup_path(&self) -> PathBuf {
        self.root.join(BACKUP_NAME)
    }

    fn marker_bytes(&self) -> Vec<u8> {
        format!(
            "{OWNER_PROTOCOL}\nkind={}\ntask={}\ntransfer={}\nwork={}\ntoken={}\n",
            self.kind.marker_value(),
            self.owner.task_id,
            self.owner.transfer_index,
            self.owner.work_index,
            self.token.hex(),
        )
        .into_bytes()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OwnedArtifact {
    pub plan: OwnedArtifactPlan,
    pub root_identity: FileIdentity,
}

pub fn plan_owned_artifact(
    parent: &Path,
    kind: OwnedArtifactKind,
    owner: ArtifactOwner,
) -> Result<OwnedArtifactPlan, RecoverableTransferError> {
    let token = ArtifactToken::random()?;
    Ok(OwnedArtifactPlan {
        kind,
        root: parent.join(format!("{}{}", kind.path_prefix(), token.hex())),
        token,
        owner,
    })
}

pub async fn create_owned_artifact(
    plan: OwnedArtifactPlan,
) -> Result<OwnedArtifact, RecoverableTransferError> {
    fs::create_dir(&plan.root).await.map_err(|source| {
        RecoverableTransferError::file_system("create transfer artifact", &plan.root, source)
    })?;
    #[cfg(unix)]
    if let Err(source) = set_owner_only_permissions(&plan.root).await {
        let _ = fs::remove_dir(&plan.root).await;
        return Err(RecoverableTransferError::file_system(
            "restrict transfer artifact",
            &plan.root,
            source,
        ));
    }
    let root_identity = inspect_file_identity(&plan.root).await?;
    let owner_path = plan.owner_path();
    let marker_outcome = async {
        let mut owner_file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&owner_path)
            .await?;
        owner_file.write_all(&plan.marker_bytes()).await?;
        owner_file.sync_all().await
    }
    .await;
    if let Err(source) = marker_outcome {
        let _ = fs::remove_file(&owner_path).await;
        let _ = fs::remove_dir(&plan.root).await;
        return Err(RecoverableTransferError::file_system(
            "write transfer owner marker",
            &owner_path,
            source,
        ));
    }
    sync_directory_in_place(&plan.root).await?;
    sync_parent_in_place(&plan.root).await?;

    Ok(OwnedArtifact {
        plan,
        root_identity,
    })
}

pub async fn recover_owned_artifact(
    plan: OwnedArtifactPlan,
) -> Result<OwnedArtifact, RecoverableTransferError> {
    match fs::symlink_metadata(&plan.root).await {
        Ok(metadata) if metadata.file_type().is_dir() => {
            match fs::symlink_metadata(plan.owner_path()).await {
                Ok(_) => {
                    let root_identity = inspect_file_identity(&plan.root).await?;
                    let artifact = OwnedArtifact {
                        plan,
                        root_identity,
                    };
                    validate_owned_artifact(&artifact).await?;
                    Ok(artifact)
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    remove_incomplete_empty_artifact(&plan).await?;
                    create_owned_artifact(plan).await
                }
                Err(source) => Err(RecoverableTransferError::file_system(
                    "read transfer owner marker metadata",
                    &plan.owner_path(),
                    source,
                )),
            }
        }
        Ok(_) => Err(RecoverableTransferError::artifact_ownership(
            &plan.root,
            "artifact path is not a directory",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            create_owned_artifact(plan).await
        }
        Err(source) => Err(RecoverableTransferError::file_system(
            "read transfer artifact metadata",
            &plan.root,
            source,
        )),
    }
}

#[cfg(unix)]
async fn set_owner_only_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).await
}

pub async fn validate_owned_artifact(
    artifact: &OwnedArtifact,
) -> Result<(), RecoverableTransferError> {
    let actual_identity = inspect_file_identity(&artifact.plan.root).await?;
    if actual_identity.object_kind != FileObjectKind::Directory
        || !actual_identity.same_object(&artifact.root_identity)
    {
        return Err(RecoverableTransferError::artifact_ownership(
            &artifact.plan.root,
            "artifact directory identity changed",
        ));
    }
    let owner_path = artifact.plan.owner_path();
    let expected_marker = artifact.plan.marker_bytes();
    let marker = read_owner_marker(&owner_path, expected_marker.len()).await?;
    if marker != expected_marker {
        return Err(RecoverableTransferError::artifact_ownership(
            &artifact.plan.root,
            "owner marker does not match the journal token",
        ));
    }

    let mut entries = fs::read_dir(&artifact.plan.root).await.map_err(|source| {
        RecoverableTransferError::file_system(
            "read transfer artifact directory",
            &artifact.plan.root,
            source,
        )
    })?;
    while let Some(entry) = entries.next_entry().await.map_err(|source| {
        RecoverableTransferError::file_system(
            "read transfer artifact entry",
            &artifact.plan.root,
            source,
        )
    })? {
        let name = entry.file_name();
        if name != OWNER_FILE_NAME && name != PAYLOAD_NAME && name != BACKUP_NAME {
            return Err(RecoverableTransferError::artifact_ownership(
                &artifact.plan.root,
                format!("unexpected artifact entry {name:?}"),
            ));
        }
    }
    Ok(())
}

async fn read_owner_marker(
    owner_path: &Path,
    expected_length: usize,
) -> Result<Vec<u8>, RecoverableTransferError> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    let mut owner_file = options.open(owner_path).await.map_err(|source| {
        RecoverableTransferError::file_system("open transfer owner marker", owner_path, source)
    })?;
    let metadata = owner_file.metadata().await.map_err(|source| {
        RecoverableTransferError::file_system(
            "read transfer owner marker metadata",
            owner_path,
            source,
        )
    })?;
    if !metadata.is_file() || metadata.len() != expected_length as u64 {
        return Err(RecoverableTransferError::artifact_ownership(
            owner_path,
            "owner marker is not the expected regular file",
        ));
    }
    let mut marker = Vec::with_capacity(expected_length);
    owner_file
        .read_to_end(&mut marker)
        .await
        .map_err(|source| {
            RecoverableTransferError::file_system("read transfer owner marker", owner_path, source)
        })?;
    Ok(marker)
}

pub async fn remove_owned_artifact(
    artifact: &OwnedArtifact,
) -> Result<(), RecoverableTransferError> {
    validate_owned_artifact(artifact).await?;
    let backup_path = artifact.plan.backup_path();
    match fs::symlink_metadata(&backup_path).await {
        Ok(_) => {
            return Err(RecoverableTransferError::artifact_ownership(
                &artifact.plan.root,
                format!("staging cleanup cannot remove backup entry at {backup_path:?}"),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(RecoverableTransferError::file_system(
                "read transfer backup before staging cleanup",
                &backup_path,
                source,
            ));
        }
    }
    remove_owned_entry_if_exists(&artifact.plan.payload_path()).await?;
    validate_owned_artifact(artifact).await?;
    remove_owned_artifact_root(artifact).await
}

pub async fn remove_empty_owned_artifact(
    artifact: &OwnedArtifact,
) -> Result<(), RecoverableTransferError> {
    validate_owned_artifact(artifact).await?;
    for path in [artifact.plan.payload_path(), artifact.plan.backup_path()] {
        match fs::symlink_metadata(&path).await {
            Ok(_) => {
                return Err(RecoverableTransferError::artifact_ownership(
                    &artifact.plan.root,
                    format!("owned cleanup entry still exists at {path:?}"),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(RecoverableTransferError::file_system(
                    "read empty transfer artifact entry",
                    &path,
                    source,
                ));
            }
        }
    }
    remove_owned_artifact_root(artifact).await
}

async fn remove_owned_artifact_root(
    artifact: &OwnedArtifact,
) -> Result<(), RecoverableTransferError> {
    let owner_path = artifact.plan.owner_path();
    fs::remove_file(&owner_path).await.map_err(|source| {
        RecoverableTransferError::file_system("remove transfer owner marker", &owner_path, source)
    })?;
    sync_directory_in_place(&artifact.plan.root).await?;
    fs::remove_dir(&artifact.plan.root)
        .await
        .map_err(|source| {
            RecoverableTransferError::file_system(
                "remove empty transfer artifact",
                &artifact.plan.root,
                source,
            )
        })?;
    sync_parent_in_place(&artifact.plan.root).await
}

async fn remove_owned_entry_if_exists(path: &Path) -> Result<(), RecoverableTransferError> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(RecoverableTransferError::file_system(
                "read owned transfer entry",
                path,
                source,
            ));
        }
    };
    let removal = if metadata.file_type().is_dir() {
        fs::remove_dir_all(path).await
    } else {
        fs::remove_file(path).await
    };
    removal.map_err(|source| {
        RecoverableTransferError::file_system("remove owned transfer entry", path, source)
    })
}

pub async fn remove_owned_artifact_if_exists(
    artifact: &OwnedArtifact,
) -> Result<(), RecoverableTransferError> {
    match fs::symlink_metadata(&artifact.plan.root).await {
        Ok(metadata)
            if metadata.file_type().is_dir()
                && fs::symlink_metadata(artifact.plan.owner_path())
                    .await
                    .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            remove_incomplete_empty_artifact(&artifact.plan).await
        }
        Ok(_) => remove_owned_artifact(artifact).await,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(RecoverableTransferError::file_system(
            "read transfer artifact metadata",
            &artifact.plan.root,
            source,
        )),
    }
}

pub async fn remove_incomplete_empty_artifact(
    plan: &OwnedArtifactPlan,
) -> Result<(), RecoverableTransferError> {
    let metadata = match fs::symlink_metadata(&plan.root).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(RecoverableTransferError::file_system(
                "read incomplete transfer artifact",
                &plan.root,
                source,
            ));
        }
    };
    if !metadata.file_type().is_dir() {
        return Err(RecoverableTransferError::artifact_ownership(
            &plan.root,
            "incomplete artifact path is not a directory",
        ));
    }
    let mut entries = fs::read_dir(&plan.root).await.map_err(|source| {
        RecoverableTransferError::file_system(
            "read incomplete transfer artifact",
            &plan.root,
            source,
        )
    })?;
    if entries
        .next_entry()
        .await
        .map_err(|source| {
            RecoverableTransferError::file_system(
                "read incomplete transfer artifact entry",
                &plan.root,
                source,
            )
        })?
        .is_some()
    {
        return Err(RecoverableTransferError::artifact_ownership(
            &plan.root,
            "incomplete artifact is not empty and has no valid owner marker",
        ));
    }
    fs::remove_dir(&plan.root).await.map_err(|source| {
        RecoverableTransferError::file_system(
            "remove incomplete transfer artifact",
            &plan.root,
            source,
        )
    })?;
    sync_parent_in_place(&plan.root).await
}

async fn sync_directory_in_place(path: &Path) -> Result<(), RecoverableTransferError> {
    let work_path = path.to_path_buf();
    let error_path = work_path.clone();
    tokio::task::spawn_blocking(move || {
        std::fs::File::open(&work_path)
            .and_then(|directory| directory.sync_all())
            .map_err(|source| {
                RecoverableTransferError::file_system("sync directory", &work_path, source)
            })
    })
    .await
    .map_err(|join_error| {
        RecoverableTransferError::file_system(
            "join directory sync task for",
            &error_path,
            std::io::Error::other(join_error),
        )
    })?
}

async fn sync_parent_in_place(path: &Path) -> Result<(), RecoverableTransferError> {
    let work_path = path.to_path_buf();
    let error_path = work_path.clone();
    tokio::task::spawn_blocking(move || sync_parent_blocking(&work_path))
        .await
        .map_err(|join_error| {
            RecoverableTransferError::file_system(
                "join parent sync task for",
                &error_path,
                std::io::Error::other(join_error),
            )
        })?
}
