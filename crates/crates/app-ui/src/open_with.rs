use std::path::PathBuf;

use desktop_linux::{OpenWithApplication, OpenWithApplicationList, OpenWithLaunchMode};

#[derive(Debug, Clone)]
pub(crate) enum OpenWithState {
    Loading {
        path: PathBuf,
        fallback_error: Option<String>,
    },
    Ready {
        application_list: OpenWithApplicationList,
        set_as_default: bool,
        fallback_error: Option<String>,
    },
}

impl OpenWithState {
    pub(crate) fn loading(path: PathBuf, fallback_error: Option<String>) -> Self {
        Self::Loading {
            path,
            fallback_error,
        }
    }

    pub(crate) fn path(&self) -> &PathBuf {
        match self {
            Self::Loading { path, .. } => path,
            Self::Ready {
                application_list, ..
            } => &application_list.path,
        }
    }

    pub(crate) fn fallback_error(&self) -> Option<&str> {
        match self {
            Self::Loading { fallback_error, .. } | Self::Ready { fallback_error, .. } => {
                fallback_error.as_deref()
            }
        }
    }

    pub(crate) fn set_default_selected(&self) -> bool {
        match self {
            Self::Loading { .. } => false,
            Self::Ready { set_as_default, .. } => *set_as_default,
        }
    }

    pub(crate) fn applications(&self) -> &[OpenWithApplication] {
        match self {
            Self::Loading { .. } => &[],
            Self::Ready {
                application_list, ..
            } => &application_list.applications,
        }
    }

    pub(crate) fn mime_type(&self) -> Option<&str> {
        match self {
            Self::Loading { .. } => None,
            Self::Ready {
                application_list, ..
            } => Some(application_list.mime_type.as_str()),
        }
    }

    pub(crate) fn launch_mode(&self) -> OpenWithLaunchMode {
        if self.set_default_selected() {
            OpenWithLaunchMode::SetAsDefault
        } else {
            OpenWithLaunchMode::OpenOnce
        }
    }

    pub(crate) fn select_default_application_setting(&mut self, selected: bool) {
        if let Self::Ready { set_as_default, .. } = self {
            *set_as_default = selected;
        }
    }

    pub(crate) fn accept_application_list(
        &mut self,
        application_list: OpenWithApplicationList,
    ) -> bool {
        let Self::Loading {
            path,
            fallback_error,
        } = self
        else {
            return false;
        };
        if *path != application_list.path {
            return false;
        }

        *self = Self::Ready {
            application_list,
            set_as_default: false,
            fallback_error: fallback_error.take(),
        };
        true
    }
}
