//! 高级新建文件夹弹窗的模型:两种批量命名模式与最终名字计划。
//! 名字错开复用共享命名规则(`suffixed_name_candidates`),与执行侧
//! AlreadyExists 原子裁决同一套语义。

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::PathBuf;

use iced::widget::text_editor;

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
    /// 创建按钮旁的下拉菜单是否展开(hover/点击切换)。
    pub(crate) after_menu_open: bool,
    /// 菜单代际:每次展开/取消待关闭时自增,使过期的延迟关闭任务失效。
    pub(crate) menu_generation: u64,
    /// 鼠标悬停在分裂按钮上(整个按钮统一提亮)。
    pub(crate) split_button_hover: bool,
    /// 打开弹窗时 directory 已有的条目名,plan() 用它错开磁盘重名。
    taken_names: HashSet<String>,
    /// 嵌套链父目录的内容快照(key = 相对前缀,如 "test" 或 "a/b"),
    /// 由应用层异步探测回填;plan() 用它错开嵌套叶子段的磁盘重名。
    pub(crate) taken_deep: HashMap<String, HashSet<String>>,
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
            menu_generation: 0,
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

    /// 批内与磁盘重名错开后的最终叶子路径。
    pub(crate) fn plan(&self) -> Vec<String> {
        self.plan_detailed()
            .into_iter()
            .map(|(name, _)| name)
            .collect()
    }

    /// (最终叶子路径, 是否被自动错开):预览用它标注哪些名字撞了车。
    /// - 单段行沿用共享命名规则错开磁盘与批内重名(名字、名字 2...);
    /// - 多段行(嵌套链)的中间段允许复用已有目录,只有完整叶子路径在
    ///   批内重复时才错开最后一段。
    /// 执行侧逐段创建,AlreadyExists 裁决兜底磁盘深层重名与极端竞争。
    pub(crate) fn plan_detailed(&self) -> Vec<(String, bool)> {
        let requested = match self.mode {
            AdvancedNewFolderMode::NameList => self.listed_names(),
            AdvancedNewFolderMode::Numbered => self.numbered_names(),
        };
        let mut taken = self.taken_names.clone();
        let mut planned_leaves: Vec<(String, bool)> = Vec::with_capacity(requested.len());
        for name in requested {
            let segments: Vec<&str> = name
                .split('/')
                .map(str::trim)
                .filter(|segment| !segment.is_empty())
                .collect();
            match segments.as_slice() {
                [] => continue,
                [single] => {
                    let name = if taken.contains(*single) {
                        crate::model::suffixed_name_candidates(OsStr::new(single), "", false)
                            .into_iter()
                            .map(|candidate| candidate.to_string_lossy().into_owned())
                            .find(|candidate| !taken.contains(candidate))
                            .unwrap_or_else(|| (*single).to_owned())
                    } else {
                        (*single).to_owned()
                    };
                    let renamed = name != *single;
                    taken.insert(name.clone());
                    planned_leaves.push((name, renamed));
                }
                _ => {
                    // 嵌套链:中间段复用已有目录,叶子段对磁盘(深层探测)
                    // 与批内已占用的名字做候选错开,撞车即标注。
                    let (head, last) = segments.split_at(segments.len() - 1);
                    let prefix = head.join("/");
                    let leaf_last = last[0];
                    let leaf = segments.join("/");
                    let mut occupied: HashSet<String> =
                        self.taken_deep.get(&prefix).cloned().unwrap_or_default();
                    for (seen, _) in &planned_leaves {
                        if let Some((seen_head, seen_last)) = seen.rsplit_once('/') {
                            if seen_head == prefix {
                                occupied.insert(seen_last.to_owned());
                            }
                        } else if prefix.is_empty() {
                            occupied.insert(seen.clone());
                        }
                    }
                    let leaf = if occupied.contains(leaf_last) {
                        crate::model::suffixed_name_candidates(OsStr::new(leaf_last), "", false)
                            .into_iter()
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
                    let renamed = leaf != segments.join("/");
                    planned_leaves.push((leaf, renamed));
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
    /// 箭头点击切换菜单显隐。
    AfterMenuOpenChanged(bool),
    /// 鼠标进入/离开菜单区域:进入取消待关闭,离开安排延迟关闭。
    MenuHoverChanged(bool),
    /// 鼠标进入/离开分裂按钮:进入展开菜单,离开安排延迟关闭。
    SplitButtonHoverChanged(bool),
    /// 延迟关闭检查:代际匹配才真正收起菜单。
    MenuCloseTick(u64),
    /// 带收尾动作的确认(创建后进入 / 新标签进入)。
    ConfirmedWithAfter(AdvancedNewFolderAfter),
    /// 嵌套链父目录的内容探测回执:预览据此提前标注叶子段磁盘重名。
    NestedNamesProbed {
        prefix: String,
        names: HashSet<String>,
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
            menu_generation: 0,
            split_button_hover: false,
            taken_names: taken.iter().map(|name| name.to_string()).collect(),
            taken_deep: HashMap::new(),
        }
    }

    #[test]
    fn listed_names_drop_blank_lines_and_trim() {
        let mut advanced = state("/tmp", AdvancedNewFolderMode::NameList, &[]);
        advanced.draft = "照片\n\n  影像  \r\n".to_owned();
        assert_eq!(advanced.plan(), vec!["照片", "影像"]);
    }

    #[test]
    fn draft_edits_flow_into_plan_source() {
        let mut advanced = state("/tmp", AdvancedNewFolderMode::NameList, &[]);
        advanced.editor_content = text_editor::Content::with_text("照片\n影像");
        advanced.draft = advanced.editor_content.text();
        assert_eq!(advanced.plan(), vec!["照片", "影像"]);
    }

    #[test]
    fn numbered_names_pad_to_two_digits_and_follow_wider_counts() {
        let mut advanced = state("/tmp", AdvancedNewFolderMode::Numbered, &[]);
        advanced.prefix = "项目".to_owned();
        advanced.start_number = "1".to_owned();
        advanced.count = "10".to_owned();
        let planned = advanced.plan();
        assert_eq!(planned.first().map(String::as_str), Some("项目01"));
        assert_eq!(planned.last().map(String::as_str), Some("项目10"));

        advanced.count = "101".to_owned();
        let planned = advanced.plan();
        assert_eq!(planned.first().map(String::as_str), Some("项目001"));
        assert_eq!(planned.len(), 101);
    }

    #[test]
    fn plan_duplicates_within_batch_get_shared_suffix_rule() {
        let mut advanced = state("/tmp", AdvancedNewFolderMode::NameList, &[]);
        advanced.draft = "照片\n照片\n照片".to_owned();
        assert_eq!(advanced.plan(), vec!["照片", "照片 2", "照片 3"]);
    }

    #[test]
    fn plan_avoids_names_taken_on_disk() {
        let mut advanced = state(
            "/tmp",
            AdvancedNewFolderMode::NameList,
            &["新建文件夹", "新建文件夹 2"],
        );
        advanced.draft = "新建文件夹".to_owned();
        assert_eq!(advanced.plan(), vec!["新建文件夹 3"]);
    }

    #[test]
    fn invalid_number_fields_fall_back_to_defaults() {
        let mut advanced = state("/tmp", AdvancedNewFolderMode::Numbered, &[]);
        advanced.start_number = "abc".to_owned();
        advanced.count = "0".to_owned();
        assert_eq!(advanced.plan(), vec!["01"]);
    }

    #[test]
    fn plan_detailed_flags_only_renamed_entries() {
        let mut advanced = state("/tmp", AdvancedNewFolderMode::NameList, &["已占用"]);
        advanced.draft = "已占用\n新名字\na/b\na/b".to_owned();
        let detailed = advanced.plan_detailed();
        assert_eq!(
            detailed,
            vec![
                ("已占用 2".to_owned(), true),
                ("新名字".to_owned(), false),
                ("a/b".to_owned(), false),
                ("a/b 2".to_owned(), true),
            ]
        );
    }

    #[test]
    fn nested_leaf_conflicts_with_deep_disk_entry_get_renamed() {
        let mut advanced = state("/tmp", AdvancedNewFolderMode::NameList, &["test"]);
        advanced
            .taken_deep
            .insert("test".to_owned(), ["test".to_owned()].into_iter().collect());
        advanced.draft = "test/test".to_owned();
        assert_eq!(
            advanced.plan_detailed(),
            vec![("test/test 2".to_owned(), true)]
        );

        // 快照还没探测回来:不乱改,等探测后预览刷新。
        let mut unprobed = state("/tmp", AdvancedNewFolderMode::NameList, &["test"]);
        unprobed.draft = "test/test".to_owned();
        assert_eq!(
            unprobed.plan_detailed(),
            vec![("test/test".to_owned(), false)]
        );
    }

    #[test]
    fn nested_paths_stay_as_slash_separated_leaves() {
        let mut advanced = state("/tmp", AdvancedNewFolderMode::NameList, &[]);
        advanced.draft = " 123 / 345 / 678 \n纯名字".to_owned();
        assert_eq!(advanced.plan(), vec!["123/345/678", "纯名字"]);
    }

    #[test]
    fn duplicate_nested_leaves_get_last_segment_suffixed() {
        let mut advanced = state("/tmp", AdvancedNewFolderMode::NameList, &[]);
        advanced.draft = "a/b\na/b\na/c".to_owned();
        assert_eq!(advanced.plan(), vec!["a/b", "a/b 2", "a/c"]);
    }

    #[test]
    fn nested_first_segment_reuses_disk_directory_instead_of_avoiding_it() {
        // 磁盘已有 123:嵌套行首段应复用而不是整条错开。
        let mut advanced = state("/tmp", AdvancedNewFolderMode::NameList, &["123"]);
        advanced.draft = "123/345".to_owned();
        assert_eq!(advanced.plan(), vec!["123/345"]);
    }
}
