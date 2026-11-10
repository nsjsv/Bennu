//! 本地传输：发送到手机、接收确认、传输窗口与通知。

pub(super) const ENTRIES: &[(&str, &str)] = &[
    ("%1 wants to send %2 item(s)", "%1 想要发送 %2 个文件"),
    ("Incoming files", "收到文件"),
    ("Remember this device", "记住此设备"),
    ("Accept", "接受"),
    ("Decline", "拒绝"),
    ("Files received", "文件接收完成"),
    ("Received %1 file(s).", "已接收 %1 个文件。"),
    (
        "Received %1 file(s), %2 failed.",
        "已接收 %1 个文件，%2 个失败。",
    ),
    ("Choose receive directory", "选择接收目录"),
    (
        "Could not start receiving: port %1 is already in use. Turn off receiving or choose another port.",
        "接收没开起来：端口 %1 被占用。关掉接收，或换一个端口。",
    ),
    (
        "Could not start receiving on port %1: %2",
        "接收没开起来（端口 %1）：%2",
    ),
    ("Receiving stopped (port %1): %2", "接收已停止（端口 %1）：%2"),
    (
        "the selected location is not a local directory",
        "所选位置不是本地目录",
    ),
    (
        "Could not start transfer service: %1",
        "传输服务启动失败：%1",
    ),
    // 扫码发送
    ("Preparing download link...", "正在准备下载链接..."),
    ("Could not start download service", "下载服务启动失败"),
    (
        "Scan the code or open the link on your phone: %1",
        "在手机上扫码或打开链接：%1",
    ),
    ("Choose another address", "重新选择地址"),
    (
        "Choose an address your phone can reach",
        "选择手机可访问的本机地址",
    ),
    ("Transfer timed out", "连接已超时关闭"),
    ("Transfer finished", "传输完成"),
    ("Connected. Sending files...", "已连接，正在发送..."),
    ("Waiting for a phone to scan...", "等待手机扫码..."),
    ("Sent %1", "已发送 %1"),
    ("Sent %1 (%2/s)", "已发送 %1（%2/秒）"),
    // 直接发送到附近设备
    ("No nearby devices found", "未发现附近设备"),
    ("Nearby LocalSend devices", "附近的 LocalSend 设备"),
    ("Send", "发送"),
    (
        "Files inside an archive cannot be sent directly. Extract them first.",
        "压缩包里的文件不能直接发送，先解压出来。",
    ),
    ("LocalSend is not running.", "LocalSend 没有在运行。"),
    ("Other devices can find this computer.", "其他设备可以搜到这台电脑。"),
    (
        "Receiving is off. Other devices cannot find this computer.",
        "接收已关闭，其他设备搜不到这台电脑。",
    ),
    ("This computer: %1", "本机：%1"),
    ("No files to send", "还没有要发送的文件"),
    (
        "Drag files onto LocalSend in the sidebar, or choose them here.",
        "把文件拖到侧栏的 LocalSend 上，或在这里选择。",
    ),
    ("Clear list", "清空"),
    ("%1 item(s) · %2", "%1 项 · %2"),
    ("Choose files...", "选择文件…"),
    ("Choose folders...", "选择文件夹…"),
    ("Unreadable", "读不了"),
    ("Send with LocalSend", "用 LocalSend 发送"),
    ("Choose files to send", "选择要发送的文件"),
    ("Choose folders to send", "选择要发送的文件夹"),
    ("the selected item is not on this computer", "所选项目不在这台电脑上"),
    ("Sent", "已发送"),
    ("Send failed: %1", "发送失败：%1"),
    ("Sending %1/%2 · %3%", "正在发送 %1/%2 · %3%"),
    ("Sending...", "正在发送…"),
];
