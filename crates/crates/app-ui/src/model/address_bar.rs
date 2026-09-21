use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use crate::animation::{ease_out_cubic, elapsed_fraction_at};

use super::{BrowserPaneId, BrowserViewMode};

pub(crate) const ADDRESS_BAR_TRANSITION_DURATION: Duration = Duration::from_millis(160);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BreadcrumbSegmentKind {
    Home,
    Root,
    Name(OsString),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BreadcrumbSegment {
    pub(crate) target: PathBuf,
    pub(crate) kind: BreadcrumbSegmentKind,
    pub(crate) is_current: bool,
}

impl BreadcrumbSegment {
    pub(crate) fn display_text(&self) -> String {
        match &self.kind {
            BreadcrumbSegmentKind::Home => String::new(),
            BreadcrumbSegmentKind::Root => std::path::MAIN_SEPARATOR.to_string(),
            BreadcrumbSegmentKind::Name(name) => name.to_string_lossy().into_owned(),
        }
    }
}

pub(crate) fn displayed_address_directory<'a>(
    current_dir: &'a Path,
    view_mode: BrowserViewMode,
    deepest_open_column_directory: Option<&'a PathBuf>,
) -> &'a Path {
    match view_mode {
        BrowserViewMode::Columns => deepest_open_column_directory
            .map(PathBuf::as_path)
            .unwrap_or(current_dir),
        BrowserViewMode::List | BrowserViewMode::Icons => current_dir,
    }
}

pub(crate) fn breadcrumb_segments(current_dir: &Path, home_dir: &Path) -> Vec<BreadcrumbSegment> {
    let mut segments = if let Ok(relative_path) = current_dir.strip_prefix(home_dir) {
        let mut home_segments = vec![BreadcrumbSegment {
            target: home_dir.to_path_buf(),
            kind: BreadcrumbSegmentKind::Home,
            is_current: false,
        }];
        append_relative_segments(&mut home_segments, home_dir.to_path_buf(), relative_path);
        home_segments
    } else {
        absolute_breadcrumb_segments(current_dir)
    };

    if let Some(current_segment) = segments.last_mut() {
        current_segment.is_current = true;
    }
    segments
}

fn absolute_breadcrumb_segments(current_dir: &Path) -> Vec<BreadcrumbSegment> {
    let mut segments = Vec::new();
    let mut cumulative_target = PathBuf::new();

    for component in current_dir.components() {
        match component {
            Component::Prefix(prefix) => cumulative_target.push(prefix.as_os_str()),
            Component::RootDir => {
                cumulative_target.push(component.as_os_str());
                segments.push(BreadcrumbSegment {
                    target: cumulative_target.clone(),
                    kind: BreadcrumbSegmentKind::Root,
                    is_current: false,
                });
            }
            Component::Normal(name) => {
                cumulative_target.push(name);
                segments.push(BreadcrumbSegment {
                    target: cumulative_target.clone(),
                    kind: BreadcrumbSegmentKind::Name(name.to_os_string()),
                    is_current: false,
                });
            }
            Component::CurDir | Component::ParentDir => {
                cumulative_target.push(component.as_os_str());
                segments.push(BreadcrumbSegment {
                    target: cumulative_target.clone(),
                    kind: BreadcrumbSegmentKind::Name(component.as_os_str().to_os_string()),
                    is_current: false,
                });
            }
        }
    }

    segments
}

fn append_relative_segments(
    segments: &mut Vec<BreadcrumbSegment>,
    mut cumulative_target: PathBuf,
    relative_path: &Path,
) {
    for component in relative_path.components() {
        cumulative_target.push(component.as_os_str());
        let kind = match component {
            Component::Normal(name) => BreadcrumbSegmentKind::Name(name.to_os_string()),
            Component::CurDir | Component::ParentDir => {
                BreadcrumbSegmentKind::Name(component.as_os_str().to_os_string())
            }
            Component::Prefix(_) | Component::RootDir => continue,
        };
        segments.push(BreadcrumbSegment {
            target: cumulative_target.clone(),
            kind,
            is_current: false,
        });
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BreadcrumbWidthAllocation {
    pub(crate) segment_widths: Vec<f32>,
    pub(crate) content_width: f32,
    pub(crate) overflows: bool,
}

pub(crate) fn allocate_breadcrumb_widths(
    natural_widths: &[f32],
    minimum_widths: &[f32],
    separator_total_width: f32,
    viewport_width: f32,
) -> BreadcrumbWidthAllocation {
    assert_eq!(natural_widths.len(), minimum_widths.len());

    let viewport_width = viewport_width.max(0.0);
    let separator_total_width = separator_total_width.max(0.0);
    let natural_widths = natural_widths
        .iter()
        .map(|width| width.max(0.0))
        .collect::<Vec<_>>();
    let minimum_widths = minimum_widths
        .iter()
        .zip(&natural_widths)
        .map(|(minimum, natural)| minimum.max(0.0).min(*natural))
        .collect::<Vec<_>>();
    let natural_content_width = separator_total_width + natural_widths.iter().sum::<f32>();

    if natural_content_width <= viewport_width {
        return BreadcrumbWidthAllocation {
            segment_widths: natural_widths,
            content_width: natural_content_width,
            overflows: false,
        };
    }

    let required_reduction = natural_content_width - viewport_width;
    let compression_capacity = natural_widths
        .iter()
        .zip(&minimum_widths)
        .map(|(natural, minimum)| natural - minimum)
        .sum::<f32>();

    if required_reduction >= compression_capacity {
        let content_width = separator_total_width + minimum_widths.iter().sum::<f32>();
        return BreadcrumbWidthAllocation {
            segment_widths: minimum_widths,
            content_width,
            overflows: content_width > viewport_width,
        };
    }

    let compression_fraction = required_reduction / compression_capacity;
    let segment_widths = natural_widths
        .iter()
        .zip(&minimum_widths)
        .map(|(natural, minimum)| natural - (natural - minimum) * compression_fraction)
        .collect::<Vec<_>>();

    BreadcrumbWidthAllocation {
        segment_widths,
        content_width: viewport_width,
        overflows: false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct AddressEditingSessionId(pub(crate) u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AddressSuggestionRequest {
    pub(crate) pane_id: BrowserPaneId,
    pub(crate) session_id: AddressEditingSessionId,
    pub(crate) draft: String,
    pub(crate) current_dir: PathBuf,
    pub(crate) generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AddressEditingSession {
    pub(crate) pane_id: BrowserPaneId,
    pub(crate) session_id: AddressEditingSessionId,
    pub(crate) draft: String,
    pub(crate) suggestions: Vec<PathBuf>,
    pub(crate) suggestion_selection: Option<usize>,
    pub(crate) generation: u64,
}

impl AddressEditingSession {
    pub(crate) fn new(
        pane_id: BrowserPaneId,
        session_id: AddressEditingSessionId,
        current_dir: &Path,
    ) -> Self {
        Self {
            pane_id,
            session_id,
            draft: current_dir.to_string_lossy().into_owned(),
            suggestions: Vec::new(),
            suggestion_selection: None,
            generation: 0,
        }
    }

    pub(crate) fn next_suggestion_request(
        &mut self,
        current_dir: &Path,
    ) -> AddressSuggestionRequest {
        self.generation = self.generation.wrapping_add(1);
        AddressSuggestionRequest {
            pane_id: self.pane_id,
            session_id: self.session_id,
            draft: self.draft.clone(),
            current_dir: current_dir.to_path_buf(),
            generation: self.generation,
        }
    }

    pub(crate) fn matches_suggestion_request(
        &self,
        request: &AddressSuggestionRequest,
        current_dir: &Path,
    ) -> bool {
        self.pane_id == request.pane_id
            && self.session_id == request.session_id
            && self.draft == request.draft
            && current_dir == request.current_dir
            && self.generation == request.generation
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AddressBarTransition {
    pub(crate) pane_id: BrowserPaneId,
    start_fraction: f32,
    target_fraction: f32,
    started_at: Instant,
    duration: Duration,
    pub(crate) exit_snapshot: Option<String>,
}

impl AddressBarTransition {
    pub(crate) fn retarget(
        previous: Option<&Self>,
        pane_id: BrowserPaneId,
        target_fraction: f32,
        exit_snapshot: Option<String>,
        now: Instant,
    ) -> Self {
        let target_fraction = target_fraction.clamp(0.0, 1.0);
        let start_fraction = previous
            .filter(|transition| transition.pane_id == pane_id)
            .map(|transition| transition.fraction_at(now))
            .unwrap_or(1.0 - target_fraction);
        let distance = (target_fraction - start_fraction).abs();
        let duration = ADDRESS_BAR_TRANSITION_DURATION.mul_f32(distance);

        Self {
            pane_id,
            start_fraction,
            target_fraction,
            started_at: now,
            duration,
            exit_snapshot,
        }
    }

    pub(crate) fn fraction(&self) -> f32 {
        self.fraction_at(Instant::now())
    }

    pub(crate) fn fraction_at(&self, now: Instant) -> f32 {
        let progress = elapsed_fraction_at(self.started_at, now, self.duration);
        let eased_progress = ease_out_cubic(progress);
        self.start_fraction + (self.target_fraction - self.start_fraction) * eased_progress
    }

    pub(crate) fn is_complete_at(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started_at) >= self.duration
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.is_complete_at(Instant::now())
    }

    pub(crate) fn target_fraction(&self) -> f32 {
        self.target_fraction
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;

    #[test]
    fn columns_display_the_deepest_open_directory() {
        let current_dir = Path::new("/workspace");
        let deepest_open_directory = PathBuf::from("/workspace/project/src");

        assert_eq!(
            displayed_address_directory(
                current_dir,
                BrowserViewMode::Columns,
                Some(&deepest_open_directory),
            ),
            deepest_open_directory
        );
    }

    #[test]
    fn list_view_ignores_column_open_directory() {
        let current_dir = Path::new("/workspace");
        let deepest_open_directory = PathBuf::from("/workspace/project/src");

        assert_eq!(
            displayed_address_directory(
                current_dir,
                BrowserViewMode::List,
                Some(&deepest_open_directory),
            ),
            current_dir
        );
    }

    #[test]
    fn icon_view_ignores_hidden_column_open_directory() {
        let current_dir = Path::new("/workspace");
        let deepest_open_directory = PathBuf::from("/workspace/project/src");

        assert_eq!(
            displayed_address_directory(
                current_dir,
                BrowserViewMode::Icons,
                Some(&deepest_open_directory),
            ),
            current_dir
        );
    }

    #[test]
    fn home_path_segments_keep_cumulative_targets() {
        let segments = breadcrumb_segments(
            Path::new("/home/user/Documents/Project"),
            Path::new("/home/user"),
        );

        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0].kind, BreadcrumbSegmentKind::Home);
        assert_eq!(segments[0].target, PathBuf::from("/home/user"));
        assert_eq!(segments[1].target, PathBuf::from("/home/user/Documents"));
        assert_eq!(
            segments[2].target,
            PathBuf::from("/home/user/Documents/Project")
        );
        assert!(segments[2].is_current);
    }

    #[test]
    fn path_outside_home_starts_at_filesystem_root() {
        let segments = breadcrumb_segments(Path::new("/opt/data"), Path::new("/home/user"));

        assert_eq!(segments[0].kind, BreadcrumbSegmentKind::Root);
        assert_eq!(segments[0].target, PathBuf::from("/"));
        assert_eq!(segments[1].target, PathBuf::from("/opt"));
        assert_eq!(segments[2].target, PathBuf::from("/opt/data"));
    }

    #[test]
    fn filesystem_root_is_a_single_current_segment() {
        let segments = breadcrumb_segments(Path::new("/"), Path::new("/home/user"));

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].kind, BreadcrumbSegmentKind::Root);
        assert!(segments[0].is_current);
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_name_keeps_original_path_target() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};

        let raw_name = OsString::from_vec(vec![b'n', b'a', 0x80, b'm', b'e']);
        let current_dir = PathBuf::from("/tmp").join(&raw_name);
        let segments = breadcrumb_segments(&current_dir, Path::new("/home/user"));
        let final_segment = segments.last().expect("non UTF-8 segment");

        assert_eq!(
            final_segment.target.as_os_str().as_bytes(),
            current_dir.as_os_str().as_bytes()
        );
        assert_eq!(
            match &final_segment.kind {
                BreadcrumbSegmentKind::Name(name) => name.as_os_str(),
                _ => OsStr::new(""),
            }
            .as_bytes(),
            raw_name.as_os_str().as_bytes()
        );
    }

    #[test]
    fn natural_widths_are_preserved_when_they_fit() {
        let allocation = allocate_breadcrumb_widths(&[40.0, 80.0], &[32.0, 32.0], 12.0, 140.0);

        assert_eq!(allocation.segment_widths, vec![40.0, 80.0]);
        assert_eq!(allocation.content_width, 132.0);
        assert!(!allocation.overflows);
    }

    #[test]
    fn one_long_segment_uses_available_compression() {
        let allocation = allocate_breadcrumb_widths(&[220.0], &[56.0], 0.0, 120.0);

        assert_eq!(allocation.segment_widths, vec![120.0]);
        assert_eq!(allocation.content_width, 120.0);
        assert!(!allocation.overflows);
    }

    #[test]
    fn compressible_space_is_shared_without_flattening_short_names() {
        let allocation = allocate_breadcrumb_widths(&[60.0, 180.0], &[48.0, 48.0], 12.0, 180.0);

        assert!(allocation.segment_widths[0] > 48.0);
        assert!(allocation.segment_widths[1] > allocation.segment_widths[0]);
        assert!((allocation.content_width - 180.0).abs() <= f32::EPSILON);
    }

    #[test]
    fn minimum_widths_overflow_after_compression_is_exhausted() {
        let allocation = allocate_breadcrumb_widths(&[100.0, 120.0], &[64.0, 64.0], 12.0, 100.0);

        assert_eq!(allocation.segment_widths, vec![64.0, 64.0]);
        assert_eq!(allocation.content_width, 140.0);
        assert!(allocation.overflows);
    }

    #[test]
    fn session_identity_and_generation_reject_stale_requests() {
        let pane_id = BrowserPaneId::PRIMARY;
        let mut session =
            AddressEditingSession::new(pane_id, AddressEditingSessionId(7), Path::new("/tmp"));
        session.draft = "docs".to_owned();
        let stale_request = session.next_suggestion_request(Path::new("/tmp"));
        let current_request = session.next_suggestion_request(Path::new("/tmp"));

        assert!(!session.matches_suggestion_request(&stale_request, Path::new("/tmp")));
        assert!(session.matches_suggestion_request(&current_request, Path::new("/tmp")));

        let replacement =
            AddressEditingSession::new(pane_id, AddressEditingSessionId(8), Path::new("/tmp"));
        assert!(!replacement.matches_suggestion_request(&current_request, Path::new("/tmp")));
    }

    #[test]
    fn transition_reverses_from_current_fraction() {
        let started_at = Instant::now();
        let opening =
            AddressBarTransition::retarget(None, BrowserPaneId::PRIMARY, 1.0, None, started_at);
        let reversed_at = started_at + Duration::from_millis(80);
        let visible_fraction = opening.fraction_at(reversed_at);
        let closing = AddressBarTransition::retarget(
            Some(&opening),
            BrowserPaneId::PRIMARY,
            0.0,
            Some("/tmp".to_owned()),
            reversed_at,
        );

        assert!((closing.fraction_at(reversed_at) - visible_fraction).abs() <= f32::EPSILON);
        assert_eq!(
            closing.fraction_at(reversed_at + ADDRESS_BAR_TRANSITION_DURATION),
            0.0
        );
    }
}
