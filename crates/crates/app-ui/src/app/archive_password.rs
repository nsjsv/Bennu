use std::fmt;

use file_core::ArchivePassword;

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ArchivePasswordDraft(String);

impl ArchivePasswordDraft {
    pub(crate) fn new(password: String) -> Self {
        Self(password)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn to_archive_password(&self) -> Option<ArchivePassword> {
        ArchivePassword::new(self.0.clone())
    }
}

impl fmt::Debug for ArchivePasswordDraft {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            formatter.write_str("ArchivePasswordDraft(<empty>)")
        } else {
            formatter.write_str("ArchivePasswordDraft(<redacted>)")
        }
    }
}
