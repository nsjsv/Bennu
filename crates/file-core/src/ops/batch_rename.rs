use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use tokio::fs;

use crate::FileError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchRenameItem {
    pub from: PathBuf,
    pub to: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedBatchRename {
    pub from: PathBuf,
    pub to: PathBuf,
}

#[derive(Debug, Clone)]
struct BatchRenameStep {
    source: PathBuf,
    temporary: PathBuf,
    target: PathBuf,
}

pub async fn batch_rename_paths(
    items: Vec<BatchRenameItem>,
) -> Result<Vec<CompletedBatchRename>, FileError> {
    let items = validated_batch_rename_items(items).await?;
    if items.is_empty() {
        return Ok(Vec::new());
    }

    let steps = prepare_batch_rename_steps(&items).await?;
    let mut renamed_to_temporary = Vec::new();
    let mut completed_steps = Vec::new();

    for step in &steps {
        if let Err(error) = rename_for_batch(&step.source, &step.temporary).await {
            rollback_temporary_batch_renames(&renamed_to_temporary).await;
            return Err(error);
        }
        renamed_to_temporary.push((step.temporary.clone(), step.source.clone()));
    }

    for step in &steps {
        if let Err(error) = rename_for_batch(&step.temporary, &step.target).await {
            rollback_final_batch_renames(&completed_steps, &renamed_to_temporary).await;
            return Err(error);
        }
        completed_steps.push((step.target.clone(), step.source.clone()));
    }

    Ok(items
        .into_iter()
        .filter(|item| item.from != item.to)
        .map(|item| CompletedBatchRename {
            from: item.from,
            to: item.to,
        })
        .collect())
}

async fn validated_batch_rename_items(
    items: Vec<BatchRenameItem>,
) -> Result<Vec<BatchRenameItem>, FileError> {
    let mut sources = HashSet::new();
    let mut targets = HashSet::new();
    let source_set = items
        .iter()
        .map(|item| item.from.clone())
        .collect::<HashSet<_>>();

    for item in &items {
        let Some(parent) = item.from.parent() else {
            return Err(invalid_batch_rename_input(
                &item.from,
                "source path has no parent",
            ));
        };
        if item.to.parent() != Some(parent) {
            return Err(invalid_batch_rename_input(
                &item.to,
                "target must stay in the source directory",
            ));
        }
        if item.to.file_name().is_none_or(|name| name.is_empty()) {
            return Err(invalid_batch_rename_input(
                &item.to,
                "target name cannot be empty",
            ));
        }
        if !sources.insert(item.from.clone()) {
            return Err(invalid_batch_rename_input(
                &item.from,
                "source appears more than once",
            ));
        }
        if !targets.insert(item.to.clone()) {
            return Err(invalid_batch_rename_input(
                &item.to,
                "target appears more than once",
            ));
        }
        if item.from != item.to && !source_exists(&item.from).await? {
            return Err(FileError::Metadata {
                path: item.from.clone(),
                source: io::Error::new(io::ErrorKind::NotFound, "source does not exist"),
            });
        }
        if item.from != item.to && !source_set.contains(&item.to) && path_exists(&item.to).await? {
            return Err(FileError::Rename {
                from: item.from.clone(),
                to: item.to.clone(),
                source: io::Error::new(io::ErrorKind::AlreadyExists, "target already exists"),
            });
        }
    }

    Ok(items)
}

async fn prepare_batch_rename_steps(
    items: &[BatchRenameItem],
) -> Result<Vec<BatchRenameStep>, FileError> {
    let mut reserved = items
        .iter()
        .flat_map(|item| [item.from.clone(), item.to.clone()])
        .collect::<HashSet<_>>();
    let mut steps = Vec::new();

    for item in items.iter().filter(|item| item.from != item.to) {
        let temporary = available_batch_rename_temporary_path(&item.from, &mut reserved).await?;
        steps.push(BatchRenameStep {
            source: item.from.clone(),
            temporary,
            target: item.to.clone(),
        });
    }

    Ok(steps)
}

async fn available_batch_rename_temporary_path(
    source: &Path,
    reserved: &mut HashSet<PathBuf>,
) -> Result<PathBuf, FileError> {
    let parent = source
        .parent()
        .ok_or_else(|| invalid_batch_rename_input(source, "source path has no parent"))?;
    source
        .file_name()
        .ok_or_else(|| invalid_batch_rename_input(source, "source name cannot be empty"))?;

    for index in 0..1000 {
        let candidate = parent.join(format!(".bennu-batch-rename-{index}.tmp"));
        if reserved.contains(&candidate) {
            continue;
        }
        if path_exists(&candidate).await? {
            continue;
        }
        reserved.insert(candidate.clone());
        return Ok(candidate);
    }

    Err(invalid_batch_rename_input(
        source,
        "could not reserve a temporary rename path",
    ))
}

async fn rename_for_batch(from: &Path, to: &Path) -> Result<(), FileError> {
    fs::rename(from, to)
        .await
        .map_err(|source| FileError::Rename {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
            source,
        })
}

async fn source_exists(path: &Path) -> Result<bool, FileError> {
    fs::symlink_metadata(path)
        .await
        .map(|_| true)
        .or_else(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                Ok(false)
            } else {
                Err(FileError::Metadata {
                    path: path.to_path_buf(),
                    source,
                })
            }
        })
}

async fn path_exists(path: &Path) -> Result<bool, FileError> {
    fs::symlink_metadata(path)
        .await
        .map(|_| true)
        .or_else(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                Ok(false)
            } else {
                Err(FileError::Metadata {
                    path: path.to_path_buf(),
                    source,
                })
            }
        })
}

async fn rollback_temporary_batch_renames(renames: &[(PathBuf, PathBuf)]) {
    for (from, to) in renames.iter().rev() {
        let _ = fs::rename(from, to).await;
    }
}

async fn rollback_final_batch_renames(
    completed_steps: &[(PathBuf, PathBuf)],
    temporary_steps: &[(PathBuf, PathBuf)],
) {
    let mut reserved = temporary_steps
        .iter()
        .flat_map(|(temporary_path, original_path)| [temporary_path.clone(), original_path.clone()])
        .chain(
            completed_steps
                .iter()
                .flat_map(|(final_path, original_path)| {
                    [final_path.clone(), original_path.clone()]
                }),
        )
        .collect::<HashSet<_>>();
    let mut completed_rollback_steps = Vec::new();

    for (final_path, original_path) in completed_steps.iter().rev() {
        let Ok(rollback_temporary) =
            available_batch_rename_temporary_path(final_path, &mut reserved).await
        else {
            continue;
        };
        if fs::rename(final_path, &rollback_temporary).await.is_ok() {
            completed_rollback_steps.push((rollback_temporary, original_path.clone()));
        }
    }

    let completed_originals = completed_steps
        .iter()
        .map(|(_, original_path)| original_path.clone())
        .collect::<HashSet<_>>();
    for (temporary_path, original_path) in temporary_steps {
        if completed_originals.contains(original_path) {
            continue;
        }
        let _ = fs::rename(temporary_path, original_path).await;
    }
    for (temporary_path, original_path) in completed_rollback_steps {
        let _ = fs::rename(temporary_path, original_path).await;
    }
}

fn invalid_batch_rename_input(path: &Path, message: &'static str) -> FileError {
    FileError::InvalidInput {
        path: path.to_path_buf(),
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn rollback_final_batch_renames_restores_completed_and_temporary_paths() {
        let dir = tempdir().unwrap();
        let first = dir.path().join("first.txt");
        let second = dir.path().join("second.txt");
        let final_first = dir.path().join("alpha.txt");
        let temp_second = dir.path().join(".bennu-batch-rename-1.tmp");
        fs::write(&final_first, b"one").unwrap();
        fs::write(&temp_second, b"two").unwrap();

        rollback_final_batch_renames(
            &[(final_first.clone(), first.clone())],
            &[
                (
                    dir.path().join(".bennu-batch-rename-0.tmp"),
                    first.clone(),
                ),
                (temp_second.clone(), second.clone()),
            ],
        )
        .await;

        assert_eq!(fs::read(&first).unwrap(), b"one");
        assert_eq!(fs::read(&second).unwrap(), b"two");
        assert!(!final_first.exists());
        assert!(!temp_second.exists());
    }

    #[tokio::test]
    async fn rollback_final_batch_renames_restores_cycles_without_overwriting() {
        let dir = tempdir().unwrap();
        let first = dir.path().join("first.txt");
        let second = dir.path().join("second.txt");
        let third = dir.path().join("third.txt");
        let temp_first = dir.path().join(".bennu-batch-rename-0.tmp");
        let temp_second = dir.path().join(".bennu-batch-rename-1.tmp");
        let temp_third = dir.path().join(".bennu-batch-rename-2.tmp");
        fs::write(&second, b"one").unwrap();
        fs::write(&third, b"two").unwrap();
        fs::write(&temp_third, b"three").unwrap();

        rollback_final_batch_renames(
            &[
                (second.clone(), first.clone()),
                (third.clone(), second.clone()),
            ],
            &[
                (temp_first.clone(), first.clone()),
                (temp_second.clone(), second.clone()),
                (temp_third.clone(), third.clone()),
            ],
        )
        .await;

        assert_eq!(fs::read(&first).unwrap(), b"one");
        assert_eq!(fs::read(&second).unwrap(), b"two");
        assert_eq!(fs::read(&third).unwrap(), b"three");
        assert!(!temp_third.exists());
    }
}
