use std::path::PathBuf;

/// 门户目录选择器；取消返回 `Ok(None)`。
pub(crate) async fn choose_directory(title: String) -> Result<Option<PathBuf>, String> {
    let accept_label = tr!("Select");
    let request = ashpd::desktop::file_chooser::SelectedFiles::open_file()
        .title(title.as_str())
        .accept_label(accept_label.as_str())
        .modal(true)
        .directory(true)
        .multiple(false)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let selected = match request.response() {
        Ok(selected) => selected,
        Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)) => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    selected_directory(selected.uris())
}

fn selected_directory(uris: &[url::Url]) -> Result<Option<PathBuf>, String> {
    uris.first()
        .map(|uri| {
            uri.to_file_path()
                .map_err(|_| tr!("the selected location is not a local directory"))
        })
        .transpose()
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::ffi::OsStringExt;
    use std::path::PathBuf;

    use url::Url;

    use super::selected_directory;

    #[test]
    fn selected_directory_preserves_native_non_utf8_paths() {
        let path = PathBuf::from(std::ffi::OsString::from_vec(
            b"/tmp/bennu-search-\xff".to_vec(),
        ));
        let uri = Url::from_file_path(&path).unwrap();

        assert_eq!(selected_directory(&[]).unwrap(), None);
        assert_eq!(selected_directory(&[uri]).unwrap(), Some(path.clone()));
        assert!(selected_directory(&[Url::parse("https://example.com/").unwrap()]).is_err());
    }
}
