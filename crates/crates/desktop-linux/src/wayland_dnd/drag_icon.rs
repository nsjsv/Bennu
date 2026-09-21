use smithay_client_toolkit::compositor::{CompositorState, Surface};
use smithay_client_toolkit::shm::slot::{ActivateSlotError, Buffer, CreateBufferError, SlotPool};
use thiserror::Error;
use wayland_client::protocol::wl_shm;
use wayland_client::QueueHandle;

use super::{WaylandFileDnd, WaylandFileDragIcon};

pub(super) const INITIAL_DRAG_ICON_POOL_BYTES: usize = 64 * 64 * 4;

pub(super) struct WaylandDragIconSurface {
    surface: Surface,
    _buffer: Buffer,
}

impl WaylandDragIconSurface {
    pub(super) fn create(
        compositor_state: &CompositorState,
        pool: &mut SlotPool,
        qh: &QueueHandle<WaylandFileDnd>,
        icon: &WaylandFileDragIcon,
    ) -> Result<Self, WaylandDragIconSurfaceError> {
        let width = icon.width() as i32;
        let height = icon.height() as i32;
        let stride = width * 4;
        let surface = Surface::from(compositor_state.create_surface(qh));
        let (buffer, canvas) = pool
            .create_buffer(width, height, stride, wl_shm::Format::Argb8888)
            .map_err(|source| WaylandDragIconSurfaceError::CreateBuffer { source })?;
        write_argb8888_pixels(icon.premultiplied_rgba(), canvas);
        buffer
            .attach_to(surface.wl_surface())
            .map_err(|source| WaylandDragIconSurfaceError::AttachBuffer { source })?;
        surface.wl_surface().damage_buffer(0, 0, width, height);
        surface.wl_surface().commit();

        Ok(Self {
            surface,
            _buffer: buffer,
        })
    }

    pub(super) fn wl_surface(&self) -> &wayland_client::protocol::wl_surface::WlSurface {
        self.surface.wl_surface()
    }
}

#[derive(Debug, Error)]
pub(super) enum WaylandDragIconSurfaceError {
    #[error("could not allocate Wayland drag icon buffer: {source}")]
    CreateBuffer {
        #[source]
        source: CreateBufferError,
    },
    #[error("could not attach Wayland drag icon buffer: {source}")]
    AttachBuffer {
        #[source]
        source: ActivateSlotError,
    },
}

fn write_argb8888_pixels(premultiplied_rgba: &[u8], argb8888: &mut [u8]) {
    for (rgba, argb) in premultiplied_rgba
        .chunks_exact(4)
        .zip(argb8888.chunks_exact_mut(4))
    {
        let packed = u32::from(rgba[3]) << 24
            | u32::from(rgba[0]) << 16
            | u32::from(rgba[1]) << 8
            | u32::from(rgba[2]);
        argb.copy_from_slice(&packed.to_ne_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premultiplied_rgba_is_packed_as_native_argb8888() {
        let source = [10, 20, 30, 40, 0, 0, 0, 0, 90, 80, 70, 255];
        let mut destination = [0; 12];

        write_argb8888_pixels(&source, &mut destination);

        let expected = [
            (40_u32 << 24 | 10_u32 << 16 | 20_u32 << 8 | 30_u32).to_ne_bytes(),
            0_u32.to_ne_bytes(),
            (255_u32 << 24 | 90_u32 << 16 | 80_u32 << 8 | 70_u32).to_ne_bytes(),
        ]
        .concat();
        assert_eq!(destination, expected.as_slice());
    }
}
