//! 侧边栏的 main 层桥接：效果 → Task 翻译（数据加载/设备挂载/网络
//! 挂载）与拖宽指针路由。独立成模块守住 main.rs 的行数预算。
//!
//! 拖宽状态放这一层而不是会话层：增量计算需要指针 X（iced::Point
//! 属运行时类型，spec 禁止进 picker_session）；宽度写入统一走会话的
//! `set_sidebar_width`（共享 clamp）。

use iced::{window, Task};

use crate::picker_session::{load_sidebar_data, PickerSession, SessionMessage};
use crate::{Message, PickerDaemon};

/// 侧栏拖宽起点：指针增量即宽度增量。
#[derive(Debug, Clone, Copy)]
pub(crate) struct SidebarResizeDrag {
    cursor_start_x: f32,
    width_start: f32,
}

/// 侧边栏数据一次性加载：与每窗重解析主题同模式（打开时读一次，
/// 打开期间主程序改收藏/连接不影响已开窗口）。
pub(crate) fn sidebar_data_task(window_id: window::Id) -> Task<Message> {
    Task::perform(load_sidebar_data(), move |data| {
        Message::Session(window_id, SessionMessage::SidebarDataLoaded(Box::new(data)))
    })
}

/// 未挂载设备：挂载回信携带目标路径/错误。
pub(crate) fn mount_device_task(
    window_id: window::Id,
    id: desktop_linux::StorageDeviceId,
) -> Task<Message> {
    Task::perform(
        desktop_linux::mount_storage_device(id.clone()),
        move |outcome| {
            Message::Session(
                window_id,
                SessionMessage::SidebarDeviceMountFinished {
                    id,
                    mount_path: outcome.map_err(|error| error.to_string()),
                },
            )
        },
    )
}

/// 网络连接：先查主程序已存凭证（密钥环），查不到按无 app 凭证
/// 挂载——选择器没有凭据表单，失败只在侧栏提示（app-ui 手动连接
/// 的 lookup 流程裁剪版）。
pub(crate) fn mount_connection_task(
    window_id: window::Id,
    session: &PickerSession,
    id: desktop_linux::NetworkConnectionId,
) -> Task<Message> {
    let Some(connection) = session.sidebar_connection(&id) else {
        return Task::none();
    };
    Task::perform(
        async move {
            let credentials =
                desktop_linux::lookup_network_connection_credentials(connection.clone())
                    .await
                    .ok()
                    .flatten();
            desktop_linux::mount_network_connection_with_credentials(connection, credentials)
                .await
                .map(|mounted| mounted.mount_path)
                .map_err(|error| error.to_string())
        },
        move |mount_path| {
            Message::Session(
                window_id,
                SessionMessage::SidebarConnectionMountFinished { id, mount_path },
            )
        },
    )
}

/// 拖宽按下：读 daemon 的指针簿记定起点（主程序 start_sidebar_resize_
/// drag 读 cursor_position 同构）。
pub(crate) fn start_sidebar_resize(daemon: &mut PickerDaemon, window_id: window::Id) {
    let Some(session) = daemon.windows.get(&window_id) else {
        return;
    };
    let Some(cursor_start_x) = daemon.cursor_position.map(|point| point.x) else {
        return;
    };
    daemon.sidebar_resize.insert(
        window_id,
        SidebarResizeDrag {
            cursor_start_x,
            width_start: session.sidebar_width(),
        },
    );
}

/// 指针移动：拖宽中的窗口把增量写入会话宽度；其余交给预览宿主。
pub(crate) fn handle_pointer_moved(
    daemon: &mut PickerDaemon,
    window_id: window::Id,
    position: iced::Point,
) -> Task<Message> {
    daemon.cursor_position = Some(position);
    if let Some(drag) = daemon.sidebar_resize.get(&window_id) {
        if let Some(session) = daemon.windows.get_mut(&window_id) {
            session.set_sidebar_width(drag.width_start + position.x - drag.cursor_start_x);
        }
        return Task::none();
    }
    daemon.preview.handle_pointer_moved(window_id, position)
}
