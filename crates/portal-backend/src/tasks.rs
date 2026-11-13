//! 会话效果的 Task 构造助手。从 main.rs 拆出以守住其行数上限；
//! 每个函数把一种会话请求翻译成 iced Task（扫描/防抖/建议读取/
//! 程序化滚动），消息统一按窗口路由回 `Message::Session`。

use iced::{window, Task};

use bennu_theme::address_bar::AddressSuggestionRequest;

use super::Message;
use crate::picker_session::suggestions::{
    load_path_suggestions, PATH_SUGGESTION_INPUT_STABILIZATION_DELAY,
};
use crate::picker_session::{PickerSession, SessionMessage, SessionScrollRegion};

/// 会话产出的 Task 按所属窗口路由成全局消息。
pub(crate) fn route_session_tasks(
    window_id: window::Id,
    tasks: impl IntoIterator<Item = Task<SessionMessage>>,
) -> impl Iterator<Item = Task<Message>> {
    tasks
        .into_iter()
        .map(move |task| task.map(move |message| Message::Session(window_id, message)))
}

/// 布局探针 Task：核实各滚动区域的溢出并按窗口路由回信。
pub(crate) fn scrollbar_layout_probe_task(
    window_id: window::Id,
    session: &PickerSession,
) -> Task<Message> {
    Task::batch(route_session_tasks(
        window_id,
        session.scrollbar_layout_probe_tasks(),
    ))
}

/// 结果携带扫描的位置：会话据此路由回根列表或展开节点。
pub(crate) fn scan_location_task(
    window_id: window::Id,
    location: crate::picker_session::PickerLocation,
) -> Task<Message> {
    let scan_future = crate::picker_session::scan_location(location.clone());
    Task::perform(scan_future, move |outcome| {
        Message::Session(
            window_id,
            SessionMessage::ScanReady(Box::new(crate::picker_session::DirectoryScanResult {
                location: location.clone(),
                outcome,
            })),
        )
    })
}

/// 防抖回信：停笔 120ms 后原样带回凭据；会话按凭据决定是否升级为
/// 真正的读取请求（陈旧回信在会话侧拒收）。
pub(crate) fn stabilize_address_input_task(
    window_id: window::Id,
    request: AddressSuggestionRequest,
) -> Task<Message> {
    Task::perform(
        async move {
            tokio::time::sleep(PATH_SUGGESTION_INPUT_STABILIZATION_DELAY).await;
            request
        },
        move |request| {
            Message::Session(
                window_id,
                SessionMessage::AddressSuggestionInputStabilized { request },
            )
        },
    )
}

pub(crate) fn load_path_suggestions_task(
    window_id: window::Id,
    request: AddressSuggestionRequest,
) -> Task<Message> {
    Task::perform(
        load_path_suggestions(request.draft.clone(), request.current_dir.clone()),
        move |suggestions| {
            Message::Session(
                window_id,
                SessionMessage::AddressSuggestionsLoaded {
                    request,
                    suggestions,
                },
            )
        },
    )
}

/// 面包屑滚尾（导航后揭示当前目录段；x 取 MAX 由 scrollable 钳位）。
pub(crate) fn breadcrumb_scroll_to_end_task(session: &PickerSession) -> Task<Message> {
    scroll_region_to_end(session, SessionScrollRegion::Breadcrumb)
}

/// 多栏横向栏容器滚到最右（打开新栏后新栏自动可见，prd）。
pub(crate) fn columns_rail_scroll_to_end_task(session: &PickerSession) -> Task<Message> {
    scroll_region_to_end(session, SessionScrollRegion::ColumnsRail)
}

fn scroll_region_to_end(session: &PickerSession, region: SessionScrollRegion) -> Task<Message> {
    iced::widget::operation::scroll_to(
        crate::picker_session::scrollbar::scroll_id(session.request_path(), region),
        iced::widget::scrollable::AbsoluteOffset {
            x: f32::MAX,
            y: 0.0,
        },
    )
}
