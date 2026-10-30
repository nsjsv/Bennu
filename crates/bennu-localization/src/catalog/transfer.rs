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
    ("Sent to %1", "已发送到 %1"),
    ("Send to %1 failed: %2", "发送到 %1 失败：%2"),
    (
        "Sending to %1: %2 (%3/%4, %5 of %6)",
        "正在发送到 %1：%2（第 %3/%4 个，已传 %5/共 %6）",
    ),
    ("Sending to %1...", "正在发送到 %1..."),
];
