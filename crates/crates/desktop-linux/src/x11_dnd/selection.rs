use std::path::PathBuf;

use x11rb::protocol::xproto::Atom;

use crate::parse_file_uri_list;

pub(super) const MAX_SELECTION_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PropertyPayload {
    pub type_: Atom,
    pub format: u8,
    pub bytes_after: u32,
    pub value: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SelectionProgress {
    ReadingIncr,
    Complete(Vec<PathBuf>),
}

#[derive(Debug, Clone)]
pub(super) struct SelectionTransfer {
    expected_type: Atom,
    incr_type: Atom,
    data: Vec<u8>,
    incremental: bool,
    terminal: bool,
}

impl SelectionTransfer {
    pub fn new(expected_type: Atom, incr_type: Atom) -> Self {
        Self {
            expected_type,
            incr_type,
            data: Vec::new(),
            incremental: false,
            terminal: false,
        }
    }

    pub fn accept_initial(
        &mut self,
        payload: PropertyPayload,
    ) -> Result<SelectionProgress, String> {
        self.ensure_active()?;
        if payload.type_ == self.incr_type {
            if payload.format != 32 || payload.value.len() != 4 || payload.bytes_after != 0 {
                return self.fail("invalid INCR selection header");
            }
            let announced = u32::from_ne_bytes(
                payload
                    .value
                    .as_slice()
                    .try_into()
                    .map_err(|_| "invalid INCR selection header".to_owned())?,
            ) as usize;
            if announced > MAX_SELECTION_BYTES {
                return self.fail("X11 file drop payload exceeds the size limit");
            }
            self.incremental = true;
            return Ok(SelectionProgress::ReadingIncr);
        }
        self.validate_data_property(&payload)?;
        self.append(&payload.value)?;
        self.complete()
    }

    pub fn accept_chunk(&mut self, payload: PropertyPayload) -> Result<SelectionProgress, String> {
        self.ensure_active()?;
        if !self.incremental {
            return self.fail("unexpected X11 selection property update");
        }
        self.validate_data_property(&payload)?;
        if payload.value.is_empty() {
            return self.complete();
        }
        self.append(&payload.value)?;
        Ok(SelectionProgress::ReadingIncr)
    }

    #[cfg(test)]
    pub fn fail_terminal(&mut self, details: impl Into<String>) -> Option<String> {
        if self.terminal {
            None
        } else {
            self.terminal = true;
            Some(details.into())
        }
    }

    fn ensure_active(&self) -> Result<(), String> {
        if self.terminal {
            Err("X11 selection transfer already terminated".to_owned())
        } else {
            Ok(())
        }
    }

    fn validate_data_property(&mut self, payload: &PropertyPayload) -> Result<(), String> {
        if payload.type_ != self.expected_type || payload.format != 8 || payload.bytes_after != 0 {
            return self.fail("invalid text/uri-list selection property");
        }
        Ok(())
    }

    fn append(&mut self, chunk: &[u8]) -> Result<(), String> {
        if self.data.len().saturating_add(chunk.len()) > MAX_SELECTION_BYTES {
            return self.fail("X11 file drop payload exceeds the size limit");
        }
        self.data.extend_from_slice(chunk);
        Ok(())
    }

    fn complete(&mut self) -> Result<SelectionProgress, String> {
        let parsed = (|| {
            let text = std::str::from_utf8(&self.data)
                .map_err(|error| format!("X11 file drop payload is not UTF-8: {error}"))?;
            let paths = parse_file_uri_list(text)
                .map_err(|error| format!("X11 file drop URI list is invalid: {error}"))?;
            if paths.is_empty() {
                return Err("X11 file drop URI list is empty".to_owned());
            }
            Ok(paths)
        })();
        match parsed {
            Ok(paths) => {
                self.terminal = true;
                Ok(SelectionProgress::Complete(paths))
            }
            Err(details) => self.fail(details),
        }
    }

    fn fail<T>(&mut self, details: impl Into<String>) -> Result<T, String> {
        self.terminal = true;
        Err(details.into())
    }
}
