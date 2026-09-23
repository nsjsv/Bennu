use std::os::unix::fs::PermissionsExt;

use super::*;

#[test]
fn office_workspace_uses_private_session_directories_and_a_real_file_url() {
    let workspace = OfficeDocumentPreviewWorkspace::create().unwrap();

    for path in workspace.private_directories() {
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "{}", path.display());
    }
    let profile_path = workspace.profile_url().to_file_path().unwrap();
    assert_eq!(
        workspace.profile_url(),
        &Url::from_directory_path(profile_path).unwrap()
    );
    assert_eq!(workspace.profile_url().scheme(), "file");
}

#[tokio::test]
async fn converted_pdf_keeps_the_office_tempdir_alive() {
    let workspace = OfficeDocumentPreviewWorkspace::create().unwrap();
    let root = workspace.root.path().to_path_buf();
    let pdf = workspace.output_dir().join("converted.pdf");
    std::fs::write(&pdf, b"pdf").unwrap();

    let document = workspace
        .into_document_workspace(pdf.clone())
        .await
        .expect("finished workspace");
    assert_eq!(document.pdf_path(), pdf);
    assert!(root.exists());

    drop(document);
    assert!(!root.exists());
}
