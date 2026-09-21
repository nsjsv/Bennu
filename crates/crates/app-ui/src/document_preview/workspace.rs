use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use url::Url;

use super::model::DocumentPageRequestKey;

#[derive(Debug)]
pub(crate) struct DocumentPreviewWorkspace {
    root: TempDir,
    pdf_path: PathBuf,
}

impl DocumentPreviewWorkspace {
    pub(crate) fn create_for_pdf(pdf_path: PathBuf) -> Result<Self, io::Error> {
        let root = create_private_tempdir()?;
        Ok(Self { root, pdf_path })
    }

    pub(crate) fn pdf_path(&self) -> &Path {
        &self.pdf_path
    }

    pub(crate) fn page_output_prefix(&self, key: &DocumentPageRequestKey) -> PathBuf {
        self.root.path().join(format!(
            "page-{}-{}-{}-{}",
            key.render.request.document_generation,
            key.render.render_generation,
            key.render.width_bucket,
            key.page_index + 1
        ))
    }

    #[cfg(test)]
    pub(crate) fn root_path(&self) -> &Path {
        self.root.path()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DirectoryIdentity {
    device: u64,
    inode: u64,
    is_directory: bool,
}

impl DirectoryIdentity {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            is_directory: metadata.file_type().is_dir(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct OfficeDocumentPreviewWorkspace {
    root: TempDir,
    profile_url: Url,
    output: PathBuf,
    output_identity: DirectoryIdentity,
    home: PathBuf,
    temporary: PathBuf,
    xdg_config: PathBuf,
    xdg_cache: PathBuf,
    xdg_data: PathBuf,
}

impl OfficeDocumentPreviewWorkspace {
    pub(crate) fn create() -> Result<Self, io::Error> {
        let root = create_private_tempdir()?;
        let profile = root.path().join("profile");
        let output = root.path().join("output");
        let home = root.path().join("home");
        let temporary = root.path().join("tmp");
        let xdg_config = root.path().join("xdg-config");
        let xdg_cache = root.path().join("xdg-cache");
        let xdg_data = root.path().join("xdg-data");
        for path in [
            &profile,
            &output,
            &home,
            &temporary,
            &xdg_config,
            &xdg_cache,
            &xdg_data,
        ] {
            create_private_directory(path)?;
        }
        let profile_url = Url::from_directory_path(&profile).map_err(|()| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "could not represent LibreOffice profile as a file URL",
            )
        })?;
        let output_metadata = fs::symlink_metadata(&output)?;
        let output_identity = DirectoryIdentity::from_metadata(&output_metadata);
        if !output_identity.is_directory {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "LibreOffice output path is not a directory",
            ));
        }

        Ok(Self {
            root,
            profile_url,
            output,
            output_identity,
            home,
            temporary,
            xdg_config,
            xdg_cache,
            xdg_data,
        })
    }

    pub(crate) fn profile_url(&self) -> &Url {
        &self.profile_url
    }

    pub(crate) fn output_dir(&self) -> &Path {
        &self.output
    }

    pub(crate) fn home_dir(&self) -> &Path {
        &self.home
    }

    pub(crate) fn temporary_dir(&self) -> &Path {
        &self.temporary
    }

    pub(crate) fn xdg_config_dir(&self) -> &Path {
        &self.xdg_config
    }

    pub(crate) fn xdg_cache_dir(&self) -> &Path {
        &self.xdg_cache
    }

    pub(crate) fn xdg_data_dir(&self) -> &Path {
        &self.xdg_data
    }

    pub(crate) async fn output_directory_identity_is_current(&self) -> io::Result<bool> {
        let metadata = tokio::fs::symlink_metadata(&self.output).await?;
        Ok(DirectoryIdentity::from_metadata(&metadata) == self.output_identity)
    }

    pub(crate) async fn into_document_workspace(
        self,
        pdf_path: PathBuf,
    ) -> Result<DocumentPreviewWorkspace, String> {
        // 转换程序控制 output 内容，最终转移前必须重新证明目录对象仍由当前会话拥有。
        if !self
            .output_directory_identity_is_current()
            .await
            .map_err(|error| format!("Could not inspect LibreOffice output directory: {error}"))?
        {
            return Err("LibreOffice output directory identity changed".to_owned());
        }
        if pdf_path.parent() != Some(self.output.as_path()) {
            return Err("LibreOffice output escaped the preview workspace".to_owned());
        }
        Ok(DocumentPreviewWorkspace {
            root: self.root,
            pdf_path,
        })
    }

    #[cfg(test)]
    pub(crate) fn private_directories(&self) -> Vec<PathBuf> {
        vec![
            self.root.path().to_path_buf(),
            self.profile_url.to_file_path().unwrap(),
            self.output.clone(),
            self.home.clone(),
            self.temporary.clone(),
            self.xdg_config.clone(),
            self.xdg_cache.clone(),
            self.xdg_data.clone(),
        ]
    }
}

fn create_private_tempdir() -> Result<TempDir, io::Error> {
    let root = tempfile::Builder::new()
        .prefix("bennu-document-preview-")
        .tempdir()?;
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700))?;
    Ok(root)
}

fn create_private_directory(path: &Path) -> Result<(), io::Error> {
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700).create(path)
}

#[cfg(test)]
#[path = "workspace/tests.rs"]
mod tests;
