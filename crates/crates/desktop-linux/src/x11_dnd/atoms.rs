use x11rb::protocol::xproto::{Atom, AtomEnum, ConnectionExt as _, GetPropertyReply, Window};
use x11rb::rust_connection::RustConnection;

use super::runtime::X11DndError;

pub(super) const XDND_VERSION: u32 = 5;

#[derive(Debug, Clone)]
pub(super) struct X11Atoms {
    pub xdnd_aware: Atom,
    pub xdnd_proxy: Atom,
    pub xdnd_enter: Atom,
    pub xdnd_position: Atom,
    pub xdnd_leave: Atom,
    pub xdnd_drop: Atom,
    pub xdnd_status: Atom,
    pub xdnd_finished: Atom,
    pub xdnd_selection: Atom,
    pub xdnd_action_copy: Atom,
    pub xdnd_type_list: Atom,
    pub text_uri_list: Atom,
    pub incr: Atom,
}

impl X11Atoms {
    pub fn intern(conn: &RustConnection) -> Result<Self, X11DndError> {
        Ok(Self {
            xdnd_aware: intern(conn, b"XdndAware")?,
            xdnd_proxy: intern(conn, b"XdndProxy")?,
            xdnd_enter: intern(conn, b"XdndEnter")?,
            xdnd_position: intern(conn, b"XdndPosition")?,
            xdnd_leave: intern(conn, b"XdndLeave")?,
            xdnd_drop: intern(conn, b"XdndDrop")?,
            xdnd_status: intern(conn, b"XdndStatus")?,
            xdnd_finished: intern(conn, b"XdndFinished")?,
            xdnd_selection: intern(conn, b"XdndSelection")?,
            xdnd_action_copy: intern(conn, b"XdndActionCopy")?,
            xdnd_type_list: intern(conn, b"XdndTypeList")?,
            text_uri_list: intern(conn, b"text/uri-list")?,
            incr: intern(conn, b"INCR")?,
        })
    }

    pub fn selection_property(
        &self,
        conn: &RustConnection,
        session_id: super::X11FileDropTargetSessionId,
    ) -> Result<Atom, X11DndError> {
        intern(
            conn,
            format!("_FILE_MANAGER_XDND_SELECTION_{session_id}").as_bytes(),
        )
    }
}

fn intern(conn: &RustConnection, name: &[u8]) -> Result<Atom, X11DndError> {
    conn.intern_atom(false, name)
        .map_err(|error| X11DndError::request("intern-atom", error))?
        .reply()
        .map(|reply| reply.atom)
        .map_err(|error| X11DndError::reply("intern-atom", error))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PropertyValue {
    pub type_: Atom,
    pub format: u8,
    pub value: Vec<u8>,
}

impl PropertyValue {
    pub fn from_reply(reply: GetPropertyReply) -> Option<Self> {
        (reply.type_ != AtomEnum::NONE.into()).then_some(Self {
            type_: reply.type_,
            format: reply.format,
            value: reply.value,
        })
    }

    pub fn single_u32(&self, expected_type: Atom) -> Option<u32> {
        if self.type_ != expected_type || self.format != 32 || self.value.len() != 4 {
            return None;
        }
        Some(u32::from_ne_bytes(self.value.as_slice().try_into().ok()?))
    }
}

pub(super) fn read_property(
    conn: &RustConnection,
    window: Window,
    property: Atom,
) -> Result<Option<PropertyValue>, X11DndError> {
    let reply = conn
        .get_property(false, window, property, AtomEnum::ANY, 0, 1024)
        .map_err(|error| X11DndError::request("get-property", error))?
        .reply()
        .map_err(|error| X11DndError::reply("get-property", error))?;
    if reply.bytes_after != 0 {
        return Err(X11DndError::Setup {
            stage: "get-property",
            details: "property reply was incomplete".to_owned(),
        });
    }
    Ok(PropertyValue::from_reply(reply))
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProxyLifecycleStep {
    CreateProxy,
    PublishProxySelfReference,
    PublishProxyAware,
    VerifyProxy,
    PublishMainAware,
    PublishMainProxy,
    DeleteMainProxy,
    RestoreMainAware,
    DestroyProxy,
}

#[cfg(test)]
pub(super) const SETUP_ORDER: &[ProxyLifecycleStep] = &[
    ProxyLifecycleStep::CreateProxy,
    ProxyLifecycleStep::PublishProxySelfReference,
    ProxyLifecycleStep::PublishProxyAware,
    ProxyLifecycleStep::VerifyProxy,
    ProxyLifecycleStep::PublishMainAware,
    ProxyLifecycleStep::PublishMainProxy,
];

#[cfg(test)]
pub(super) const TEARDOWN_ORDER: &[ProxyLifecycleStep] = &[
    ProxyLifecycleStep::DeleteMainProxy,
    ProxyLifecycleStep::RestoreMainAware,
    ProxyLifecycleStep::DestroyProxy,
];
