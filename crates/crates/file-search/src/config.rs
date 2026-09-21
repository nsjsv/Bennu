use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::path_encoding::serde_path_vec;

pub(crate) const MAX_CONFIGURED_ROOTS: usize = 32;
pub(crate) const MAX_CONFIGURED_EXCLUDED_PATHS: usize = 256;
pub(crate) const MAX_CONFIGURED_PATH_BYTES: usize = 4_096;
pub(crate) const MAX_CONFIGURED_TOTAL_PATH_BYTES: usize = 262_144;
pub(crate) const MAX_EXTRACT_BYTES: u64 = 2_000_000;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchPathPreferences {
    #[serde(with = "serde_path_vec")]
    pub custom_roots: Vec<PathBuf>,
    #[serde(with = "serde_path_vec")]
    pub exclusions: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionedSearchPathPreferences {
    pub revision: u64,
    pub preferences: SearchPathPreferences,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchPathDecision<'a> {
    Included { owning_root: &'a Path },
    Excluded { boundary: &'a Path },
    OutsideIndex,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchPathPolicyChange {
    pub(crate) affected_scopes: Vec<PathBuf>,
    pub(crate) newly_included_frontiers: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct SearchPathPolicy {
    roots: Vec<PathBuf>,
    preferences: SearchPathPreferences,
}

impl SearchPathPolicy {
    pub fn new(home: PathBuf, preferences: SearchPathPreferences) -> Result<Self, String> {
        let home = normalize_absolute_path(&home)?;
        validate_path_size(&home)?;
        let custom_roots = normalize_unique_paths(preferences.custom_roots)?;
        let exclusions = normalize_unique_paths(preferences.exclusions)?;

        if custom_roots.len() > MAX_CONFIGURED_ROOTS {
            return Err(format!(
                "configured custom search roots exceed the {MAX_CONFIGURED_ROOTS} entry limit"
            ));
        }
        if exclusions.len() > MAX_CONFIGURED_EXCLUDED_PATHS {
            return Err(format!(
                "configured search exclusions exceed the {MAX_CONFIGURED_EXCLUDED_PATHS} entry limit"
            ));
        }

        let mut total_path_bytes = 0_usize;
        for path in custom_roots.iter().chain(&exclusions) {
            validate_path_size(path)?;
            total_path_bytes = total_path_bytes.saturating_add(path_byte_len(path));
            if total_path_bytes > MAX_CONFIGURED_TOTAL_PATH_BYTES {
                return Err(format!(
                    "configured search paths exceed the {MAX_CONFIGURED_TOTAL_PATH_BYTES} byte total limit"
                ));
            }
        }

        if custom_roots.iter().any(|root| root == &home) {
            return Err("the Home search root is implicit and cannot be added again".to_owned());
        }
        let mut included_paths = custom_roots.iter().collect::<HashSet<_>>();
        included_paths.insert(&home);
        if let Some(conflict) = exclusions
            .iter()
            .find(|excluded| included_paths.contains(excluded))
        {
            return Err(format!(
                "the same path cannot be indexed and excluded: {}",
                conflict.display()
            ));
        }

        let preferences = SearchPathPreferences {
            custom_roots,
            exclusions,
        };
        let mut roots = Vec::with_capacity(preferences.custom_roots.len() + 1);
        roots.push(home);
        roots.extend(preferences.custom_roots.iter().cloned());
        Ok(Self { roots, preferences })
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    pub fn preferences(&self) -> &SearchPathPreferences {
        &self.preferences
    }

    pub fn exclusions(&self) -> &[PathBuf] {
        &self.preferences.exclusions
    }

    pub fn decision<'a>(&'a self, path: &Path) -> SearchPathDecision<'a> {
        let included = deepest_ancestor(&self.roots, path);
        let excluded = deepest_ancestor(&self.preferences.exclusions, path);
        match (included, excluded) {
            (None, _) => SearchPathDecision::OutsideIndex,
            (Some(root), None) => SearchPathDecision::Included { owning_root: root },
            (Some(root), Some(boundary))
                if root.components().count() > boundary.components().count() =>
            {
                SearchPathDecision::Included { owning_root: root }
            }
            (Some(_), Some(boundary)) => SearchPathDecision::Excluded { boundary },
        }
    }

    pub(crate) fn change_to(&self, next: &Self) -> SearchPathPolicyChange {
        let mut changed_boundaries = Vec::new();
        for boundary in self
            .roots
            .iter()
            .chain(&self.preferences.exclusions)
            .chain(next.roots.iter())
            .chain(&next.preferences.exclusions)
        {
            let root_changed = self.roots.contains(boundary) != next.roots.contains(boundary);
            let exclusion_changed = self.preferences.exclusions.contains(boundary)
                != next.preferences.exclusions.contains(boundary);
            if (root_changed || exclusion_changed) && !changed_boundaries.contains(boundary) {
                changed_boundaries.push(boundary.clone());
            }
        }

        let newly_included_frontiers = changed_boundaries
            .iter()
            .filter(|boundary| {
                included_owner(next, boundary)
                    .is_some_and(|next_owner| included_owner(self, boundary) != Some(next_owner))
            })
            .cloned()
            .collect();

        changed_boundaries.sort_by(|left, right| {
            left.components()
                .count()
                .cmp(&right.components().count())
                .then_with(|| left.cmp(right))
        });
        let mut affected_scopes = Vec::<PathBuf>::new();
        for boundary in changed_boundaries {
            if !affected_scopes
                .iter()
                .any(|ancestor| boundary.starts_with(ancestor))
            {
                affected_scopes.push(boundary);
            }
        }

        SearchPathPolicyChange {
            affected_scopes,
            newly_included_frontiers,
        }
    }

    fn directory_fallback_includes(&self, path: &Path) -> bool {
        let included = deepest_ancestor(&self.roots, path);
        let excluded = deepest_ancestor(&self.preferences.exclusions, path);
        match excluded {
            None => true,
            Some(boundary) => included
                .is_some_and(|root| root.components().count() > boundary.components().count()),
        }
    }
}

fn included_owner<'a>(policy: &'a SearchPathPolicy, path: &Path) -> Option<&'a Path> {
    match policy.decision(path) {
        SearchPathDecision::Included { owning_root } => Some(owning_root),
        SearchPathDecision::Excluded { .. } | SearchPathDecision::OutsideIndex => None,
    }
}

fn normalize_unique_paths(paths: Vec<PathBuf>) -> Result<Vec<PathBuf>, String> {
    let mut seen = HashSet::with_capacity(paths.len());
    let mut normalized = Vec::with_capacity(paths.len());
    for path in paths {
        let path = normalize_absolute_path(&path)?;
        if seen.insert(path.clone()) {
            normalized.push(path);
        }
    }
    Ok(normalized)
}

fn normalize_absolute_path(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!(
            "configured search path is not absolute: {}",
            path.display()
        ));
    }

    let mut normalized = PathBuf::new();
    let mut normal_components = 0_usize;
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if normal_components == 0 {
                    return Err(format!(
                        "configured search path escapes its root: {}",
                        path.display()
                    ));
                }
                normalized.pop();
                normal_components -= 1;
            }
            Component::Normal(name) => {
                normalized.push(name);
                normal_components += 1;
            }
        }
    }
    Ok(normalized)
}

fn deepest_ancestor<'a>(candidates: &'a [PathBuf], path: &Path) -> Option<&'a Path> {
    candidates
        .iter()
        .filter(|candidate| path.starts_with(candidate))
        .max_by_key(|candidate| candidate.components().count())
        .map(PathBuf::as_path)
}

fn path_byte_len(path: &Path) -> usize {
    path.as_os_str().as_encoded_bytes().len()
}

fn validate_path_size(path: &Path) -> Result<(), String> {
    if path_byte_len(path) > MAX_CONFIGURED_PATH_BYTES {
        return Err(format!(
            "configured search path exceeds the {MAX_CONFIGURED_PATH_BYTES} byte limit: {}",
            path.display()
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchIndexConfig {
    pub roots: Vec<PathBuf>,
    pub excluded_paths: Vec<PathBuf>,
    pub unavailable_roots: Vec<PathBuf>,
    pub content_indexing_enabled: bool,
    pub max_extract_bytes: u64,
}

impl Default for SearchIndexConfig {
    fn default() -> Self {
        let roots = dirs::home_dir().into_iter().collect();
        Self {
            roots,
            excluded_paths: Vec::new(),
            unavailable_roots: Vec::new(),
            content_indexing_enabled: true,
            max_extract_bytes: 2_000_000,
        }
    }
}

impl SearchIndexConfig {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.roots.len() > MAX_CONFIGURED_ROOTS + 1 {
            return Err(format!(
                "configured effective search roots exceed the {} entry limit",
                MAX_CONFIGURED_ROOTS + 1
            ));
        }
        if self.excluded_paths.len() > MAX_CONFIGURED_EXCLUDED_PATHS {
            return Err(format!(
                "configured search exclusions exceed the {MAX_CONFIGURED_EXCLUDED_PATHS} entry limit"
            ));
        }
        let unavailable = self.unavailable_roots.iter().collect::<HashSet<_>>();
        if unavailable.len() != self.unavailable_roots.len()
            || unavailable.iter().any(|root| !self.roots.contains(root))
        {
            return Err("unavailable search roots must be unique configured roots".to_owned());
        }
        if self.max_extract_bytes > MAX_EXTRACT_BYTES {
            return Err(format!(
                "configured extraction limit {} exceeds the {MAX_EXTRACT_BYTES} byte service budget",
                self.max_extract_bytes
            ));
        }

        let mut total_path_bytes = 0_usize;
        for path in self.roots.iter().chain(&self.excluded_paths) {
            validate_path_size(path)?;
            total_path_bytes = total_path_bytes.saturating_add(path_byte_len(path));
            if total_path_bytes > MAX_CONFIGURED_TOTAL_PATH_BYTES {
                return Err(format!(
                    "configured search paths exceed the {MAX_CONFIGURED_TOTAL_PATH_BYTES} byte total limit"
                ));
            }
        }
        if !self.roots.is_empty() {
            self.search_path_policy()?;
        }
        Ok(())
    }

    pub(crate) fn available_roots(&self) -> impl Iterator<Item = &PathBuf> {
        self.roots
            .iter()
            .filter(|root| !self.unavailable_roots.contains(root))
    }

    pub(crate) fn owning_root(&self, path: &Path) -> Option<&Path> {
        self.roots
            .iter()
            .filter(|root| path.starts_with(root))
            .max_by_key(|root| root.components().count())
            .map(PathBuf::as_path)
    }

    pub(crate) fn path_is_available(&self, path: &Path) -> bool {
        self.owning_root(path).is_some_and(|root| {
            !self
                .unavailable_roots
                .iter()
                .any(|unavailable| unavailable == root)
        })
    }

    pub(crate) fn search_path_policy(&self) -> Result<Option<SearchPathPolicy>, String> {
        let Some(home) = self.roots.first() else {
            return Ok(None);
        };
        SearchPathPolicy::new(
            home.clone(),
            SearchPathPreferences {
                custom_roots: self.roots[1..].to_vec(),
                exclusions: self.excluded_paths.clone(),
            },
        )
        .map(Some)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchTraversalDecision {
    Included,
    Excluded,
    DelegatedToNestedRoot,
}

#[derive(Debug, Clone)]
enum SearchExclusionPolicy {
    ExplicitPaths(Vec<PathBuf>),
    IndexedRoots(SearchPathPolicy),
    DirectoryFallback(SearchPathPolicy),
}

#[derive(Debug, Clone)]
pub struct SearchExcludeRules {
    policy: SearchExclusionPolicy,
}

impl SearchExcludeRules {
    pub fn new(excluded_paths: Vec<PathBuf>) -> Self {
        Self {
            policy: SearchExclusionPolicy::ExplicitPaths(excluded_paths),
        }
    }

    pub fn for_directory_fallback(
        home: PathBuf,
        preferences: SearchPathPreferences,
    ) -> Result<Self, String> {
        Ok(Self {
            policy: SearchExclusionPolicy::DirectoryFallback(SearchPathPolicy::new(
                home,
                preferences,
            )?),
        })
    }

    pub(crate) fn from_index_config(config: &SearchIndexConfig) -> Result<Self, String> {
        Ok(Self {
            policy: match config.search_path_policy()? {
                Some(policy) => SearchExclusionPolicy::IndexedRoots(policy),
                None => SearchExclusionPolicy::ExplicitPaths(Vec::new()),
            },
        })
    }

    pub(crate) fn traversal_decision(
        &self,
        active_root: &Path,
        path: &Path,
    ) -> SearchTraversalDecision {
        match &self.policy {
            SearchExclusionPolicy::IndexedRoots(policy) => match policy.decision(path) {
                SearchPathDecision::Included { owning_root } if owning_root == active_root => {
                    SearchTraversalDecision::Included
                }
                SearchPathDecision::Included { .. } => {
                    SearchTraversalDecision::DelegatedToNestedRoot
                }
                SearchPathDecision::Excluded { .. } | SearchPathDecision::OutsideIndex => {
                    SearchTraversalDecision::Excluded
                }
            },
            SearchExclusionPolicy::DirectoryFallback(policy) => {
                if policy.directory_fallback_includes(path) {
                    SearchTraversalDecision::Included
                } else {
                    SearchTraversalDecision::Excluded
                }
            }
            SearchExclusionPolicy::ExplicitPaths(excluded_paths)
                if excluded_paths
                    .iter()
                    .any(|excluded| path == excluded || path.starts_with(excluded)) =>
            {
                SearchTraversalDecision::Excluded
            }
            SearchExclusionPolicy::ExplicitPaths(_) => SearchTraversalDecision::Included,
        }
    }

    pub(crate) fn directory_traversal_decision(
        &self,
        active_root: &Path,
        path: &Path,
    ) -> SearchTraversalDecision {
        match self.traversal_decision(active_root, path) {
            SearchTraversalDecision::Included
                if is_hidden_name(path.file_name())
                    || has_ignored_directory_name(path.file_name())
                    || path.join(".nomedia").is_file() =>
            {
                SearchTraversalDecision::Excluded
            }
            decision => decision,
        }
    }

    pub fn should_skip_directory(&self, path: &Path) -> bool {
        self.should_skip_path(path)
            || is_hidden_name(path.file_name())
            || has_ignored_directory_name(path.file_name())
            || path.join(".nomedia").is_file()
    }

    pub fn should_skip_path(&self, path: &Path) -> bool {
        match &self.policy {
            SearchExclusionPolicy::IndexedRoots(policy) => {
                !matches!(policy.decision(path), SearchPathDecision::Included { .. })
            }
            SearchExclusionPolicy::DirectoryFallback(policy) => {
                !policy.directory_fallback_includes(path)
            }
            SearchExclusionPolicy::ExplicitPaths(excluded_paths) => excluded_paths
                .iter()
                .any(|excluded| path == excluded || path.starts_with(excluded)),
        }
    }
}

fn is_hidden_name(name: Option<&OsStr>) -> bool {
    name.and_then(OsStr::to_str)
        .is_some_and(|name| name.starts_with('.') && name != "." && name != "..")
}

fn has_ignored_directory_name(name: Option<&OsStr>) -> bool {
    matches!(
        name.and_then(OsStr::to_str),
        Some(".git" | "node_modules" | "target" | ".cache")
    )
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{
        SearchExcludeRules, SearchIndexConfig, SearchPathDecision, SearchPathPolicy,
        SearchPathPolicyChange, SearchPathPreferences, MAX_CONFIGURED_EXCLUDED_PATHS,
        MAX_CONFIGURED_PATH_BYTES, MAX_CONFIGURED_ROOTS, MAX_EXTRACT_BYTES,
    };

    #[test]
    fn default_content_payload_limit_is_two_decimal_megabytes() {
        assert_eq!(SearchIndexConfig::default().max_extract_bytes, 2_000_000);
    }

    #[test]
    fn skips_hidden_and_configured_directories() {
        let rules = SearchExcludeRules::new(vec![Path::new("/home/me/private").to_path_buf()]);

        assert!(rules.should_skip_directory(Path::new("/home/me/.hidden")));
        assert!(rules.should_skip_directory(Path::new("/home/me/project/.git")));
        assert!(rules.should_skip_directory(Path::new("/home/me/private")));
        assert!(!rules.should_skip_directory(Path::new("/home/me/Documents")));
    }

    #[test]
    fn configured_exclusion_applies_to_files_without_content_size_policy() {
        let rules = SearchExcludeRules::new(vec![Path::new("/home/me/private").to_path_buf()]);

        assert!(rules.should_skip_path(Path::new("/home/me/private/big.txt")));
        assert!(!rules.should_skip_path(Path::new("/home/me/public/big.txt")));
    }

    #[test]
    fn directory_fallback_respects_exclusions_and_nested_reincluded_roots() {
        let rules = SearchExcludeRules::for_directory_fallback(
            PathBuf::from("/home/me"),
            SearchPathPreferences {
                custom_roots: vec![PathBuf::from("/archive/private/reincluded")],
                exclusions: vec![
                    PathBuf::from("/home/me/private"),
                    PathBuf::from("/archive/private"),
                    PathBuf::from("/outside/secret"),
                ],
            },
        )
        .unwrap();

        assert!(rules.should_skip_path(Path::new("/home/me/private/file.txt")));
        assert!(rules.should_skip_path(Path::new("/outside/secret/file.txt")));
        assert!(!rules.should_skip_path(Path::new("/archive/private/reincluded/file.txt")));
        assert!(!rules.should_skip_path(Path::new("/outside/public/file.txt")));
    }

    #[test]
    fn service_configuration_accepts_values_within_resource_boundaries() {
        SearchIndexConfig::default().validate().unwrap();
        SearchIndexConfig {
            roots: Vec::new(),
            excluded_paths: Vec::new(),
            unavailable_roots: Vec::new(),
            content_indexing_enabled: false,
            max_extract_bytes: 0,
        }
        .validate()
        .unwrap();
    }

    #[test]
    fn service_configuration_rejects_unbounded_collections_and_payloads() {
        let too_many_roots = SearchIndexConfig {
            roots: vec![PathBuf::from("/tmp"); MAX_CONFIGURED_ROOTS + 2],
            ..SearchIndexConfig::default()
        };
        assert!(too_many_roots.validate().is_err());

        let too_many_exclusions = SearchIndexConfig {
            excluded_paths: vec![PathBuf::from("/tmp"); MAX_CONFIGURED_EXCLUDED_PATHS + 1],
            ..SearchIndexConfig::default()
        };
        assert!(too_many_exclusions.validate().is_err());

        let oversized_path = SearchIndexConfig {
            roots: vec![PathBuf::from("x".repeat(MAX_CONFIGURED_PATH_BYTES + 1))],
            ..SearchIndexConfig::default()
        };
        assert!(oversized_path.validate().is_err());

        let oversized_extraction = SearchIndexConfig {
            max_extract_bytes: MAX_EXTRACT_BYTES + 1,
            ..SearchIndexConfig::default()
        };
        assert!(oversized_extraction.validate().is_err());
    }

    #[test]
    fn nested_path_rules_use_the_deepest_matching_boundary() {
        let policy = SearchPathPolicy::new(
            PathBuf::from("/a"),
            SearchPathPreferences {
                custom_roots: vec![PathBuf::from("/a/b/c")],
                exclusions: vec![PathBuf::from("/a/b"), PathBuf::from("/a/b/c/tmp")],
            },
        )
        .unwrap();

        assert_eq!(
            policy.decision(Path::new("/a/document.txt")),
            SearchPathDecision::Included {
                owning_root: Path::new("/a")
            }
        );
        assert_eq!(
            policy.decision(Path::new("/a/b/private.txt")),
            SearchPathDecision::Excluded {
                boundary: Path::new("/a/b")
            }
        );
        assert_eq!(
            policy.decision(Path::new("/a/b/c/reincluded.txt")),
            SearchPathDecision::Included {
                owning_root: Path::new("/a/b/c")
            }
        );
        assert_eq!(
            policy.decision(Path::new("/a/b/c/tmp/ignored.txt")),
            SearchPathDecision::Excluded {
                boundary: Path::new("/a/b/c/tmp")
            }
        );
        assert_eq!(
            policy.decision(Path::new("/outside")),
            SearchPathDecision::OutsideIndex
        );
    }

    #[test]
    fn policy_change_limits_scans_and_surfaces_newly_included_frontiers() {
        let previous = SearchPathPolicy::new(
            PathBuf::from("/a"),
            SearchPathPreferences {
                custom_roots: vec![PathBuf::from("/archive")],
                exclusions: vec![PathBuf::from("/a/b")],
            },
        )
        .unwrap();
        let next = SearchPathPolicy::new(
            PathBuf::from("/a"),
            SearchPathPreferences {
                custom_roots: vec![PathBuf::from("/archive"), PathBuf::from("/mnt/disk")],
                exclusions: vec![PathBuf::from("/a/b/c/tmp")],
            },
        )
        .unwrap();

        assert_eq!(
            previous.change_to(&next),
            SearchPathPolicyChange {
                affected_scopes: vec![PathBuf::from("/a/b"), PathBuf::from("/mnt/disk")],
                newly_included_frontiers: vec![PathBuf::from("/a/b"), PathBuf::from("/mnt/disk"),],
            }
        );
        assert_eq!(
            next.change_to(&next),
            SearchPathPolicyChange {
                affected_scopes: Vec::new(),
                newly_included_frontiers: Vec::new(),
            }
        );
    }

    #[test]
    fn path_policy_normalizes_and_deduplicates_without_touching_the_filesystem() {
        let policy = SearchPathPolicy::new(
            PathBuf::from("/home/me"),
            SearchPathPreferences {
                custom_roots: vec![
                    PathBuf::from("/missing/one/../root"),
                    PathBuf::from("/missing/root"),
                ],
                exclusions: vec![PathBuf::from("/future/./excluded")],
            },
        )
        .unwrap();

        assert_eq!(
            policy.preferences().custom_roots,
            vec![PathBuf::from("/missing/root")]
        );
        assert_eq!(
            policy.preferences().exclusions,
            vec![PathBuf::from("/future/excluded")]
        );
    }

    #[test]
    fn path_policy_rejects_relative_duplicate_home_and_include_exclude_conflicts() {
        assert!(SearchPathPolicy::new(
            PathBuf::from("/home/me"),
            SearchPathPreferences {
                custom_roots: vec![PathBuf::from("relative")],
                exclusions: Vec::new(),
            },
        )
        .is_err());
        assert!(SearchPathPolicy::new(
            PathBuf::from("/home/me"),
            SearchPathPreferences {
                custom_roots: vec![PathBuf::from("/home/me")],
                exclusions: Vec::new(),
            },
        )
        .is_err());
        assert!(SearchPathPolicy::new(
            PathBuf::from("/home/me"),
            SearchPathPreferences {
                custom_roots: vec![PathBuf::from("/data/root")],
                exclusions: vec![PathBuf::from("/data/./root")],
            },
        )
        .is_err());
    }
}
