use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use crate::animation::{ease_out_cubic, elapsed_fraction_at};

pub const ADDRESS_BAR_TRANSITION_DURATION: Duration = Duration::from_millis(160);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BreadcrumbSegmentKind {
    Home,
    Root,
    Name(OsString),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreadcrumbSegment {
    pub target: PathBuf,
    pub kind: BreadcrumbSegmentKind,
    pub is_current: bool,
}

impl BreadcrumbSegment {
    pub fn display_text(&self) -> String {
        match &self.kind {
            BreadcrumbSegmentKind::Home => String::new(),
            BreadcrumbSegmentKind::Root => std::path::MAIN_SEPARATOR.to_string(),
            BreadcrumbSegmentKind::Name(name) => name.to_string_lossy().into_owned(),
        }
    }
}

pub fn breadcrumb_segments(current_dir: &Path, home_dir: &Path) -> Vec<BreadcrumbSegment> {
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
pub struct BreadcrumbWidthAllocation {
    pub segment_widths: Vec<f32>,
    pub content_width: f32,
    pub overflows: bool,
}

pub fn allocate_breadcrumb_widths(
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
pub struct AddressEditingSessionId(pub u64);

/// 路径补全请求的防陈旧凭据：只携带会话身份/草稿/基准目录/代数。
/// 窗格归属是多窗格前端（主程序）的外部关注点，portal 无此概念，故不进共享层。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressSuggestionRequest {
    pub session_id: AddressEditingSessionId,
    pub draft: String,
    pub current_dir: PathBuf,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressEditingSession {
    pub session_id: AddressEditingSessionId,
    pub draft: String,
    pub suggestions: Vec<PathBuf>,
    pub suggestion_selection: Option<usize>,
    pub generation: u64,
}

impl AddressEditingSession {
    pub fn new(session_id: AddressEditingSessionId, current_dir: &Path) -> Self {
        Self {
            session_id,
            draft: current_dir.to_string_lossy().into_owned(),
            suggestions: Vec::new(),
            suggestion_selection: None,
            generation: 0,
        }
    }

    pub fn next_suggestion_request(&mut self, current_dir: &Path) -> AddressSuggestionRequest {
        self.generation = self.generation.wrapping_add(1);
        AddressSuggestionRequest {
            session_id: self.session_id,
            draft: self.draft.clone(),
            current_dir: current_dir.to_path_buf(),
            generation: self.generation,
        }
    }

    pub fn matches_suggestion_request(
        &self,
        request: &AddressSuggestionRequest,
        current_dir: &Path,
    ) -> bool {
        self.session_id == request.session_id
            && self.draft == request.draft
            && current_dir == request.current_dir
            && self.generation == request.generation
    }
}

#[derive(Debug, Clone)]
pub struct AddressBarTransition {
    start_fraction: f32,
    target_fraction: f32,
    started_at: Instant,
    duration: Duration,
    pub exit_snapshot: Option<String>,
}

impl AddressBarTransition {
    /// 多窗格调用方需先把 previous 过滤成本窗格的过渡再传入；
    /// 跨窗格的旧过渡与本窗格无连续性，应传 None 从静止端起步。
    pub fn retarget(
        previous: Option<&Self>,
        target_fraction: f32,
        exit_snapshot: Option<String>,
        now: Instant,
    ) -> Self {
        let target_fraction = target_fraction.clamp(0.0, 1.0);
        let start_fraction = previous
            .map(|transition| transition.fraction_at(now))
            .unwrap_or(1.0 - target_fraction);
        let distance = (target_fraction - start_fraction).abs();
        let duration = ADDRESS_BAR_TRANSITION_DURATION.mul_f32(distance);

        Self {
            start_fraction,
            target_fraction,
            started_at: now,
            duration,
            exit_snapshot,
        }
    }

    pub fn fraction(&self) -> f32 {
        self.fraction_at(Instant::now())
    }

    pub fn fraction_at(&self, now: Instant) -> f32 {
        let progress = elapsed_fraction_at(self.started_at, now, self.duration);
        let eased_progress = ease_out_cubic(progress);
        self.start_fraction + (self.target_fraction - self.start_fraction) * eased_progress
    }

    pub fn is_complete_at(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started_at) >= self.duration
    }

    pub fn is_complete(&self) -> bool {
        self.is_complete_at(Instant::now())
    }

    pub fn target_fraction(&self) -> f32 {
        self.target_fraction
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;

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
        let mut session = AddressEditingSession::new(AddressEditingSessionId(7), Path::new("/tmp"));
        session.draft = "docs".to_owned();
        let stale_request = session.next_suggestion_request(Path::new("/tmp"));
        let current_request = session.next_suggestion_request(Path::new("/tmp"));

        assert!(!session.matches_suggestion_request(&stale_request, Path::new("/tmp")));
        assert!(session.matches_suggestion_request(&current_request, Path::new("/tmp")));

        let replacement = AddressEditingSession::new(AddressEditingSessionId(8), Path::new("/tmp"));
        assert!(!replacement.matches_suggestion_request(&current_request, Path::new("/tmp")));
    }

    #[test]
    fn transition_reverses_from_current_fraction() {
        let started_at = Instant::now();
        let opening = AddressBarTransition::retarget(None, 1.0, None, started_at);
        let reversed_at = started_at + Duration::from_millis(80);
        let visible_fraction = opening.fraction_at(reversed_at);
        let closing = AddressBarTransition::retarget(
            Some(&opening),
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
