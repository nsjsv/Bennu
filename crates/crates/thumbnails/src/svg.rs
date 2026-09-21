use std::path::Path;
use std::time::Instant;

use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::{Options, Tree};

use crate::ThumbnailError;

pub(crate) fn load_svg_dimensions(source: &Path) -> Result<(u32, u32), ThumbnailError> {
    tracing::debug!(
        target: "thumbnails",
        source = ?source,
        "SVG dimensions load started"
    );
    let tree = load_svg_tree(source)?;
    let size = tree.size();
    tracing::debug!(
        target: "thumbnails",
        source = ?source,
        width = size.width(),
        height = size.height(),
        "SVG dimensions load finished"
    );

    Ok((
        round_svg_dimension(size.width()),
        round_svg_dimension(size.height()),
    ))
}

pub(crate) fn render_svg_thumbnail(
    source: &Path,
    output: &Path,
    max_edge: u32,
) -> Result<(u32, u32), ThumbnailError> {
    let tree = load_svg_tree(source)?;
    let source_size = tree.size();
    let scale = svg_thumbnail_scale(source_size.width(), source_size.height(), max_edge);
    let width = round_svg_dimension(source_size.width() * scale);
    let height = round_svg_dimension(source_size.height() * scale);
    tracing::debug!(
        target: "thumbnails",
        source = ?source,
        output = ?output,
        source_width = source_size.width(),
        source_height = source_size.height(),
        scale,
        width,
        height,
        max_edge,
        "SVG thumbnail dimensions prepared"
    );
    let Some(mut pixmap) = Pixmap::new(width, height) else {
        return Err(ThumbnailError::RenderSvg {
            path: source.to_path_buf(),
        });
    };

    resvg::render(
        &tree,
        Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap
        .save_png(output)
        .map_err(|source_error| ThumbnailError::WriteSvgThumbnail {
            path: output.to_path_buf(),
            source: svg_png_error_to_io_error(source_error),
        })?;

    Ok((width, height))
}

fn load_svg_tree(source: &Path) -> Result<Tree, ThumbnailError> {
    let started_at = Instant::now();
    tracing::debug!(
        target: "thumbnails",
        source = ?source,
        "SVG file read started"
    );
    let svg_data = std::fs::read(source).map_err(|source_error| ThumbnailError::ReadSvg {
        path: source.to_path_buf(),
        source: source_error,
    })?;
    tracing::debug!(
        target: "thumbnails",
        source = ?source,
        bytes = svg_data.len(),
        elapsed_ms = started_at.elapsed().as_millis(),
        "SVG file read finished"
    );

    let parse_started_at = Instant::now();
    tracing::debug!(
        target: "thumbnails",
        source = ?source,
        bytes = svg_data.len(),
        "SVG parse started"
    );
    let tree = Tree::from_data(&svg_data, &Options::default()).map_err(|source_error| {
        ThumbnailError::ParseSvg {
            path: source.to_path_buf(),
            source: source_error,
        }
    })?;
    tracing::debug!(
        target: "thumbnails",
        source = ?source,
        elapsed_ms = parse_started_at.elapsed().as_millis(),
        "SVG parse finished"
    );
    Ok(tree)
}

fn svg_thumbnail_scale(width: f32, height: f32, max_edge: u32) -> f32 {
    let max_edge = max_edge.max(1) as f32;
    (max_edge / width).min(max_edge / height)
}

fn round_svg_dimension(value: f32) -> u32 {
    value.round().max(1.0) as u32
}

fn svg_png_error_to_io_error(error: impl std::error::Error) -> std::io::Error {
    std::io::Error::other(error.to_string())
}
