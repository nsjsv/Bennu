use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use iced::widget::image;
use tokio_util::sync::CancellationToken;

use super::resources::{
    page_render_plan, render_width_bucket, select_render_set, ReadyDocumentPage,
    ReadyDocumentPageCache, RenderCandidate,
};
use super::DocumentPreviewWorkspace;

pub(crate) const MAX_DOCUMENT_PAGES: usize = 10_000;
const DOCUMENT_PAGE_SIDE_MARGIN: f32 = 12.0;
const DOCUMENT_PAGE_GAP: f32 = 12.0;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct DocumentPreviewRequestKey {
    pub(crate) source_path: PathBuf,
    pub(crate) document_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct DocumentRenderKey {
    pub(crate) request: DocumentPreviewRequestKey,
    pub(crate) render_generation: u64,
    pub(crate) width_bucket: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct DocumentPageRequestKey {
    pub(crate) render: DocumentRenderKey,
    pub(crate) page_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct DocumentViewportKey {
    pub(crate) render: DocumentRenderKey,
    pub(crate) layout_generation: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct DocumentPrepareRequest {
    pub(crate) key: DocumentPreviewRequestKey,
    pub(crate) max_file_bytes: u64,
    pub(crate) cancellation: CancellationToken,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingDocumentPreview {
    pub(crate) key: DocumentPreviewRequestKey,
    pub(crate) cancellation: CancellationToken,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DocumentPageSize {
    pub(crate) width: f64,
    pub(crate) height: f64,
    pub(super) quarter_turn: bool,
}

impl DocumentPageSize {
    pub(crate) fn from_crop_box(
        mut width: f64,
        mut height: f64,
        rotation: i32,
    ) -> Result<Self, String> {
        if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
            return Err("PDF page has invalid dimensions".to_owned());
        }
        let quarter_turn = match rotation {
            0 | 180 => false,
            90 | 270 => true,
            _ => return Err("PDF page has invalid rotation".to_owned()),
        };
        if quarter_turn {
            std::mem::swap(&mut width, &mut height);
        }
        let aspect_ratio = height / width;
        if !aspect_ratio.is_finite() || aspect_ratio <= 0.0 {
            return Err("PDF page has invalid dimensions".to_owned());
        }
        Ok(Self {
            width,
            height,
            quarter_turn,
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedDocumentPreview {
    pub(crate) key: DocumentPreviewRequestKey,
    pub(crate) workspace: Arc<DocumentPreviewWorkspace>,
    pub(crate) pages: Vec<DocumentPageSize>,
}

#[derive(Debug, Clone)]
pub(crate) enum DocumentPrepareOutcome {
    Ready(PreparedDocumentPreview),
    Cancelled(DocumentPreviewRequestKey),
    Failed(DocumentPreviewRequestKey, String),
}

impl DocumentPrepareOutcome {
    pub(crate) fn key(&self) -> &DocumentPreviewRequestKey {
        match self {
            Self::Ready(prepared) => &prepared.key,
            Self::Cancelled(key) | Self::Failed(key, _) => key,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DocumentScaleAxis {
    Width(u32),
    Height(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DocumentPageRenderPlan {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) estimated_rgba_bytes: u64,
    pub(crate) scale_axis: DocumentScaleAxis,
}

#[derive(Debug, Clone)]
pub(crate) struct DocumentPageRenderRequest {
    pub(crate) key: DocumentPageRequestKey,
    pub(crate) workspace: Arc<DocumentPreviewWorkspace>,
    pub(crate) plan: DocumentPageRenderPlan,
    pub(crate) document_cancellation: CancellationToken,
    pub(crate) render_cancellation: CancellationToken,
}

#[derive(Debug, Clone)]
pub(crate) struct DocumentPageRenderResult {
    pub(crate) key: DocumentPageRequestKey,
    pub(crate) handle: image::Handle,
    #[cfg(test)]
    pub(crate) width: u32,
    #[cfg(test)]
    pub(crate) height: u32,
    pub(crate) rgba_bytes: u64,
}

#[derive(Debug, Clone)]
pub(crate) enum DocumentPageRenderOutcome {
    Ready(DocumentPageRenderResult),
    Cancelled(DocumentPageRequestKey),
    Failed(DocumentPageRequestKey, String),
}

impl DocumentPageRenderOutcome {
    pub(crate) fn key(&self) -> &DocumentPageRequestKey {
        match self {
            Self::Ready(result) => &result.key,
            Self::Cancelled(key) | Self::Failed(key, _) => key,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum DocumentPreviewMessage {
    Prepared(DocumentPrepareOutcome),
    PageRendered(DocumentPageRenderOutcome),
    Scrolled {
        key: DocumentViewportKey,
        offset_y: f32,
        viewport_height: f32,
        content_height: f32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DocumentPageLayout {
    pub(crate) top: f32,
    pub(crate) height: f32,
}

pub(crate) enum DocumentPageView<'a> {
    Ready(&'a image::Handle),
    Error(&'a str),
    Loading,
    Deferred,
}

#[derive(Debug, Clone)]
pub(crate) struct PagedDocumentPreview {
    key: DocumentPreviewRequestKey,
    workspace: Arc<DocumentPreviewWorkspace>,
    pages: Vec<DocumentPageSize>,
    document_cancellation: CancellationToken,
    render_cancellation: CancellationToken,
    render_generation: u64,
    layout_generation: u64,
    width_bucket: u32,
    preview_width: f32,
    preview_height: f32,
    page_width: f32,
    viewport_offset: f32,
    viewport_height: f32,
    display_heights: Vec<f32>,
    page_tops: Vec<f32>,
    content_height: f32,
    wanted_pages: Vec<usize>,
    current_render_set: Vec<usize>,
    queued: VecDeque<usize>,
    in_flight: BTreeSet<usize>,
    ready: ReadyDocumentPageCache,
    failures: BTreeMap<usize, String>,
}

impl PagedDocumentPreview {
    pub(crate) fn new(
        prepared: PreparedDocumentPreview,
        document_cancellation: CancellationToken,
        preview_width: f32,
        preview_height: f32,
    ) -> Result<Self, String> {
        if prepared.pages.is_empty() || prepared.pages.len() > MAX_DOCUMENT_PAGES {
            return Err("PDF preview has an invalid page count".to_owned());
        }
        let page_width = document_page_width(preview_width);
        let viewport_height = document_viewport_height(preview_height);
        let width_bucket = render_width_bucket(page_width);
        let mut preview = Self {
            key: prepared.key,
            workspace: prepared.workspace,
            pages: prepared.pages,
            document_cancellation,
            render_cancellation: CancellationToken::new(),
            render_generation: 1,
            layout_generation: 1,
            width_bucket,
            preview_width,
            preview_height,
            page_width,
            viewport_offset: 0.0,
            viewport_height,
            display_heights: Vec::new(),
            page_tops: Vec::new(),
            content_height: 0.0,
            wanted_pages: Vec::new(),
            current_render_set: Vec::new(),
            queued: VecDeque::new(),
            in_flight: BTreeSet::new(),
            ready: ReadyDocumentPageCache::default(),
            failures: BTreeMap::new(),
        };
        preview.rebuild_layout()?;
        preview.replace_render_window();
        Ok(preview)
    }

    pub(crate) fn render_key(&self) -> DocumentRenderKey {
        DocumentRenderKey {
            request: self.key.clone(),
            render_generation: self.render_generation,
            width_bucket: self.width_bucket,
        }
    }

    pub(crate) fn viewport_key(&self) -> DocumentViewportKey {
        DocumentViewportKey {
            render: self.render_key(),
            layout_generation: self.layout_generation,
        }
    }

    pub(crate) fn page_width(&self) -> f32 {
        self.page_width
    }

    pub(crate) fn content_height(&self) -> f32 {
        self.content_height
    }

    pub(crate) fn wanted_pages(&self) -> &[usize] {
        &self.wanted_pages
    }

    pub(crate) fn page_layout(&self, page_index: usize) -> Option<DocumentPageLayout> {
        Some(DocumentPageLayout {
            top: *self.page_tops.get(page_index)?,
            height: *self.display_heights.get(page_index)?,
        })
    }

    pub(crate) fn top_spacer_height(&self) -> f32 {
        self.wanted_pages
            .first()
            .and_then(|page_index| self.page_tops.get(*page_index))
            .copied()
            .unwrap_or(0.0)
    }

    pub(crate) fn bottom_spacer_height(&self) -> f32 {
        let Some(page_index) = self.wanted_pages.last().copied() else {
            return self.content_height;
        };
        let end = self.page_tops[page_index] + self.display_heights[page_index];
        (self.content_height - end).max(0.0)
    }

    pub(crate) fn page_view(&self, page_index: usize) -> DocumentPageView<'_> {
        if let Some(page) = self.ready.get(page_index) {
            DocumentPageView::Ready(&page.handle)
        } else if let Some(error) = self.failures.get(&page_index) {
            DocumentPageView::Error(error)
        } else if self.current_render_set.contains(&page_index) {
            DocumentPageView::Loading
        } else {
            DocumentPageView::Deferred
        }
    }

    pub(crate) fn cancel(&self) {
        self.document_cancellation.cancel();
        self.render_cancellation.cancel();
    }

    pub(crate) fn update_viewport(
        &mut self,
        key: &DocumentViewportKey,
        offset: f32,
        viewport_height: f32,
    ) -> bool {
        if key != &self.viewport_key() {
            return false;
        }
        let viewport_height = viewport_height.max(1.0);
        if (self.viewport_height - viewport_height).abs() > f32::EPSILON {
            self.layout_generation = self.layout_generation.wrapping_add(1);
        }
        self.viewport_height = viewport_height;
        self.viewport_offset =
            clamp_viewport_offset(offset, self.viewport_height, self.content_height);
        self.replace_render_window();
        true
    }

    pub(crate) fn resize(
        &mut self,
        preview_width: f32,
        preview_height: f32,
    ) -> Result<f32, String> {
        if self.preview_width == preview_width && self.preview_height == preview_height {
            return Ok(self.viewport_offset);
        }
        let anchor = self.scroll_anchor();
        let next_page_width = document_page_width(preview_width);
        let next_viewport_height = document_viewport_height(preview_height);
        let next_bucket = render_width_bucket(next_page_width);
        self.layout_generation = self.layout_generation.wrapping_add(1);
        self.preview_width = preview_width;
        self.preview_height = preview_height;
        self.page_width = next_page_width;
        self.viewport_height = next_viewport_height;
        if next_bucket != self.width_bucket {
            self.render_cancellation.cancel();
            self.render_cancellation = CancellationToken::new();
            self.render_generation = self.render_generation.wrapping_add(1);
            self.width_bucket = next_bucket;
            self.in_flight.clear();
            self.ready.clear();
            self.failures.clear();
        }
        self.rebuild_layout()?;
        self.viewport_offset = self.offset_for_anchor(anchor);
        self.replace_render_window();
        Ok(self.viewport_offset)
    }

    pub(crate) fn drain_render_requests(
        &mut self,
        maximum_requests: usize,
    ) -> Vec<DocumentPageRenderRequest> {
        let available = maximum_requests.saturating_sub(self.in_flight.len());
        let render_key = self.render_key();
        let mut requests = Vec::with_capacity(available);
        for _ in 0..available {
            let Some(page_index) = self.queued.pop_front() else {
                break;
            };
            if !self.current_render_set.contains(&page_index)
                || self.ready.contains(page_index)
                || self.failures.contains_key(&page_index)
                || !self.in_flight.insert(page_index)
            {
                continue;
            }
            let Ok(plan) = page_render_plan(self.pages[page_index], self.width_bucket) else {
                self.in_flight.remove(&page_index);
                self.failures.insert(
                    page_index,
                    "PDF page exceeds the rendering budget".to_owned(),
                );
                continue;
            };
            requests.push(DocumentPageRenderRequest {
                key: DocumentPageRequestKey {
                    render: render_key.clone(),
                    page_index,
                },
                workspace: self.workspace.clone(),
                plan,
                document_cancellation: self.document_cancellation.clone(),
                render_cancellation: self.render_cancellation.clone(),
            });
        }
        requests
    }

    pub(crate) fn accept_page_outcome(&mut self, outcome: DocumentPageRenderOutcome) -> bool {
        let key = outcome.key().clone();
        if key.render != self.render_key() || key.page_index >= self.pages.len() {
            return false;
        }
        self.in_flight.remove(&key.page_index);
        match outcome {
            DocumentPageRenderOutcome::Ready(result) => {
                if !self.current_render_set.contains(&result.key.page_index) {
                    return true;
                }
                self.failures.remove(&result.key.page_index);
                self.ready.insert(
                    result.key.page_index,
                    ReadyDocumentPage {
                        handle: result.handle,
                        rgba_bytes: result.rgba_bytes,
                    },
                );
                self.reserve_ready_cache_for_current_set();
            }
            DocumentPageRenderOutcome::Failed(key, error) => {
                if self.current_render_set.contains(&key.page_index) {
                    self.failures.insert(key.page_index, error);
                }
            }
            DocumentPageRenderOutcome::Cancelled(_) => {}
        }
        true
    }

    fn rebuild_layout(&mut self) -> Result<(), String> {
        self.display_heights.clear();
        self.page_tops.clear();
        let mut top = 0.0_f32;
        for (index, page) in self.pages.iter().enumerate() {
            let height = (self.page_width as f64 * page.height / page.width) as f32;
            if !height.is_finite() || height <= 0.0 {
                return Err(format!(
                    "PDF page {} has invalid layout dimensions",
                    index + 1
                ));
            }
            self.page_tops.push(top);
            self.display_heights.push(height);
            top += height;
            if !top.is_finite() {
                return Err("PDF document layout height overflowed".to_owned());
            }
            if index + 1 < self.pages.len() {
                top += DOCUMENT_PAGE_GAP;
                if !top.is_finite() {
                    return Err("PDF document layout height overflowed".to_owned());
                }
            }
        }
        self.content_height = top.max(1.0);
        Ok(())
    }

    fn replace_render_window(&mut self) {
        self.wanted_pages = wanted_page_range(
            &self.page_tops,
            &self.display_heights,
            self.viewport_offset,
            self.viewport_height,
        );
        let viewport_center = self.viewport_offset + self.viewport_height / 2.0;
        let viewport_end = self.viewport_offset + self.viewport_height;
        let candidates = self
            .wanted_pages
            .iter()
            .filter_map(|page_index| {
                let layout = self.page_layout(*page_index)?;
                let plan = page_render_plan(self.pages[*page_index], self.width_bucket).ok()?;
                let page_center = layout.top + layout.height / 2.0;
                Some(RenderCandidate {
                    page_index: *page_index,
                    estimated_rgba_bytes: plan.estimated_rgba_bytes,
                    intersects_viewport: layout.top < viewport_end
                        && layout.top + layout.height > self.viewport_offset,
                    center_distance: (page_center - viewport_center).abs(),
                })
            })
            .collect::<Vec<_>>();
        self.current_render_set = select_render_set(candidates);
        self.failures
            .retain(|page_index, _| self.current_render_set.contains(page_index));
        let ready_active = self
            .current_render_set
            .iter()
            .copied()
            .filter(|page_index| self.ready.contains(*page_index))
            .collect::<Vec<_>>();
        for page_index in ready_active {
            self.ready.touch(page_index);
        }
        self.reserve_ready_cache_for_current_set();
        self.queued = self
            .current_render_set
            .iter()
            .copied()
            .filter(|page_index| {
                !self.ready.contains(*page_index)
                    && !self.failures.contains_key(page_index)
                    && !self.in_flight.contains(page_index)
            })
            .collect();
    }

    fn reserve_ready_cache_for_current_set(&mut self) {
        let active = self
            .current_render_set
            .iter()
            .filter_map(|page_index| {
                page_render_plan(self.pages[*page_index], self.width_bucket)
                    .ok()
                    .map(|plan| (*page_index, plan.estimated_rgba_bytes))
            })
            .collect::<Vec<_>>();
        self.ready.reserve_for_active(&active);
    }

    fn scroll_anchor(&self) -> ScrollAnchor {
        let page_index =
            page_at_offset(&self.page_tops, &self.display_heights, self.viewport_offset);
        let page_top = self.page_tops[page_index];
        let page_height = self.display_heights[page_index];
        ScrollAnchor {
            page_index,
            page_fraction: ((self.viewport_offset - page_top) / page_height).clamp(0.0, 1.0),
        }
    }

    fn offset_for_anchor(&self, anchor: ScrollAnchor) -> f32 {
        let page_index = anchor.page_index.min(self.pages.len() - 1);
        let offset =
            self.page_tops[page_index] + self.display_heights[page_index] * anchor.page_fraction;
        clamp_viewport_offset(offset, self.viewport_height, self.content_height)
    }

    #[cfg(test)]
    pub(crate) fn document_cancellation(&self) -> CancellationToken {
        self.document_cancellation.clone()
    }

    #[cfg(test)]
    pub(crate) fn viewport_offset(&self) -> f32 {
        self.viewport_offset
    }

    #[cfg(test)]
    pub(crate) fn queued_pages(&self) -> Vec<usize> {
        self.queued.iter().copied().collect()
    }

    #[cfg(test)]
    pub(crate) fn current_render_pages(&self) -> &[usize] {
        &self.current_render_set
    }

    #[cfg(test)]
    pub(crate) fn ready_page_count(&self) -> usize {
        self.ready.pages.len()
    }

    #[cfg(test)]
    pub(crate) fn ready_rgba_bytes(&self) -> u64 {
        self.ready.rgba_bytes
    }
}

#[derive(Debug, Clone, Copy)]
struct ScrollAnchor {
    page_index: usize,
    page_fraction: f32,
}

pub(crate) fn document_page_width(preview_width: f32) -> f32 {
    (preview_width - 2.0 * DOCUMENT_PAGE_SIDE_MARGIN).max(1.0)
}

pub(crate) fn document_viewport_height(preview_height: f32) -> f32 {
    preview_height.max(1.0)
}

fn wanted_page_range(
    page_tops: &[f32],
    page_heights: &[f32],
    viewport_offset: f32,
    viewport_height: f32,
) -> Vec<usize> {
    if page_tops.is_empty() {
        return Vec::new();
    }
    let viewport_end = viewport_offset + viewport_height.max(1.0);
    let first_visible = page_tops
        .partition_point(|top| *top <= viewport_offset)
        .saturating_sub(1)
        .min(page_tops.len() - 1);
    let first_visible = (first_visible..page_tops.len())
        .find(|index| page_tops[*index] + page_heights[*index] > viewport_offset)
        .unwrap_or(page_tops.len() - 1);
    let last_visible = (first_visible..page_tops.len())
        .take_while(|index| page_tops[*index] < viewport_end)
        .last()
        .unwrap_or(first_visible);
    let start = first_visible.saturating_sub(1);
    let end = (last_visible + 1).min(page_tops.len() - 1);
    (start..=end).collect()
}

fn page_at_offset(page_tops: &[f32], page_heights: &[f32], offset: f32) -> usize {
    let candidate = page_tops
        .partition_point(|top| *top <= offset)
        .saturating_sub(1)
        .min(page_tops.len() - 1);
    if offset > page_tops[candidate] + page_heights[candidate] && candidate + 1 < page_tops.len() {
        candidate + 1
    } else {
        candidate
    }
}

fn clamp_viewport_offset(offset: f32, viewport_height: f32, content_height: f32) -> f32 {
    offset
        .max(0.0)
        .min((content_height - viewport_height).max(0.0))
}

#[cfg(test)]
#[path = "model/tests.rs"]
mod tests;
