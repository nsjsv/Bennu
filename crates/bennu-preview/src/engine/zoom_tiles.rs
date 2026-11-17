//! 超大图放大时切块显示的状态机：放大到超过按屏幕大小解的那张、整张又超过 64 MB 时，在后台切块，
//! 之后每次视口变化读看得见的块（几何见 `crate::zoom_tiles`）。换图、关预览时随原图代际一起作废。

use std::path::PathBuf;
use std::sync::Arc;

use file_core::image_decode::tiles::TiledImage;
use iced::widget::image;
use iced::Task;
use tokio_util::sync::CancellationToken;

use crate::original_image_preview::ORIGINAL_IMAGE_DECODE_SEMAPHORE;
use crate::preview_message::PreviewMessage;
use crate::zoom_tiles::{SharedTiledImage, TileKey, TileView, ZoomTiles, ZoomTilesMessage};

pub(crate) enum ZoomTilesState {
    Building,
    Ready(ZoomTiles),
    /// 切不了（格式不能逐行读、磁盘不够、出错）：停在屏幕大小那张，这张图不再试。
    Unavailable,
}

impl super::PreviewEngine {
    /// 放大到要更多像素时：还没切就切，切好了就读看得见的块。
    pub(crate) fn request_zoom_tiles(
        &mut self,
        path: PathBuf,
        view: TileView,
    ) -> Task<PreviewMessage> {
        self.zoom_tile_view = Some(view);
        match self.zoom_tiles {
            None => {
                self.zoom_tiles = Some(ZoomTilesState::Building);
                build_command(
                    path,
                    self.original_image_preview_generation,
                    self.original_image_preview_cancel
                        .clone()
                        .unwrap_or_default(),
                )
            }
            Some(ZoomTilesState::Ready(_)) => self.load_visible_tiles(),
            Some(ZoomTilesState::Building | ZoomTilesState::Unavailable) => Task::none(),
        }
    }

    /// 缩回到不用更多像素的大小：读进来的块全丢掉；切块文件留着，再放大不用重切。
    pub(crate) fn release_zoom_tiles(&mut self) {
        self.zoom_tile_view = None;
        if let Some(ZoomTilesState::Ready(tiles)) = &mut self.zoom_tiles {
            tiles.loaded.clear();
        }
    }

    pub fn update_zoom_tiles(&mut self, message: ZoomTilesMessage) -> Task<PreviewMessage> {
        match message {
            ZoomTilesMessage::Built {
                generation,
                outcome,
            } => {
                if generation != self.original_image_preview_generation
                    || !matches!(self.zoom_tiles, Some(ZoomTilesState::Building))
                {
                    return Task::none();
                }
                match outcome {
                    Ok(Some(image)) => {
                        self.zoom_tiles = Some(ZoomTilesState::Ready(ZoomTiles::new(image)));
                        self.load_visible_tiles()
                    }
                    Ok(None) => {
                        self.zoom_tiles = Some(ZoomTilesState::Unavailable);
                        Task::none()
                    }
                    Err(error) => {
                        tracing::warn!(target: "app_ui::preview", %error, "tiling the image for zooming failed; keeping the screen-sized one");
                        self.zoom_tiles = Some(ZoomTilesState::Unavailable);
                        Task::none()
                    }
                }
            }
            ZoomTilesMessage::Loaded { generation, tiles } => {
                if generation != self.original_image_preview_generation {
                    return Task::none();
                }
                let Some(ZoomTilesState::Ready(zoom_tiles)) = &mut self.zoom_tiles else {
                    return Task::none();
                };
                for (key, handle) in tiles {
                    zoom_tiles.in_flight.remove(&key);
                    if let Some(handle) = handle {
                        zoom_tiles.loaded.insert(key, handle);
                    }
                }
                // 读的时候视口可能又动了：丢掉已经看不见的，补上新看见的。
                self.load_visible_tiles()
            }
        }
    }

    fn load_visible_tiles(&mut self) -> Task<PreviewMessage> {
        let (Some(view), Some(ZoomTilesState::Ready(tiles))) =
            (self.zoom_tile_view, &mut self.zoom_tiles)
        else {
            return Task::none();
        };
        tiles.evict_outside(&view);
        let missing = tiles.take_missing(&view);
        if missing.is_empty() {
            return Task::none();
        }
        let image = tiles.image.clone();
        let generation = self.original_image_preview_generation;
        Task::perform(read_tiles(image, missing), move |tiles| {
            PreviewMessage::ZoomTiles(ZoomTilesMessage::Loaded { generation, tiles })
        })
    }

    /// 界面要画的切块：切好了、并且正放大到要更多像素时才有。
    pub(crate) fn visible_zoom_tiles(&self) -> Option<&ZoomTiles> {
        match (&self.zoom_tiles, self.zoom_tile_view) {
            (Some(ZoomTilesState::Ready(tiles)), Some(_)) => Some(tiles),
            _ => None,
        }
    }
}

fn build_command(
    path: PathBuf,
    generation: u64,
    cancellation: CancellationToken,
) -> Task<PreviewMessage> {
    Task::perform(
        async move {
            // 和整张解原图共用一个名额：两张超大图不同时解。
            let _permit = ORIGINAL_IMAGE_DECODE_SEMAPHORE
                .acquire()
                .await
                .map_err(|_| "original image decoder is unavailable".to_owned())?;
            tokio::task::spawn_blocking(move || {
                TiledImage::build(&path, &cancellation)
                    .map(|tiled| tiled.map(|tiled| SharedTiledImage(Arc::new(tiled))))
                    .map_err(|error| error.to_string())
            })
            .await
            .map_err(|error| format!("could not join the tiling task: {error}"))?
        },
        move |outcome| {
            PreviewMessage::ZoomTiles(ZoomTilesMessage::Built {
                generation,
                outcome,
            })
        },
    )
}

/// 读一批块（几十块、每块 256 KB，一次读完再交回界面）；读坏的块记日志，交回 `None`。
async fn read_tiles(
    image: SharedTiledImage,
    keys: Vec<TileKey>,
) -> Vec<(TileKey, Option<image::Handle>)> {
    let fallback = keys.iter().map(|key| (*key, None)).collect();
    tokio::task::spawn_blocking(move || {
        keys.into_iter()
            .map(|key| {
                let handle = match image.0.tile(key.level, key.column, key.row) {
                    Ok(tile) => Some(image::Handle::from_rgba(
                        tile.width(),
                        tile.height(),
                        tile.into_raw(),
                    )),
                    Err(error) => {
                        tracing::warn!(target: "app_ui::preview", ?key, %error, "reading an image tile failed");
                        None
                    }
                };
                (key, handle)
            })
            .collect()
    })
    .await
    .unwrap_or(fallback)
}
