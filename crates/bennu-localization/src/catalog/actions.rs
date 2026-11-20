//! 自定义动作：执行命令这一步、动作文件的错误说明、设置页、菜单、通知。

pub(super) const ENTRIES: &[(&str, &str)] = &[
    // 执行命令这一步
    ("Run Command", "执行命令"),
    ("Could not start %1: %2", "没能启动 %1：%2"),
    ("%1 exited with status %2", "%1 退出，状态码 %2"),
    ("%1 was stopped by signal %2", "%1 被信号 %2 停止"),
];
