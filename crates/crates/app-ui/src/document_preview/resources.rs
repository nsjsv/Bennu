use std::collections::{BTreeMap, BTreeSet, VecDeque};

use iced::widget::image;

use super::model::{DocumentPageRenderPlan, DocumentPageSize, DocumentScaleAxis};

pub(crate) const MAX_READY_DOCUMENT_PAGES: usize = 6;
pub(crate) const MAX_READY_DOCUMENT_RGBA_BYTES: u64 = 64 * 1024 * 1024;
pub(crate) const MAX_DOCUMENT_PAGE_EDGE: u32 = 4096;
pub(crate) const MAX_DOCUMENT_PAGE_PIXELS: u64 = 16 * 1024 * 1024;
const RENDER_WIDTH_BUCKETS: [u32; 5] = [512, 768, 1024, 1280, 1600];

#[derive(Debug, Clone)]
pub(super) struct ReadyDocumentPage {
    pub(super) handle: image::Handle,
    pub(super) rgba_bytes: u64,
}

#[derive(Debug, Clone, Default)]
pub(super) struct ReadyDocumentPageCache {
    pub(super) pages: BTreeMap<usize, ReadyDocumentPage>,
    lru: VecDeque<usize>,
    pub(super) rgba_bytes: u64,
}

impl ReadyDocumentPageCache {
    pub(super) fn contains(&self, page_index: usize) -> bool {
        self.pages.contains_key(&page_index)
    }

    pub(super) fn get(&self, page_index: usize) -> Option<&ReadyDocumentPage> {
        self.pages.get(&page_index)
    }

    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(super) fn touch(&mut self, page_index: usize) {
        if !self.pages.contains_key(&page_index) {
            return;
        }
        self.lru.retain(|existing| *existing != page_index);
        self.lru.push_back(page_index);
    }

    pub(super) fn insert(&mut self, page_index: usize, page: ReadyDocumentPage) {
        if let Some(replaced) = self.pages.remove(&page_index) {
            self.rgba_bytes = self.rgba_bytes.saturating_sub(replaced.rgba_bytes);
        }
        self.rgba_bytes = self.rgba_bytes.saturating_add(page.rgba_bytes);
        self.pages.insert(page_index, page);
        self.touch(page_index);
    }

    pub(super) fn reserve_for_active(&mut self, active: &[(usize, u64)]) {
        let active_pages = active
            .iter()
            .map(|(page_index, _)| *page_index)
            .collect::<BTreeSet<_>>();
        let reserved_bytes = active
            .iter()
            .fold(0_u64, |total, (_, bytes)| total.saturating_add(*bytes));
        let historical_page_limit = MAX_READY_DOCUMENT_PAGES.saturating_sub(active.len());
        let historical_byte_limit = MAX_READY_DOCUMENT_RGBA_BYTES.saturating_sub(reserved_bytes);

        loop {
            let historical_count = self
                .pages
                .keys()
                .filter(|page_index| !active_pages.contains(page_index))
                .count();
            let historical_bytes = self
                .pages
                .iter()
                .filter(|(page_index, _)| !active_pages.contains(page_index))
                .map(|(_, page)| page.rgba_bytes)
                .sum::<u64>();
            if historical_count <= historical_page_limit
                && historical_bytes <= historical_byte_limit
            {
                break;
            }
            let Some(position) = self
                .lru
                .iter()
                .position(|page_index| !active_pages.contains(page_index))
            else {
                break;
            };
            self.remove_lru(position);
        }

        while self.pages.len() > MAX_READY_DOCUMENT_PAGES
            || self.rgba_bytes > MAX_READY_DOCUMENT_RGBA_BYTES
        {
            let Some(position) = self
                .lru
                .iter()
                .position(|page_index| !active_pages.contains(page_index))
            else {
                break;
            };
            self.remove_lru(position);
        }
    }

    fn remove_lru(&mut self, position: usize) {
        let page_index = self.lru.remove(position).expect("LRU position");
        if let Some(removed) = self.pages.remove(&page_index) {
            self.rgba_bytes = self.rgba_bytes.saturating_sub(removed.rgba_bytes);
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct RenderCandidate {
    pub(super) page_index: usize,
    pub(super) estimated_rgba_bytes: u64,
    pub(super) intersects_viewport: bool,
    pub(super) center_distance: f32,
}

pub(super) fn select_render_set(mut candidates: Vec<RenderCandidate>) -> Vec<usize> {
    candidates.sort_by(|left, right| {
        right
            .intersects_viewport
            .cmp(&left.intersects_viewport)
            .then_with(|| left.center_distance.total_cmp(&right.center_distance))
            .then_with(|| left.page_index.cmp(&right.page_index))
    });
    let mut selected = Vec::new();
    let mut selected_bytes = 0_u64;
    for candidate in candidates {
        if selected.len() >= MAX_READY_DOCUMENT_PAGES
            || selected_bytes.saturating_add(candidate.estimated_rgba_bytes)
                > MAX_READY_DOCUMENT_RGBA_BYTES
        {
            continue;
        }
        selected_bytes += candidate.estimated_rgba_bytes;
        selected.push(candidate.page_index);
    }
    selected
}

pub(super) fn render_width_bucket(page_width: f32) -> u32 {
    RENDER_WIDTH_BUCKETS
        .iter()
        .copied()
        .find(|bucket| page_width <= *bucket as f32)
        .unwrap_or(*RENDER_WIDTH_BUCKETS.last().expect("render buckets"))
}

pub(super) fn page_render_plan(
    page: DocumentPageSize,
    width_bucket: u32,
) -> Result<DocumentPageRenderPlan, String> {
    let target_width = width_bucket.max(1);
    let target_height = ((target_width as f64 * page.height / page.width).ceil()) as u64;
    let width_scaled_pixels = u64::from(target_width).saturating_mul(target_height);
    let (width, height, scale_axis) = if target_height <= u64::from(MAX_DOCUMENT_PAGE_EDGE)
        && width_scaled_pixels <= MAX_DOCUMENT_PAGE_PIXELS
    {
        (
            target_width,
            u32::try_from(target_height).map_err(|_| "PDF page is too tall".to_owned())?,
            if page.quarter_turn {
                DocumentScaleAxis::Height(target_width)
            } else {
                DocumentScaleAxis::Width(target_width)
            },
        )
    } else {
        let ratio = page.height / page.width;
        let pixel_limited_height =
            ((MAX_DOCUMENT_PAGE_PIXELS as f64 * ratio).sqrt().floor()) as u32;
        let height = MAX_DOCUMENT_PAGE_EDGE.min(pixel_limited_height).max(1);
        let width = ((height as f64 * page.width / page.height).ceil()) as u32;
        (
            width,
            height,
            if page.quarter_turn {
                DocumentScaleAxis::Width(height)
            } else {
                DocumentScaleAxis::Height(height)
            },
        )
    };
    if width == 0
        || height == 0
        || width > MAX_DOCUMENT_PAGE_EDGE
        || height > MAX_DOCUMENT_PAGE_EDGE
        || u64::from(width).saturating_mul(u64::from(height)) > MAX_DOCUMENT_PAGE_PIXELS
    {
        return Err("PDF page exceeds the rendering pixel budget".to_owned());
    }
    let estimated_rgba_bytes = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "PDF page rendering size overflowed".to_owned())?;
    if estimated_rgba_bytes > MAX_READY_DOCUMENT_RGBA_BYTES {
        return Err("PDF page exceeds the preview memory budget".to_owned());
    }
    Ok(DocumentPageRenderPlan {
        width,
        height,
        estimated_rgba_bytes,
        scale_axis,
    })
}
