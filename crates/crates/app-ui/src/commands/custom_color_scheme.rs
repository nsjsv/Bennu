use std::path::PathBuf;

use iced::Task;

use crate::model::Message;

pub(crate) fn custom_color_scheme_import_command() -> Task<Message> {
    Task::perform(
        choose_custom_color_scheme_file(),
        Message::CustomColorSchemeImportCompleted,
    )
}

async fn choose_custom_color_scheme_file() -> Result<Option<String>, String> {
    let title = crate::localization::translate_current("Import custom color scheme");
    let accept_label = crate::localization::translate_current("Import");
    let request = ashpd::desktop::file_chooser::SelectedFiles::open_file()
        .title(title.as_str())
        .accept_label(accept_label.as_str())
        .modal(true)
        .multiple(false)
        .filter(ashpd::desktop::file_chooser::FileFilter::new("JSON").glob("*.json"))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let selected = match request.response() {
        Ok(selected) => selected,
        Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)) => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let Some(path) = selected_custom_color_scheme_path(selected.uris())? else {
        return Ok(None);
    };
    tokio::fs::read_to_string(&path)
        .await
        .map(Some)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))
}

fn selected_custom_color_scheme_path(uris: &[url::Url]) -> Result<Option<PathBuf>, String> {
    uris.first()
        .map(|uri| {
            uri.to_file_path()
                .map_err(|_| "the selected color scheme is not a local file".to_owned())
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_path_requires_a_local_file_uri() {
        assert_eq!(
            selected_custom_color_scheme_path(&[]).expect("empty selection"),
            None
        );
        assert_eq!(
            selected_custom_color_scheme_path(
                &[url::Url::parse("file:///tmp/theme.json").unwrap()]
            )
            .expect("local path"),
            Some(PathBuf::from("/tmp/theme.json"))
        );
        assert!(selected_custom_color_scheme_path(&[url::Url::parse(
            "https://example.com/theme.json"
        )
        .unwrap()])
        .is_err());
    }
}
