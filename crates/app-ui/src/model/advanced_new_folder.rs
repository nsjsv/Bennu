//! 高级新建文件夹弹窗的模型:两种批量命名模式与最终名字计划。
//! 名字错开用 file-core 的候选序列(`entry_name_candidates`),与执行侧
//! AlreadyExists 原子裁决同一套语义。

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::PathBuf;

use file_core::{entry_name_candidates, validate_entry_name, EntryNameKind, InvalidEntryName};
use iced::widget::text_editor;

/// 预览里一行的状态。后两种会让创建失败或无法判断会不会建错，确认因此禁用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlannedFolderStatus {
    AsRequested,
    /// 与磁盘或批内重名，已按"名字 2"错开。
    AutoRenamed,
    /// 某一段名字不能用（`.`、`..`、含 NUL）。
    InvalidName(InvalidEntryName),
    /// 嵌套链的父目录读不到内容，无法判断重名；附读错误原文。
    UnreadableParent(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedFolder {
    /// 相对目标目录的叶子路径，段间 `/` 表示嵌套。
    pub(crate) path: String,
    pub(crate) status: PlannedFolderStatus,
}

impl PlannedFolder {
    fn creatable(&self) -> bool {
        matches!(
            self.status,
            PlannedFolderStatus::AsRequested | PlannedFolderStatus::AutoRenamed
        )
    }
}

/// 嵌套链父目录的内容探测结果。目录还不存在算作空（会由本次新建）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NestedParentListing {
    Names(HashSet<String>),
    Unreadable(String),
}

/// 编号模式的数量上限:批量建目录是显式动作,500 足够覆盖
/// 整理场景,同时防误输入 100000 之类的值拖垮队列。
pub(crate) const ADVANCED_NEW_FOLDER_MAX_COUNT: u32 = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdvancedNewFolderMode {
    /// 每行一个名字。
    NameList,
    /// 前缀 + 起始编号 + 数量。
    Numbered,
}

/// 创建完成后的收尾动作：None = 默认进重命名态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdvancedNewFolderAfter {
    Enter,
    EnterInNewTab,
}

#[derive(Debug, Clone)]
pub(crate) struct AdvancedNewFolderState {
    pub(crate) directory: PathBuf,
    pub(crate) mode: AdvancedNewFolderMode,
    /// NameList 模式:text_editor 的内容,每次编辑同步为纯文本。
    pub(crate) editor_content: text_editor::Content,
    /// NameList 模式:每行一个名字,plan() 消费。
    pub(crate) draft: String,
    /// Numbered 模式:前缀(可为空)。
    pub(crate) prefix: String,
    /// Numbered 模式:起始编号,text_input 原始文本,解析失败按 1。
    pub(crate) start_number: String,
    /// Numbered 模式:数量,text_input 原始文本。
    pub(crate) count: String,
    /// 建完把选中条目移入第一个新文件夹。
    pub(crate) gather: bool,
    /// 建完移入的候选源(目标目录下的选中项);空 = 无移入能力。
    pub(crate) gather_sources: Vec<PathBuf>,
    /// 创建按钮旁的下拉菜单是否展开(点箭头打开,点菜单外/选完/Esc 关闭)。
    pub(crate) after_menu_open: bool,
    /// 鼠标悬停在分裂按钮上(整个按钮统一提亮)。
    pub(crate) split_button_hover: bool,
    /// 打开弹窗时 directory 已有的条目名,plan() 用它错开磁盘重名。
    taken_names: HashSet<String>,
    /// 嵌套链父目录的内容快照(key = 相对前缀,如 "test" 或 "a/b"),
    /// 由应用层异步探测回填;plan() 用它错开嵌套叶子段的磁盘重名。
    pub(crate) taken_deep: HashMap<String, NestedParentListing>,
}

impl AdvancedNewFolderState {
    pub(crate) fn new(
        directory: PathBuf,
        existing_names: HashSet<String>,
        gather_sources: Vec<PathBuf>,
    ) -> Self {
        Self {
            directory,
            mode: AdvancedNewFolderMode::NameList,
            editor_content: text_editor::Content::new(),
            draft: String::new(),
            prefix: String::new(),
            start_number: "1".to_owned(),
            count: "3".to_owned(),
            gather: !gather_sources.is_empty(),
            gather_sources,
            after_menu_open: false,
            split_button_hover: false,
            taken_names: existing_names,
            taken_deep: HashMap::new(),
        }
    }

    /// 编号模式的位数:至少两位(01),超过两位跟随总数(100…是三位)。
    fn numbered_width(start: u32, count: u32) -> usize {
        (start + count - 1).to_string().len().max(2)
    }

    /// Numbered 模式的请求名序列。解析失败按默认值收敛,不做报错打断。
    fn numbered_names(&self) -> Vec<String> {
        let start = self
            .start_number
            .trim()
            .parse::<u32>()
            .unwrap_or(1)
            .min(99_999);
        let count = self
            .count
            .trim()
            .parse::<u32>()
            .unwrap_or(1)
            .clamp(1, ADVANCED_NEW_FOLDER_MAX_COUNT);
        let width = Self::numbered_width(start, count);
        (start..start + count)
            .map(|number| format!("{}{:0width$}", self.prefix, number, width = width))
            .collect()
    }

    /// NameList 模式的请求名序列:每行 trim 非空,保留内部 '/' 表达嵌套链。
    fn listed_names(&self) -> Vec<String> {
        self.draft
            .split('\n')
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect()
    }

    /// 批内与磁盘重名错开后的最终叶子路径；有不能创建的行时为 None。
    pub(crate) fn plan(&self) -> Option<Vec<String>> {
        let planned = self.plan_detailed();
        (!planned.is_empty() && planned.iter().all(PlannedFolder::creatable))
            .then(|| planned.into_iter().map(|folder| folder.path).collect())
    }

    /// 能否确认创建：至少一行，且每一行都能按预览建出来。
    pub(crate) fn can_confirm(&self) -> bool {
        self.plan().is_some()
    }

    /// 每一行的最终叶子路径与状态，预览据此标注撞车与问题行。
    /// - 单段行沿用共享命名规则错开磁盘与批内重名(名字、名字 2...);
    /// - 多段行(嵌套链)的中间段允许复用已有目录,只有完整叶子路径在
    ///   批内重复时才错开最后一段;
    /// - 每一段都按执行端的 `validate_entry_name` 校验。
    /// 执行侧逐段创建,AlreadyExists 裁决兜底磁盘深层重名与极端竞争。
    pub(crate) fn plan_detailed(&self) -> Vec<PlannedFolder> {
        let requested = match self.mode {
            AdvancedNewFolderMode::NameList => self.listed_names(),
            AdvancedNewFolderMode::Numbered => self.numbered_names(),
        };
        let mut taken = self.taken_names.clone();
        let mut planned_leaves: Vec<PlannedFolder> = Vec::with_capacity(requested.len());
        for name in requested {
            let segments: Vec<&str> = name
                .split('/')
                .map(str::trim)
                .filter(|segment| !segment.is_empty())
                .collect();
            if let Some(reason) = segments
                .iter()
                .find_map(|segment| validate_entry_name(OsStr::new(segment)).err())
            {
                planned_leaves.push(PlannedFolder {
                    path: segments.join("/"),
                    status: PlannedFolderStatus::InvalidName(reason),
                });
                continue;
            }
            match segments.as_slice() {
                [] => continue,
                [single] => {
                    let name = if taken.contains(*single) {
                        entry_name_candidates(OsStr::new(single), EntryNameKind::Directory)
                            .map(|candidate| candidate.to_string_lossy().into_owned())
                            .find(|candidate| !taken.contains(candidate))
                            .unwrap_or_else(|| (*single).to_owned())
                    } else {
                        (*single).to_owned()
                    };
                    let status = if name == *single {
                        PlannedFolderStatus::AsRequested
                    } else {
                        PlannedFolderStatus::AutoRenamed
                    };
                    taken.insert(name.clone());
                    planned_leaves.push(PlannedFolder { path: name, status });
                }
                _ => {
                    // 嵌套链:中间段复用已有目录,叶子段对磁盘(深层探测)
                    // 与批内已占用的名字做候选错开,撞车即标注。
                    let (head, last) = segments.split_at(segments.len() - 1);
                    let prefix = head.join("/");
                    let leaf_last = last[0];
                    let leaf = segments.join("/");
                    let mut occupied: HashSet<String> = match self.taken_deep.get(&prefix) {
                        Some(NestedParentListing::Names(names)) => names.clone(),
                        Some(NestedParentListing::Unreadable(error)) => {
                            planned_leaves.push(PlannedFolder {
                                path: leaf,
                                status: PlannedFolderStatus::UnreadableParent(error.clone()),
                            });
                            continue;
                        }
                        // 还没探测回来:先按无重名显示,探测后预览刷新。
                        None => HashSet::new(),
                    };
                    for seen in &planned_leaves {
                        if let Some((seen_head, seen_last)) = seen.path.rsplit_once('/') {
                            if seen_head == prefix {
                                occupied.insert(seen_last.to_owned());
                            }
                        } else if prefix.is_empty() {
                            occupied.insert(seen.path.clone());
                        }
                    }
                    let leaf = if occupied.contains(leaf_last) {
                        entry_name_candidates(OsStr::new(leaf_last), EntryNameKind::Directory)
                            .map(|candidate| candidate.to_string_lossy().into_owned())
                            .find(|candidate| !occupied.contains(candidate))
                            .map(|candidate| {
                                if prefix.is_empty() {
                                    candidate
                                } else {
                                    format!("{prefix}/{candidate}")
                                }
                            })
                            .unwrap_or_else(|| leaf.clone())
                    } else {
                        leaf
                    };
                    let status = if leaf == segments.join("/") {
                        PlannedFolderStatus::AsRequested
                    } else {
                        PlannedFolderStatus::AutoRenamed
                    };
                    planned_leaves.push(PlannedFolder { path: leaf, status });
                }
            }
        }
        planned_leaves
    }

    /// 嵌套链引用了但还没有内容快照的父目录前缀；应用层按需探测。
    pub(crate) fn needed_prefixes(&self) -> Vec<String> {
        if self.mode != AdvancedNewFolderMode::NameList {
            return Vec::new();
        }
        let referenced: HashSet<String> = self
            .listed_names()
            .iter()
            .filter_map(|line| {
                let segments: Vec<&str> = line
                    .split('/')
                    .map(str::trim)
                    .filter(|segment| !segment.is_empty())
                    .collect();
                (segments.len() >= 2).then(|| segments[..segments.len() - 1].join("/"))
            })
            .collect();
        referenced
            .into_iter()
            .filter(|prefix| !self.taken_deep.contains_key(prefix))
            .collect()
    }
}

#[derive(Debug, Clone)]
pub(crate) enum AdvancedNewFolderMessage {
    ModeSelected(AdvancedNewFolderMode),
    /// text_editor 的编辑动作:先落到 editor_content,再同步纯文本 draft。
    DraftEdited(text_editor::Action),
    PrefixChanged(String),
    StartNumberChanged(String),
    CountChanged(String),
    GatherToggled(bool),
    /// 点箭头打开菜单;点菜单外关闭。
    AfterMenuOpenChanged(bool),
    /// 鼠标进入/离开分裂按钮:只管整体提亮。
    SplitButtonHoverChanged(bool),
    /// 带收尾动作的确认(创建后进入 / 新标签进入)。
    ConfirmedWithAfter(AdvancedNewFolderAfter),
    /// 嵌套链父目录的内容探测回执:预览据此提前标注叶子段磁盘重名或读不了。
    NestedNamesProbed {
        prefix: String,
        listing: NestedParentListing,
    },
    Confirmed,
    Cancel,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(
        directory: &str,
        mode: AdvancedNewFolderMode,
        taken: &[&str],
    ) -> AdvancedNewFolderState {
        AdvancedNewFolderState {
            directory: PathBuf::from(directory),
            mode,
            editor_content: iced::widget::text_editor::Content::new(),
            draft: String::new(),
            prefix: String::new(),
            start_number: "1".to_owned(),
            count: "3".to_owned(),
            gather: false,
            gather_sources: Vec::new(),
            after_menu_open: false,
            split_button_hover: false,
            taken_names: taken.iter().map(|name| name.to_string()).collect(),
            taken_deep: HashMap::new(),
        }
    }

    fn planned_names(advanced: &AdvancedNewFolderState) -> Vec<String> {
        advanced
            .plan()
            .expect("every planned folder can be created")
    }

    fn planned(path: &str, status: PlannedFolderStatus) -> PlannedFolder {
        PlannedFolder {
            path: path.to_owned(),
            status,
        }
    }

    #[test]
    fn listed_names_drop_blank_lines_and_trim() {
        let mut advanced = state("/tmp", AdvancedNewFolderMode::NameList, &[]);
        advanced.draft = "照片\n\n  影像  \r\n".to_owned();
        assert_eq!(planned_names(&advanced), vec!["照片", "影像"]);
    }

    #[test]
    fn draft_edits_flow_into_plan_source() {
        let mut advanced = state("/tmp", AdvancedNewFolderMode::NameList, &[]);
        advanced.editor_content = text_editor::Content::with_text("照片\n影像");
        advanced.draft = advanced.editor_content.text();
        assert_eq!(planned_names(&advanced), vec!["照片", "影像"]);
    }

    #[test]
    fn numbered_names_pad_to_two_digits_and_follow_wider_counts() {
        let mut advanced = state("/tmp", AdvancedNewFolderMode::Numbered, &[]);
        advanced.prefix = "项目".to_owned();
        advanced.start_number = "1".to_owned();
        advanced.count = "10".to_owned();
        let planned = planned_names(&advanced);
        assert_eq!(planned.first().map(String::as_str), Some("项目01"));
        assert_eq!(planned.last().map(String::as_str), Some("项目10"));

        advanced.count = "101".to_owned();
        let planned = planned_names(&advanced);
        assert_eq!(planned.first().map(String::as_str), Some("项目001"));
        assert_eq!(planned.len(), 101);
    }

    #[test]
    fn plan_duplicates_within_batch_get_shared_suffix_rule() {
        let mut advanced = state("/tmp", AdvancedNewFolderMode::NameList, &[]);
        advanced.draft = "照片\n照片\n照片".to_owned();
        assert_eq!(planned_names(&advanced), vec!["照片", "照片 2", "照片 3"]);
    }

    #[test]
    fn plan_avoids_names_taken_on_disk() {
        let mut advanced = state(
            "/tmp",
            AdvancedNewFolderMode::NameList,
            &["新建文件夹", "新建文件夹 2"],
        );
        advanced.draft = "新建文件夹".to_owned();
        assert_eq!(planned_names(&advanced), vec!["新建文件夹 3"]);
    }

    #[test]
    fn invalid_number_fields_fall_back_to_defaults() {
        let mut advanced = state("/tmp", AdvancedNewFolderMode::Numbered, &[]);
        advanced.start_number = "abc".to_owned();
        advanced.count = "0".to_owned();
        assert_eq!(planned_names(&advanced), vec!["01"]);
    }

    #[test]
    fn plan_detailed_flags_only_renamed_entries() {
        let mut advanced = state("/tmp", AdvancedNewFolderMode::NameList, &["已占用"]);
        advanced.draft = "已占用\n新名字\na/b\na/b".to_owned();
        let detailed = advanced.plan_detailed();
        assert_eq!(
            detailed,
            vec![
                planned("已占用 2", PlannedFolderStatus::AutoRenamed),
                planned("新名字", PlannedFolderStatus::AsRequested),
                planned("a/b", PlannedFolderStatus::AsRequested),
                planned("a/b 2", PlannedFolderStatus::AutoRenamed),
            ]
        );
    }

    #[test]
    fn nested_leaf_conflicts_with_deep_disk_entry_get_renamed() {
        let mut advanced = state("/tmp", AdvancedNewFolderMode::NameList, &["test"]);
        advanced.taken_deep.insert(
            "test".to_owned(),
            NestedParentListing::Names(["test".to_owned()].into_iter().collect()),
        );
        advanced.draft = "test/test".to_owned();
        assert_eq!(
            advanced.plan_detailed(),
            vec![planned("test/test 2", PlannedFolderStatus::AutoRenamed)]
        );

        // 快照还没探测回来:不乱改,等探测后预览刷新。
        let mut unprobed = state("/tmp", AdvancedNewFolderMode::NameList, &["test"]);
        unprobed.draft = "test/test".to_owned();
        assert_eq!(
            unprobed.plan_detailed(),
            vec![planned("test/test", PlannedFolderStatus::AsRequested)]
        );
    }

    #[test]
    fn dot_segments_and_nul_are_flagged_and_block_creation() {
        for (line, reason) in [
            ("..", InvalidEntryName::ParentDirectory),
            ("a/./b", InvalidEntryName::CurrentDirectory),
            ("a/../b", InvalidEntryName::ParentDirectory),
            ("nul\0name", InvalidEntryName::ContainsNul),
        ] {
            let mut advanced = state("/tmp", AdvancedNewFolderMode::NameList, &[]);
            advanced.draft = format!("ok\n{line}");

            assert_eq!(
                advanced.plan_detailed()[1].status,
                PlannedFolderStatus::InvalidName(reason),
                "{line:?}"
            );
            assert!(!advanced.can_confirm(), "{line:?}");
            assert_eq!(advanced.plan(), None, "{line:?}");
        }
    }

    #[test]
    fn a_numbered_prefix_that_climbs_out_is_flagged() {
        let mut advanced = state("/tmp", AdvancedNewFolderMode::Numbered, &[]);
        advanced.prefix = "../".to_owned();

        assert!(advanced.plan_detailed().iter().all(|folder| folder.status
            == PlannedFolderStatus::InvalidName(InvalidEntryName::ParentDirectory)));
        assert!(!advanced.can_confirm());
    }

    #[test]
    fn a_nested_line_under_an_unreadable_parent_is_flagged_and_blocks_creation() {
        let mut advanced = state("/tmp", AdvancedNewFolderMode::NameList, &["locked"]);
        advanced.taken_deep.insert(
            "locked".to_owned(),
            NestedParentListing::Unreadable("Permission denied (os error 13)".to_owned()),
        );
        advanced.draft = "fine\nlocked/inner".to_owned();

        assert_eq!(
            advanced.plan_detailed(),
            vec![
                planned("fine", PlannedFolderStatus::AsRequested),
                planned(
                    "locked/inner",
                    PlannedFolderStatus::UnreadableParent(
                        "Permission denied (os error 13)".to_owned()
                    )
                ),
            ]
        );
        assert!(!advanced.can_confirm());
    }

    #[test]
    fn an_empty_plan_cannot_be_confirmed() {
        let advanced = state("/tmp", AdvancedNewFolderMode::NameList, &[]);

        assert_eq!(advanced.plan(), None);
        assert!(!advanced.can_confirm());
    }

    #[test]
    fn nested_paths_stay_as_slash_separated_leaves() {
        let mut advanced = state("/tmp", AdvancedNewFolderMode::NameList, &[]);
        advanced.draft = " 123 / 345 / 678 \n纯名字".to_owned();
        assert_eq!(planned_names(&advanced), vec!["123/345/678", "纯名字"]);
    }

    #[test]
    fn duplicate_nested_leaves_get_last_segment_suffixed() {
        let mut advanced = state("/tmp", AdvancedNewFolderMode::NameList, &[]);
        advanced.draft = "a/b\na/b\na/c".to_owned();
        assert_eq!(planned_names(&advanced), vec!["a/b", "a/b 2", "a/c"]);
    }

    #[test]
    fn nested_first_segment_reuses_disk_directory_instead_of_avoiding_it() {
        // 磁盘已有 123:嵌套行首段应复用而不是整条错开。
        let mut advanced = state("/tmp", AdvancedNewFolderMode::NameList, &["123"]);
        advanced.draft = "123/345".to_owned();
        assert_eq!(planned_names(&advanced), vec!["123/345"]);
    }
}
