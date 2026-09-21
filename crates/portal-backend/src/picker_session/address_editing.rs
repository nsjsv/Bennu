//! 地址栏编辑会话与渐变过渡：主软件 `app/path_suggestions.rs` 的单窗格
//! 移植。进入/改稿/防抖回信/提交/取消的语义逐条对齐；主软件里的
//! pane 归属、trash 视图与多模态互斥特判在 portal 不存在，其余不变。

use std::path::{Path, PathBuf};
use std::time::Instant;

use bennu_theme::address_bar::{
    AddressBarTransition, AddressEditingSession, AddressEditingSessionId, AddressSuggestionRequest,
};

use super::suggestions::completed_path_text;
use super::{PickerSession, SessionEffect};

/// 补全选择的循环方向（键盘 ↓/Tab → Next，↑/Shift+Tab → Previous）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathSuggestionDirection {
    Next,
    Previous,
}

impl PickerSession {
    /// 进入编辑：建会话（草稿预填当前目录）+ 启动进入过渡（0→1）。
    /// 编辑态重复点击地址栏不重置草稿（焦点交由 main 层处理）。
    pub(crate) fn begin_address_editing(&mut self) -> SessionEffect {
        if self.address_editing.is_some() {
            return SessionEffect::None;
        }
        let session_id = AddressEditingSessionId(self.next_address_editing_session_id);
        self.next_address_editing_session_id = self.next_address_editing_session_id.wrapping_add(1);
        self.address_editing = Some(AddressEditingSession::new(session_id, &self.directory));
        self.address_bar_transition = Some(AddressBarTransition::retarget(
            self.address_bar_transition.as_ref(),
            1.0,
            None,
            Instant::now(),
        ));
        SessionEffect::None
    }

    /// 改稿：清建议与选中、代数 +1、请求防抖回信；空草稿不发起读取
    /// （防抖回信按代数与内容双重匹配，陈旧回信必然被拒）。
    pub(crate) fn update_address_draft(&mut self, value: String) -> SessionEffect {
        let Some(session) = self.address_editing.as_mut() else {
            return SessionEffect::None;
        };
        session.draft = value;
        session.suggestions.clear();
        session.suggestion_selection = None;
        let request = session.next_suggestion_request(&self.directory);
        if request.draft.trim().is_empty() {
            return SessionEffect::None;
        }
        SessionEffect::StabilizeAddressInput { request }
    }

    /// 防抖停笔回信：凭据仍匹配才升级为真正的目录读取请求。
    pub(crate) fn load_stable_address_suggestions(
        &self,
        request: &AddressSuggestionRequest,
    ) -> SessionEffect {
        if !self.address_suggestion_request_matches(request) {
            return SessionEffect::None;
        }
        SessionEffect::LoadPathSuggestions {
            request: request.clone(),
        }
    }

    /// 读取结果回填：凭据匹配（会话/草稿/目录/代数）才接受，否则丢弃。
    pub(crate) fn accept_address_suggestions(
        &mut self,
        request: &AddressSuggestionRequest,
        suggestions: Vec<PathBuf>,
    ) -> SessionEffect {
        if !self.address_suggestion_request_matches(request) {
            return SessionEffect::None;
        }
        let Some(session) = self.address_editing.as_mut() else {
            return SessionEffect::None;
        };
        session.suggestions = suggestions;
        normalize_address_suggestion_selection(session);
        SessionEffect::None
    }

    /// 提交：选中建议优先，否则按草稿解析（trim、空=None、绝对直用、
    /// 相对拼当前目录）；两者皆无 = 静默取消（不导航、不关窗）。
    pub(crate) fn submit_address_editing(&mut self) -> SessionEffect {
        let Some(editing) = self.address_editing.as_ref() else {
            return SessionEffect::None;
        };
        let selected_suggestion = editing
            .suggestion_selection
            .and_then(|index| editing.suggestions.get(index))
            .cloned();
        let parsed_draft = path_from_address_draft(&editing.draft, &self.directory);
        let Some(target) = selected_suggestion.or(parsed_draft) else {
            return self.cancel_address_editing();
        };
        self.finish_address_submission(target)
    }

    /// 补全面板行点击：仅当目标仍在当前建议列表中才提交，防止陈旧
    /// 浮层的点击导航到已不在列表里的路径（主软件 submit_address_
    /// suggestion 同因）。
    pub(crate) fn submit_address_suggestion(&mut self, target: PathBuf) -> SessionEffect {
        let suggestion_is_current = self
            .address_editing
            .as_ref()
            .is_some_and(|editing| editing.suggestions.contains(&target));
        if !suggestion_is_current {
            return SessionEffect::None;
        }
        self.finish_address_submission(target)
    }

    /// 取消：摘除会话并携带草稿快照启动退出过渡（1→0），渐出帧仍能
    /// 渲染草稿内容。
    pub(crate) fn cancel_address_editing(&mut self) -> SessionEffect {
        let Some(editing) = self.address_editing.take() else {
            return SessionEffect::None;
        };
        self.start_address_bar_exit(editing.draft);
        SessionEffect::None
    }

    /// 键盘循环选择建议；无建议/未编辑时 no-op。
    pub(crate) fn move_path_suggestion_selection(
        &mut self,
        direction: PathSuggestionDirection,
    ) -> SessionEffect {
        if let Some(session) = self.address_editing.as_mut() {
            move_address_suggestion_selection(session, direction);
        }
        SessionEffect::None
    }

    /// 键盘补全（Tab/Shift+Tab）：把选中建议补成草稿（补尾分隔符）并
    /// 立即请求下一级建议；输入光标移到末尾由 main 层按消息类型补任务。
    pub(crate) fn complete_path_suggestion(
        &mut self,
        direction: PathSuggestionDirection,
    ) -> SessionEffect {
        let Some(session) = self.address_editing.as_mut() else {
            return SessionEffect::None;
        };
        if session.suggestions.is_empty() {
            return SessionEffect::None;
        }

        if session.suggestion_selection.is_none() {
            session.suggestion_selection = Some(0);
        } else if direction == PathSuggestionDirection::Previous {
            move_address_suggestion_selection(session, direction);
        }

        let Some(path) = session
            .suggestion_selection
            .and_then(|index| session.suggestions.get(index))
            .cloned()
        else {
            return SessionEffect::None;
        };

        session.draft = completed_path_text(&path);
        let request = session.next_suggestion_request(&self.directory);
        SessionEffect::LoadPathSuggestions { request }
    }

    /// 面包屑段点击：当前目录段 = 进入编辑（主软件 activate_breadcrumb_
    /// target 同语义）；其他段 = 先退出编辑再导航。
    pub(crate) fn activate_breadcrumb_target(&mut self, target: PathBuf) -> SessionEffect {
        if target == self.directory {
            return self.begin_address_editing();
        }
        if self.address_editing.is_some() {
            self.cancel_address_editing();
        }
        self.begin_navigation(target)
    }

    /// 帧推进的过渡收尾：退出方向（target≤ε）播完即摘除，快照随过渡
    /// 消失；进入方向停在 1.0 保留，让下次退出从中点连续起步（主软件
    /// advance_address_bar_transition 同构）。
    pub(crate) fn advance_address_bar_transition(&mut self) {
        let expired = self
            .address_bar_transition
            .as_ref()
            .is_some_and(|transition| {
                transition.is_complete() && transition.target_fraction() <= f32::EPSILON
            });
        if expired {
            self.address_bar_transition = None;
        }
    }

    pub(crate) fn address_editing(&self) -> Option<&AddressEditingSession> {
        self.address_editing.as_ref()
    }

    /// 编辑态不透明度：无过渡时按「是否编辑」取 0/1，过渡期间取当帧
    /// 分数（主软件 address_bar_presentation 同构）。
    pub(crate) fn address_transition_fraction(&self) -> f32 {
        self.address_bar_transition
            .as_ref()
            .map(|transition| transition.fraction())
            .unwrap_or(if self.address_editing.is_some() {
                1.0
            } else {
                0.0
            })
    }

    /// 退出过渡期间的草稿快照：会话已摘除，视图只能从过渡里读草稿。
    pub(crate) fn address_exit_snapshot(&self) -> Option<&str> {
        if self.address_editing.is_some() {
            return None;
        }
        self.address_bar_transition
            .as_ref()
            .and_then(|transition| transition.exit_snapshot.as_deref())
    }

    /// 键盘补全路由的挂载条件：编辑中且有建议（主软件
    /// address_suggestion_keyboard_is_active 同名同义）。
    pub(crate) fn address_suggestion_keyboard_is_active(&self) -> bool {
        self.address_editing
            .as_ref()
            .is_some_and(|editing| !editing.suggestions.is_empty())
    }

    pub(crate) fn can_navigate_back(&self) -> bool {
        self.history_position > 0
    }

    pub(crate) fn can_navigate_forward(&self) -> bool {
        self.history_position + 1 < self.history.len()
    }

    fn address_suggestion_request_matches(&self, request: &AddressSuggestionRequest) -> bool {
        self.address_editing
            .as_ref()
            .is_some_and(|editing| editing.matches_suggestion_request(request, &self.directory))
    }

    fn finish_address_submission(&mut self, target: PathBuf) -> SessionEffect {
        let Some(editing) = self.address_editing.take() else {
            return SessionEffect::None;
        };
        // 提交即开始渐出：导航扫描期间地址栏保持草稿外观，避免闪回
        // 面包屑再进编辑的抖动。
        self.start_address_bar_exit(editing.draft);
        self.begin_navigation(target)
    }

    fn start_address_bar_exit(&mut self, snapshot: String) {
        self.address_bar_transition = Some(AddressBarTransition::retarget(
            self.address_bar_transition.as_ref(),
            0.0,
            Some(snapshot),
            Instant::now(),
        ));
    }
}

fn path_from_address_draft(draft: &str, current_dir: &Path) -> Option<PathBuf> {
    let trimmed = draft.trim();
    if trimmed.is_empty() {
        return None;
    }

    let path = PathBuf::from(trimmed);
    if path.is_absolute() {
        Some(path)
    } else {
        Some(current_dir.join(path))
    }
}

fn normalize_address_suggestion_selection(session: &mut AddressEditingSession) {
    if session.suggestions.is_empty() {
        session.suggestion_selection = None;
        return;
    }

    session.suggestion_selection = session
        .suggestion_selection
        .filter(|index| *index < session.suggestions.len());
}

fn move_address_suggestion_selection(
    session: &mut AddressEditingSession,
    direction: PathSuggestionDirection,
) {
    if session.suggestions.is_empty() {
        session.suggestion_selection = None;
        return;
    }

    let last_index = session.suggestions.len() - 1;
    let Some(current_index) = session.suggestion_selection else {
        session.suggestion_selection = Some(match direction {
            PathSuggestionDirection::Next => 0,
            PathSuggestionDirection::Previous => last_index,
        });
        return;
    };

    session.suggestion_selection = Some(match direction {
        PathSuggestionDirection::Next if current_index >= last_index => 0,
        PathSuggestionDirection::Next => current_index + 1,
        PathSuggestionDirection::Previous if current_index == 0 => last_index,
        PathSuggestionDirection::Previous => current_index - 1,
    });
}
