use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, CreateWindowAux, EventMask,
    PropMode, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use super::atoms::{read_property, PropertyValue, X11Atoms, XDND_VERSION};
use super::runtime::X11DndError;

pub(super) struct ProxyLifecycle {
    pub(super) main_window: Window,
    proxy_window: Option<Window>,
    original_main_aware: Option<PropertyValue>,
    main_aware_published: bool,
    main_proxy_published: bool,
}

impl ProxyLifecycle {
    pub(super) fn install(
        conn: &RustConnection,
        root: Window,
        main_window: Window,
        atoms: &X11Atoms,
    ) -> Result<Self, X11DndError> {
        let original_main_aware = read_property(conn, main_window, atoms.xdnd_aware)?;
        if read_property(conn, main_window, atoms.xdnd_proxy)?.is_some() {
            return Err(X11DndError::Setup {
                stage: "main-proxy",
                details: "main window already publishes XdndProxy".to_owned(),
            });
        }

        let mut lifecycle = Self {
            main_window,
            proxy_window: None,
            original_main_aware,
            main_aware_published: false,
            main_proxy_published: false,
        };
        let install_result = (|| {
            let proxy_window = conn
                .generate_id()
                .map_err(|error| X11DndError::request("proxy-id", error))?;
            conn.create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                proxy_window,
                root,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_ONLY,
                x11rb::COPY_FROM_PARENT,
                &CreateWindowAux::new().event_mask(EventMask::STRUCTURE_NOTIFY),
            )
            .map_err(|error| X11DndError::request("create-proxy", error))?
            .check()
            .map_err(|error| X11DndError::reply("create-proxy", error))?;
            lifecycle.proxy_window = Some(proxy_window);

            checked_property32(
                conn,
                proxy_window,
                atoms.xdnd_proxy,
                AtomEnum::WINDOW,
                &[proxy_window],
                "proxy-self-reference",
            )?;
            checked_property32(
                conn,
                proxy_window,
                atoms.xdnd_aware,
                AtomEnum::ATOM,
                &[XDND_VERSION],
                "proxy-aware",
            )?;
            conn.flush()
                .map_err(|error| X11DndError::request("verify-proxy", error))?;

            let proxy_reference = read_property(conn, proxy_window, atoms.xdnd_proxy)?;
            let proxy_aware = read_property(conn, proxy_window, atoms.xdnd_aware)?;
            if proxy_reference
                .as_ref()
                .and_then(|value| value.single_u32(AtomEnum::WINDOW.into()))
                != Some(proxy_window)
                || proxy_aware
                    .as_ref()
                    .and_then(|value| value.single_u32(AtomEnum::ATOM.into()))
                    != Some(XDND_VERSION)
            {
                return Err(X11DndError::Setup {
                    stage: "verify-proxy",
                    details: "proxy properties do not satisfy XDND v5".to_owned(),
                });
            }

            conn.change_window_attributes(
                main_window,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY),
            )
            .map_err(|error| X11DndError::request("watch-main-window", error))?
            .check()
            .map_err(|error| X11DndError::reply("watch-main-window", error))?;
            checked_property32(
                conn,
                main_window,
                atoms.xdnd_aware,
                AtomEnum::ATOM,
                &[XDND_VERSION],
                "main-aware",
            )?;
            lifecycle.main_aware_published = true;
            checked_property32(
                conn,
                main_window,
                atoms.xdnd_proxy,
                AtomEnum::WINDOW,
                &[proxy_window],
                "main-proxy",
            )?;
            lifecycle.main_proxy_published = true;
            conn.flush()
                .map_err(|error| X11DndError::request("publish-main-proxy", error))?;
            Ok(())
        })();

        if let Err(error) = install_result {
            lifecycle.teardown(conn, atoms);
            return Err(error);
        }
        Ok(lifecycle)
    }

    pub(super) fn proxy_window(&self) -> Window {
        self.proxy_window.expect("installed X11 proxy window")
    }

    pub(super) fn unpublish_main_proxy(&mut self, conn: &RustConnection, atoms: &X11Atoms) {
        if self.main_proxy_published {
            let _ = conn.delete_property(self.main_window, atoms.xdnd_proxy);
            let _ = conn.flush();
            self.main_proxy_published = false;
        }
    }

    pub(super) fn teardown(&mut self, conn: &RustConnection, atoms: &X11Atoms) {
        self.unpublish_main_proxy(conn, atoms);
        if self.main_aware_published {
            match &self.original_main_aware {
                Some(property) => {
                    let element_len = match property.format {
                        8 => property.value.len(),
                        16 => property.value.len() / 2,
                        32 => property.value.len() / 4,
                        _ => 0,
                    };
                    if element_len > 0 || property.value.is_empty() {
                        let _ = conn.change_property(
                            PropMode::REPLACE,
                            self.main_window,
                            atoms.xdnd_aware,
                            property.type_,
                            property.format,
                            element_len as u32,
                            &property.value,
                        );
                    }
                }
                None => {
                    let _ = conn.delete_property(self.main_window, atoms.xdnd_aware);
                }
            }
            self.main_aware_published = false;
        }
        if let Some(proxy_window) = self.proxy_window.take() {
            let _ = conn.destroy_window(proxy_window);
        }
        let _ = conn.flush();
    }
}

fn checked_property32(
    conn: &RustConnection,
    window: Window,
    property: Atom,
    type_: AtomEnum,
    value: &[u32],
    stage: &'static str,
) -> Result<(), X11DndError> {
    conn.change_property32(PropMode::REPLACE, window, property, type_, value)
        .map_err(|error| X11DndError::request(stage, error))?
        .check()
        .map_err(|error| X11DndError::reply(stage, error))
}
