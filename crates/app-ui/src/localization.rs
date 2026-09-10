use std::borrow::Cow;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::config::UiLanguage;

mod batch_rename;
mod convert;
mod document_preview;
mod file_operation_notifications;
mod index_status;
mod properties;
mod search_paths;
mod search_service_recovery;
mod search_service_status;
mod search_workspace;
mod trash;

static CURRENT_LANGUAGE: AtomicU8 = AtomicU8::new(UiLanguage::English.as_u8());

pub(crate) fn set_current_language(language: UiLanguage) {
    CURRENT_LANGUAGE.store(language.as_u8(), Ordering::Relaxed);
}

pub(crate) fn current_language() -> UiLanguage {
    UiLanguage::from_u8(CURRENT_LANGUAGE.load(Ordering::Relaxed))
}

pub(crate) fn current_language_is_chinese() -> bool {
    current_language() == UiLanguage::Chinese
}

pub(crate) fn translate_current(text: &str) -> String {
    translate(current_language(), text).into_owned()
}

pub(crate) fn translate<'a>(language: UiLanguage, text: &'a str) -> Cow<'a, str> {
    if language == UiLanguage::English {
        return Cow::Borrowed(text);
    }

    if let Some(translated) = index_status::translate(text) {
        return Cow::Owned(translated);
    }

    if let Some(translated) = properties::translate(text) {
        return Cow::Owned(translated);
    }

    if let Some(translated) = search_paths::translate(text) {
        return Cow::Owned(translated);
    }

    if let Some(translated) = search_service_recovery::translate(text) {
        return Cow::Owned(translated);
    }

    if let Some(translated) = search_service_status::translate(text) {
        return Cow::Owned(translated);
    }

    if let Some(translated) = file_operation_notifications::translate(text) {
        return Cow::Owned(translated);
    }

    if let Some(translated) = document_preview::translate(text) {
        return Cow::Owned(translated);
    }

    if let Some(translated) = batch_rename::translate(text) {
        return Cow::Owned(translated);
    }
    if let Some(translated) = convert::translate(text) {
        return Cow::Owned(translated);
    }
    if let Some(translated) = search_workspace::translate(text) {
        return Cow::Owned(translated);
    }

    if let Some(translated) = exact_translation(text) {
        return Cow::Borrowed(translated);
    }

    if let Some(translated) = dynamic_translation(text) {
        return Cow::Owned(translated);
    }

    Cow::Borrowed(text)
}

pub(crate) fn trash_refresh_failed(error: &str) -> String {
    trash::refresh_failed(current_language(), error)
}

pub(crate) fn trash_additional_warning_count(count: usize) -> String {
    trash::additional_warning_count(current_language(), count)
}

pub(crate) fn trash_warning_summary(count: usize) -> String {
    trash::warning_summary(current_language(), count)
}

pub(crate) fn trash_tracking_warning(language: UiLanguage, warning: &str) -> String {
    trash::tracking_warning(language, warning)
}

pub(crate) fn current_trash_tracking_warning(warning: &str) -> String {
    trash::tracking_warning(current_language(), warning)
}

pub(crate) fn detect_system_language() -> UiLanguage {
    detect_system_language_with(|key| std::env::var(key).ok())
}

fn detect_system_language_with<F>(lookup: F) -> UiLanguage
where
    F: Fn(&str) -> Option<String>,
{
    for key in ["LC_ALL", "LC_MESSAGES", "LANGUAGE", "LANG"] {
        let Some(value) = lookup(key) else {
            continue;
        };
        if locale_value_is_chinese(&value) {
            return UiLanguage::Chinese;
        }
    }
    UiLanguage::English
}

fn locale_value_is_chinese(value: &str) -> bool {
    value
        .split(':')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            segment
                .split(['.', '@'])
                .next()
                .unwrap_or(segment)
                .to_ascii_lowercase()
        })
        .any(|segment| segment == "zh" || segment.starts_with("zh_"))
}

fn dynamic_translation(text: &str) -> Option<String> {
    if let Some((shown, total)) =
        parse_two_counts(text, "Only showing ", " lines. Full line count: ", ".")
    {
        return Some(format!("当前仅显示 {shown} 行，完整行数为 {total}。"));
    }
    if let Some(count) = parse_prefixed_count(text, "", " tasks") {
        return Some(format!("{count} 个任务"));
    }
    if let Some(count) = parse_prefixed_count(text, "", " items") {
        return Some(format!("{count} 个项目"));
    }
    if let Some(count) = parse_prefixed_count(text, "", " explicit path(s)") {
        return Some(format!("{count} 个显式路径"));
    }
    if let Some(item_count) = text
        .strip_suffix(" will be added to:")
        .and_then(translated_item_count)
    {
        return Some(format!("将向以下位置添加 {item_count}："));
    }
    if let Some(item_count) = text
        .strip_prefix("Delete ")
        .and_then(|body| body.strip_suffix(" from Trash permanently? This cannot be undone."))
        .and_then(translated_item_count)
    {
        return Some(format!(
            "要从回收站永久删除 {item_count} 吗？此操作无法撤销。"
        ));
    }
    if let Some(item_count) = text
        .strip_prefix("Delete ")
        .and_then(|body| body.strip_suffix(" permanently? This cannot be undone."))
        .and_then(translated_item_count)
    {
        return Some(format!("要永久删除 {item_count} 吗？此操作无法撤销。"));
    }
    if let Some(size) = text.strip_prefix("Maximum content extraction: ") {
        return Some(format!("最大内容提取大小：{size}"));
    }
    if let Some(error) = text.strip_prefix("Service degraded: ") {
        return Some(format!("服务已降级：{error}"));
    }
    if let Some(error) = text.strip_prefix("Invalid regular expression: ") {
        return Some(format!("无效的正则表达式：{error}"));
    }
    if let Some(error) = text.strip_prefix("Service failed: ") {
        return Some(format!("服务失败：{error}"));
    }
    if let Some(size) = text.strip_prefix("Duration unknown · ") {
        return Some(format!("时长未知 · {size}"));
    }
    if let Some(position) = text.strip_prefix("Playing · ") {
        return Some(format!("播放中 · {position}"));
    }
    if let Some(position) = text.strip_prefix("Paused · ") {
        return Some(format!("已暂停 · {position}"));
    }
    if let Some(percent) = text
        .strip_prefix("Volume ")
        .and_then(|body| body.strip_suffix('%'))
    {
        return Some(format!("音量 {percent}%"));
    }
    if let Some(error) = text.strip_prefix("Unavailable: ") {
        return Some(format!("不可用：{error}"));
    }
    if let Some(error) = text.strip_prefix("Could not load: ") {
        return Some(format!("无法加载：{error}"));
    }
    if let Some(error) = text.strip_prefix("Could not load more text preview: ") {
        return Some(format!("无法继续加载文本预览：{error}"));
    }
    if let Some(error) = text.strip_prefix("Default open failed: ") {
        return Some(format!("默认打开失败：{error}"));
    }
    if let Some(error) = text.strip_prefix("Could not update permissions: ") {
        return Some(format!("无法更新权限：{error}"));
    }
    if let Some(error) = text.strip_prefix("Audio unavailable: ") {
        return Some(format!("音频不可用：{error}"));
    }
    if let Some(error) = text.strip_prefix("Could not load saved network password: ") {
        return Some(format!("无法加载已保存的网络密码：{error}"));
    }
    if let Some(error) = text.strip_prefix("Could not remove saved network password: ") {
        return Some(format!("无法移除已保存的网络密码：{error}"));
    }
    if let Some(error) = text.strip_prefix("Could not connect network location: ") {
        return Some(format!("无法连接网络位置：{error}"));
    }
    if let Some(error) = text.strip_prefix("Could not disconnect network location: ") {
        return Some(format!("无法断开网络位置：{error}"));
    }
    if let Some(error) = text.strip_prefix("File operation queue storage failed: ") {
        return Some(format!("文件操作队列存储失败：{error}"));
    }
    if let Some(error) = text.strip_prefix("Failed to initialize file operation queue storage: ") {
        return Some(format!("初始化文件操作队列存储失败：{error}"));
    }
    for (prefix, translated_prefix) in [
        ("Failed to save user preferences: ", "保存用户偏好失败："),
        (
            "Failed to save application configuration: ",
            "保存应用配置失败：",
        ),
        ("Failed to save column width: ", "保存列宽失败："),
        ("Failed to save browser session: ", "保存浏览会话失败："),
    ] {
        if let Some(error) = text.strip_prefix(prefix) {
            return Some(format!("{translated_prefix}{error}"));
        }
    }
    if let Some(path) = text
        .strip_prefix("Could not open startup directory ")
        .and_then(|body| body.strip_suffix("; opening the home directory."))
    {
        return Some(format!("无法打开启动目录 {path}；将打开主目录。"));
    }
    if let Some(error) = text
        .strip_prefix("Failed to restore saved view state: ")
        .and_then(|body| body.strip_suffix("; opening the home directory."))
    {
        return Some(format!("恢复已保存的视图状态失败：{error}；将打开主目录。"));
    }
    if let Some(content) = text
        .strip_prefix("File is too large to preview (")
        .and_then(|body| body.strip_suffix('.'))
    {
        let (size, maximum) = content.split_once("). Maximum preview size is ")?;
        return Some(format!(
            "文件过大，无法预览（{size}）。最大预览大小为 {maximum}。"
        ));
    }
    if let Some(path) = text.strip_prefix("Original: ") {
        return Some(format!("原始位置：{path}"));
    }
    if let Some(path) = text.strip_prefix("Directory: ") {
        return Some(format!("目录：{path}"));
    }

    None
}

fn parse_prefixed_count(text: &str, prefix: &str, suffix: &str) -> Option<usize> {
    text.strip_prefix(prefix)?
        .strip_suffix(suffix)?
        .parse()
        .ok()
}

fn parse_two_counts(
    text: &str,
    prefix: &str,
    middle: &str,
    suffix: &str,
) -> Option<(usize, usize)> {
    let content = text.strip_prefix(prefix)?.strip_suffix(suffix)?;
    let (left, right) = content.split_once(middle)?;
    Some((left.parse().ok()?, right.parse().ok()?))
}

fn translated_item_count(label: &str) -> Option<String> {
    if label == "1 item" {
        return Some("1 个项目".to_owned());
    }
    let count = label.strip_suffix(" items")?.parse::<usize>().ok()?;
    Some(format!("{count} 个项目"))
}

fn exact_translation(text: &str) -> Option<&'static str> {
    match text {
        "Settings - Bennu" => Some("设置 - Bennu"),
        "Properties - Bennu" => Some("属性 - Bennu"),
        "Preview - Bennu" => Some("预览 - Bennu"),
        "Search - Bennu" => Some("搜索 - Bennu"),
        "Closing window..." => Some("正在关闭窗口..."),
        "Settings" => Some("设置"),
        "General" => Some("通用"),
        "About" => Some("关于"),
        "Version" => Some("版本"),
        "License" => Some("许可证"),
        "GitHub repository" => Some("GitHub 仓库"),
        "Report issues or browse the source code in your browser." => {
            Some("在浏览器中查看源代码或提交问题。")
        }
        "Bennu is free software released under the GNU General Public License, version 3 or later." => {
            Some("Bennu 是自由软件，基于 GNU 通用公共许可证第三版或更高版本授权。")
        }
        "Appearance" => Some("外观"),
        "Theme" => Some("主题"),
        "Mode" => Some("模式"),
        "Color scheme" => Some("配色预设"),
        "Custom color scheme" => Some("自定义配色"),
        "Import custom color scheme" => Some("导入自定义配色"),
        "Import JSON" => Some("导入 JSON"),
        "Import" => Some("导入"),
        "Import failed" => Some("导入失败"),
        "Custom colors have low text contrast" => Some("自定义颜色的文字对比度较低"),
        "Background and text contrast is below 2.4:1" => {
            Some("背景与文字的对比度低于 2.4:1")
        }
        "Surface and muted text contrast is below 2.4:1" => {
            Some("表面与弱化文字的对比度低于 2.4:1")
        }
        "Light" => Some("浅色"),
        "Dark" => Some("深色"),
        "Default" => Some("默认"),
        "Hard" => Some("硬"),
        "Medium" => Some("中等"),
        "Soft" => Some("柔和"),
        "High contrast" => Some("高对比度"),
        "Colorblind" => Some("色觉辅助"),
        "Tritanopia" => Some("蓝黄色觉辅助"),
        "Dimmed" => Some("柔暗"),
        "Files" => Some("文件"),
        "Startup location" => Some("启动位置"),
        "Launch behavior" => Some("启动行为"),
        "Columns view" => Some("多栏视图"),
        "Visible columns" => Some("可见栏数"),
        "3 columns" => Some("3 栏"),
        "4 columns" => Some("4 栏"),
        "5 columns" => Some("5 栏"),
        "Merge into existing window" => Some("合并到已有窗口"),
        "Open a new window" => Some("打开新窗口"),
        "Logs" => Some("日志"),
        "Display level" => Some("显示级别"),
        "Loading logs..." => Some("正在加载日志..."),
        "No logs for this level." => Some("当前级别没有日志。"),
        "Error" => Some("错误"),
        "Warning" => Some("警告"),
        "Show warning details" => Some("显示警告详情"),
        "Hide warning details" => Some("隐藏警告详情"),
        "Info" => Some("信息"),
        "Debug" => Some("调试"),
        "App" => Some("应用"),
        "File display" => Some("文件显示"),
        "Window controls" => Some("窗口控制"),
        "Window layout" => Some("窗口布局"),
        "Integrated navigation" => Some("融合导航栏"),
        "Separate title bar" => Some("独立标题栏"),
        "Left side" => Some("左侧"),
        "Right side" => Some("右侧"),
        "Minimize" => Some("最小化"),
        "Maximize" => Some("最大化"),
        "Maximize / Restore" => Some("最大化 / 还原"),
        "Close" => Some("关闭"),
        "Always visible" => Some("始终显示"),
        "Left" => Some("左侧"),
        "Right" => Some("右侧"),
        "Drag to reorder" => Some("拖动排序"),
        "Restore defaults" => Some("恢复默认"),
        "Startup" => Some("启动"),
        "Terminal" => Some("终端"),
        "File Operations" => Some("文件操作"),
        "Verification" => Some("校验"),
        "Rendering" => Some("渲染"),
        "Discrete GPU" => Some("独立显卡"),
        "Restart Required" => Some("需要重启"),
        "Rendering GPU preference changes require restarting Bennu." => {
            Some("更改渲染显卡偏好后需要重启文件管理器。")
        }
        "Restart" => Some("重启"),
        "OK" => Some("确定"),
        "Shortcuts" => Some("快捷键"),
        "Show Hidden Files" => Some("显示隐藏文件"),
        "Show Recursive Folder Size In List View" => Some("在列表视图中显示文件夹递归大小"),
        "Home directory" => Some("主目录"),
        "Open your home directory on startup." => Some("启动时打开主目录。"),
        "Custom directory" => Some("自定义目录"),
        "Open the configured directory on startup." => Some("启动时打开配置的目录。"),
        "Previous state" => Some("上次状态"),
        "Start in the state from the last close, preserving views and directories." => {
            Some("从上次关闭时的状态启动，并保留视图和目录。")
        }
        "Directory" => Some("目录"),
        "Save" => Some("保存"),
        "Custom Startup Directory" => Some("自定义启动目录"),
        "Language" => Some("语言"),
        "Follow the system language until you choose a manual override." => {
            Some("默认跟随系统语言，直到你手动覆盖。")
        }
        "Auto" => Some("自动"),
        "Use the detected system language." => Some("使用检测到的系统语言。"),
        "English" => Some("English"),
        "Always show the interface in English." => Some("始终以英文显示界面。"),
        "中文" => Some("中文"),
        "Always show the interface in Chinese." => Some("始终以中文显示界面。"),
        "Archive name" => Some("归档名称"),
        "Optional password" => Some("可选密码"),
        "Enter a directory path." => Some("请输入目录路径。"),
        "Choose an existing directory." => Some("请选择一个已存在的目录。"),
        "Enter a whole number of MiB (0 = unlimited)." => Some("请输入 MiB 整数（0 表示不限制）。"),
        "Enter a whole number from 0 to 3." => Some("请输入 0 到 3 的整数。"),
        "Enter a smaller preview size." => Some("请输入更小的预览大小。"),
        "Enter an archive name." => Some("请输入归档名称。"),
        "Archive name cannot contain path separators." => Some("归档名称不能包含路径分隔符。"),
        "That archive already exists. Choose another name." => {
            Some("该归档已存在，请使用其他名称。")
        }
        "Enter the archive password." => Some("请输入归档密码。"),
        "Incorrect password. Try again." => Some("密码不正确，请重试。"),
        "Running" => Some("运行中"),
        "Saving" => Some("正在持久化"),
        "Preparing" => Some("准备中"),
        "Image" => Some("图像"),
        "Audio" => Some("音频"),
        "Video" => Some("视频"),
        "Audio preview" => Some("音频预览"),
        "Ready to play" => Some("可以播放"),
        "Opening audio output..." => Some("正在打开音频输出..."),
        "Stopped" => Some("已停止"),
        "Finished" => Some("播放完毕"),
        "Could not start audio preview" => Some("无法启动音频预览"),
        "Tasks" => Some("任务"),
        "Task" => Some("任务"),
        "No tasks" => Some("没有任务"),
        "Items" => Some("项目数"),
        "Status" => Some("状态"),
        "Progress" => Some("进度"),
        "Indeterminate" => Some("不确定"),
        "Processing..." => Some("处理中..."),
        "Copy Details" => Some("复制详情"),
        "Clear" => Some("清除"),
        "Clear Finished" => Some("清除已结束"),
        "Pending" => Some("等待中"),
        "Paused" => Some("已暂停"),
        "Canceling" => Some("取消中"),
        "Failed" => Some("失败"),
        "Completed" => Some("已完成"),
        "Canceled" => Some("已取消"),
        "See the task queue for details." => Some("请在任务队列中查看详情。"),
        "Resume" => Some("继续"),
        "Pause" => Some("暂停"),
        "Cancel" => Some("取消"),
        "Retry" => Some("重新计算"),
        "Rename" => Some("重命名"),
        "Batch Rename" => Some("批量重命名"),
        "Simple" => Some("简单"),
        "Advanced" => Some("高级"),
        "Rename using a template" => Some("使用模板重命名"),
        "Find and replace text" => Some("查找并替换文本"),
        "Original name" => Some("原文件名称"),
        "Name without extension" => Some("原文件名去扩展名"),
        "Number 1,2,3" => Some("编号 1,2,3"),
        "Number 01,02,03" => Some("编号 01,02,03"),
        "Number 001,002,003" => Some("编号 001,002,003"),
        "Random characters" => Some("随机字符"),
        "New Folder" => Some("新建文件夹"),
        "New File" => Some("新建文件"),
        "Move to Trash" => Some("移到回收站"),
        "Restore" => Some("恢复"),
        "Delete Permanently" => Some("永久删除"),
        "Empty Trash" => Some("清空回收站"),
        "Moved to Trash, but undo information could not be recorded." => {
            Some("项目已移到回收站，但无法记录精确的撤销信息。")
        }
        "Copy" => Some("复制"),
        "Move" => Some("移动"),
        "Create Archive" => Some("创建归档"),
        "Extract Archive" => Some("解压归档"),
        "Trash" => Some("回收站"),
        "Path" => Some("路径"),
        "File name" => Some("文件名"),
        "New name" => Some("新名称"),
        "Apply this action to all files and folders" => Some("将此操作应用到所有文件和文件夹"),
        "Keep Both" => Some("保留两者"),
        "Merge" => Some("合并"),
        "Delete Permanently?" => Some("要永久删除吗？"),
        "Empty Trash?" => Some("要清空回收站吗？"),
        "Drop Files" => Some("拖放文件"),
        "Move files into the current folder." => Some("将文件移动到当前文件夹。"),
        "Copy files into the current folder." => Some("将文件复制到当前文件夹。"),
        "Open With" => Some("打开方式"),
        "Loading applications..." => Some("正在加载应用程序..."),
        "Set as default application" => Some("设为默认应用程序"),
        "Required" => Some("必需"),
        "Hide" => Some("隐藏"),
        "Show" => Some("显示"),
        "Open" => Some("打开"),
        "Context menus" => Some("右键菜单"),
        "File entry menu" => Some("文件条目菜单"),
        "Blank area menu" => Some("空白处菜单"),
        "Trash menu" => Some("回收站菜单"),
        "Search results menu" => Some("搜索结果菜单"),
        "Search type filters" => Some("搜索类型筛选"),
        "List columns menu" => Some("列表列菜单"),
        "Favorites menu" => Some("收藏夹菜单"),
        "Devices menu" => Some("设备菜单"),
        "Network menu" => Some("网络菜单"),
        "Restore defaults for this menu" => Some("恢复此菜单的默认设置"),
        "Reset this menu to its defaults?" => Some("要恢复此菜单的默认设置吗？"),
        "Convert Format..." => Some("转换格式..."),
        "Open Containing Folder" => Some("打开所在文件夹"),
        "Spreadsheets" => Some("表格"),
        "Archives" => Some("归档"),
        "Links" => Some("链接"),
        "Images" => Some("图片"),
        "Folders" => Some("文件夹"),
        "PDF" => Some("PDF"),
        "Open with" => Some("打开方式"),
        "Paste" => Some("粘贴"),
        "Create Archive..." => Some("创建归档..."),
        "Batch Rename..." => Some("批量重命名..."),
        "Delete" => Some("删除"),
        "Delete all items in Trash permanently? This cannot be undone." => {
            Some("要永久删除回收站中的所有项目吗？此操作无法撤销。")
        }
        "1 item" => Some("1 个项目"),
        "New..." => Some("新建..."),
        "Open Terminal Here" => Some("在此处打开终端"),
        "Duplicate" => Some("复制副本"),
        "New Folder with Selection" => Some("用选中项新建文件夹"),
        "Copy Path" => Some("复制路径"),
        "Create Symbolic Link" => Some("创建符号链接"),
        "Remove from Favorites" => Some("从收藏夹移除"),
        "No device actions available" => Some("没有可用的设备操作"),
        "No pending conflicts" => Some("没有待处理的冲突"),
        "Add Network Connection" => Some("添加网络连接"),
        "Edit Network Connection" => Some("编辑网络连接"),
        "Connect Network Location" => Some("连接网络位置"),
        "Name" => Some("名称"),
        "Modified Time" => Some("修改时间"),
        "Kind" => Some("类型"),
        "Accessed Time" => Some("访问时间"),
        "Created Time" => Some("创建时间"),
        "URI" => Some("URI"),
        "Username" => Some("用户名"),
        "Password" => Some("密码"),
        "Connect on startup" => Some("启动时连接"),
        "Connect" => Some("连接"),
        "Save & Connect" => Some("保存并连接"),
        "Username is required when a password is provided" => Some("提供密码时必须填写用户名"),
        "WebDAV" => Some("WebDAV"),
        "SFTP" => Some("SFTP"),
        "Edit" => Some("编辑"),
        "Remove" => Some("移除"),
        "Disconnect" => Some("断开连接"),
        "Mount" => Some("挂载"),
        "Unmount" => Some("卸载"),
        "Safely Remove" => Some("安全移除"),
        "Eject" => Some("弹出"),
        "Common locations" => Some("常用位置"),
        "Home" => Some("主目录"),
        "Desktop" => Some("桌面"),
        "Documents" => Some("文档"),
        "Downloads" => Some("下载"),
        "Pictures" => Some("图片"),
        "Music" => Some("音乐"),
        "Videos" => Some("视频"),
        "Desktop, documents, downloads, media, and user config." => {
            Some("桌面、文档、下载、媒体目录和用户配置目录。")
        }
        "Custom selection" => Some("自定义选择"),
        "Choose folders or files from Home." => Some("从主目录中选择文件夹或文件。"),
        "No common locations found" => Some("未找到常用位置"),
        "Loading..." => Some("加载中..."),
        "Show hidden content" => Some("显示隐藏内容"),
        "Refresh" => Some("刷新"),
        "Skip" => Some("跳过"),
        "Add" => Some("添加"),
        "Modify" => Some("修改"),
        "State" => Some("状态"),
        "Records" => Some("记录数"),
        "Size" => Some("大小"),
        "Last update" => Some("最后更新"),
        "Failures" => Some("失败记录"),
        "Reason" => Some("原因"),
        "Update" => Some("更新"),
        "Out of range" => Some("超出范围"),
        "Preview" => Some("预览"),
        "No items" => Some("没有项目"),
        "Trash is empty" => Some("回收站为空"),
        "Select a file and press Space to load preview" => Some("选择一个文件并按空格键加载预览"),
        "Loading preview..." => Some("正在加载预览..."),
        "Preparing download..." => Some("准备下载中..."),
        "Empty directory" => Some("空目录"),
        "Empty archive" => Some("空归档"),
        "(empty file)" => Some("（空文件）"),
        "Text preview is not ready" => Some("文本预览尚未就绪"),
        "Rendered" => Some("渲染"),
        "Raw" => Some("原文"),
        "Select an item to preview" => Some("请选择一个项目进行预览"),
        "Properties" => Some("属性"),
        "No properties are available." => Some("没有可用属性。"),
        "Loading properties..." => Some("正在加载属性..."),
        "Could not load properties" => Some("无法加载属性"),
        "File Information" => Some("文件信息"),
        "Permissions" => Some("权限"),
        "Type" => Some("类型"),
        "Contents" => Some("内容"),
        "Location" => Some("位置"),
        "Created" => Some("创建时间"),
        "Modified" => Some("修改时间"),
        "Accessed" => Some("访问时间"),
        "Size on disk" => Some("磁盘占用"),
        "Calculating..." => Some("计算中..."),
        "Sharing & Permissions" => Some("共享与权限"),
        "Choose who can read, write, or execute this item. Changes save immediately." => {
            Some("选择谁可以读取、写入或执行此项目。更改会立即保存。")
        }
        "Current mode" => Some("当前模式"),
        "Privilege" => Some("权限级别"),
        "Owner" => Some("所有者"),
        "Group" => Some("群组"),
        "Everyone" => Some("所有人"),
        "Primary user" => Some("主要用户"),
        "Assigned group" => Some("所属群组"),
        "Other users" => Some("其他用户"),
        "Read, Write & Execute" => Some("读取、写入和执行"),
        "Read & Write" => Some("读取和写入"),
        "Read & Execute" => Some("读取和执行"),
        "Read only" => Some("只读"),
        "Write & Execute" => Some("写入和执行"),
        "Write only" => Some("只写"),
        "Execute only" => Some("仅执行"),
        "No access" => Some("无权限"),
        "Read" => Some("读取"),
        "Write" => Some("写入"),
        "Execute" => Some("执行"),
        "Apply to Enclosed Items" => Some("应用到包含的项目"),
        "Permissions are read-only" => Some("权限为只读"),
        "Permission editing is unavailable for this item." => Some("此项目不支持编辑权限。"),
        "Saving permissions..." => Some("正在保存权限..."),
        "Applying permissions to enclosed items..." => Some("正在将权限应用到包含的项目..."),
        "Unavailable" => Some("不可用"),
        "Folder" => Some("文件夹"),
        "File" => Some("文件"),
        "Symbolic Link" => Some("符号链接"),
        "Broken Link" => Some("损坏的链接"),
        "Link" => Some("链接"),
        "Other" => Some("其他"),
        "Sort" => Some("排序"),
        "Extension" => Some("扩展名"),
        "Ext" => Some("扩展名"),
        "Case" => Some("大小写"),
        "Sequence" => Some("序号"),
        "Seq" => Some("序号"),
        "Replace" => Some("替换"),
        "Insert" => Some("插入"),
        "Slice" => Some("切片"),
        "Random" => Some("随机"),
        "List" => Some("列表"),
        "Custom" => Some("自定义"),
        "Regex" => Some("正则"),
        "Batch" => Some("批处理"),
        "Selection order" => Some("选择顺序"),
        "All" => Some("全部"),
        "New extension" => Some("新扩展名"),
        "Natural" => Some("自然排序"),
        "Name A-Z" => Some("名称 A-Z"),
        "Name Z-A" => Some("名称 Z-A"),
        "Modified old-new" => Some("修改时间 旧到新"),
        "Modified new-old" => Some("修改时间 新到旧"),
        "Extension A-Z" => Some("扩展名 A-Z"),
        "Extension Z-A" => Some("扩展名 Z-A"),
        "Reverse" => Some("反转"),
        "Preserve" => Some("保留"),
        "lowercase" => Some("小写"),
        "UPPERCASE" => Some("大写"),
        "First" => Some("首个"),
        "Last" => Some("最后一个"),
        "Range" => Some("范围"),
        "Before" => Some("之前"),
        "After" => Some("之后"),
        "Position" => Some("位置"),
        "After text" => Some("在文本之后"),
        "Off" => Some("关闭"),
        "Replace stem" => Some("替换文件名主体"),
        "Prefix" => Some("前缀"),
        "Suffix" => Some("后缀"),
        "Text / Range" => Some("文本 / 范围"),
        "Character classes" => Some("字符类别"),
        "Lowercase" => Some("小写字母"),
        "Uppercase" => Some("大写字母"),
        "Digits" => Some("数字"),
        "Symbols" => Some("符号"),
        "Brackets" => Some("括号"),
        "Whitespace" => Some("空白字符"),
        "Hanzi" => Some("汉字"),
        "Keep case" => Some("保留大小写"),
        "Title Case" => Some("标题式大小写"),
        "Invert Case" => Some("反转大小写"),
        "Ready" => Some("就绪"),
        "Unchanged" => Some("未更改"),
        "Empty name" => Some("名称为空"),
        "Duplicate target" => Some("目标重复"),
        "Already exists" => Some("已存在"),
        "Rule error" => Some("规则错误"),
        "Original stem" => Some("原始名称主体"),
        "Original" => Some("原始位置"),
        "Ignore case" => Some("忽略大小写"),
        "Ignore extension" => Some("忽略扩展名"),
        "Start" => Some("起始"),
        "Step" => Some("步长"),
        "Padding" => Some("补位"),
        "Find" => Some("查找"),
        "With" => Some("替换为"),
        "Range start" => Some("范围起点"),
        "Range length" => Some("范围长度"),
        "Text" => Some("文本"),
        "Length" => Some("长度"),
        "Alphabet" => Some("字符集"),
        "Names" => Some("名称列表"),
        "Template" => Some("模板"),
        "Pattern" => Some("模式"),
        "Replacement" => Some("替换文本"),
        "Commands" => Some("命令"),
        "Apply" => Some("应用"),
        "Click a shortcut, then press the replacement keys." => {
            Some("点击一个快捷键项，然后按下新的按键组合。")
        }
        "Press keys..." => Some("请按键..."),
        "Reset" => Some("重置"),
        "Unsupported shortcut. Use a letter, number, function key, arrow, or named edit key." => {
            Some("不支持的快捷键。请使用字母、数字、功能键、方向键或命名编辑键。")
        }
        "Shortcut" => Some("快捷键"),
        "Places" => Some("位置"),
        "Favorites" => Some("收藏夹"),
        "Devices" => Some("设备"),
        "Loading devices..." => Some("正在加载设备..."),
        "UDisks devices unavailable" => Some("UDisks 设备不可用"),
        "GVfs devices unavailable" => Some("GVfs 设备不可用"),
        "Some device providers unavailable" => Some("部分设备提供方不可用"),
        "No saved connections" => Some("没有已保存的连接"),
        "Network status unavailable" => Some("网络状态不可用"),
        "Network" => Some("网络"),
        "Working..." => Some("处理中..."),
        "Not connected" => Some("未连接"),
        "Connecting..." => Some("连接中..."),
        "Connection error" => Some("连接错误"),
        "Mounted" => Some("已挂载"),
        "Not mounted" => Some("未挂载"),
        "Next" => Some("下一步"),
        "Format" => Some("格式"),
        "Compression" => Some("压缩"),
        "No compression" => Some("不压缩"),
        "Fast" => Some("快速"),
        "Balanced" => Some("均衡"),
        "Maximum" => Some("最高"),
        "tar.gz archives do not support passwords." => Some("tar.gz 归档不支持密码。"),
        "Create" => Some("创建"),
        "No selected items" => Some("没有已选择的项目"),
        "Checking" => Some("检查"),
        "Checking..." => Some("正在检查..."),
        "Extract" => Some("解压"),
        "Checking password..." => Some("正在检查密码..."),
        "Enter the archive password to continue." => Some("请输入归档密码以继续。"),
        "Checking archive..." => Some("正在检查归档..."),
        "Thumbnails" => Some("缩略图"),
        "File preview" => Some("文件预览"),
        "Preview settings" => Some("预览设置"),
        "Collapse" => Some("收起"),
        "File Checksum..." => Some("计算文件校验..."),
        "File Checksum" => Some("文件校验"),
        "Computing..." => Some("正在计算..."),
        "Checksum computation canceled" => Some("校验计算已取消"),
        "Verify" => Some("校验比对"),
        "Paste expected checksum" => Some("粘贴期望的校验值"),
        "Enter a valid hex checksum." => Some("请输入合法的十六进制校验值。"),
        "Waiting for the computation to finish..." => Some("等待计算完成..."),
        "No checksum matches the expected value." => Some("没有校验值与期望值匹配。"),
        "Checksum matches" => Some("校验值匹配"),
        "Load" => Some("加载"),
        "Load checksum file..." => Some("加载校验文件..."),
        "Checksum file value matches." => Some("校验文件单值与文件一致。"),
        "Checksum file value does not match." => Some("校验文件单值与文件不一致。"),
        "File entry in the checksum file matches." => Some("与校验文件条目一致。"),
        "File entry in the checksum file does not match." => Some("与校验文件条目不一致。"),
        "The checksum file has no entry for this file." => Some("校验文件中没有该文件的条目。"),
        "Failed to compute checksum" => Some("计算校验值失败"),
        "Click Again to Reset" => Some("再次点击重置"),
        "Remote List Thumbnails" => Some("远程位置列表缩略图"),
        "When enabled, images and videos on remote locations may use more data." => {
            Some("启用后，远程位置中的图片和视频可能会使用更多流量。")
        }
        "Max Text Preview" => Some("最大文本预览"),
        "Text Preview Extensions" => Some("文本预览后缀"),
        "Image Preview Extensions" => Some("图片预览后缀"),
        "Video Preview Extensions" => Some("视频预览后缀"),
        "Audio Preview Extensions" => Some("音频预览后缀"),
        "Archive Preview Extensions" => Some("压缩包预览后缀"),
        "Document Preview Extensions" => Some("文档预览后缀"),
        "SQLite Preview Extensions" => Some("SQLite 预览后缀"),
        "Enter an extension like txt (a leading dot is optional)." => {
            Some("请输入类似 txt 的后缀（前导点可省略）。")
        }
        "No preview available for this file type." => Some("无法预览此文件类型。"),
        "Max Image Preview" => Some("最大图片预览"),
        "Max Video Preview" => Some("最大视频预览"),
        "Max Audio Preview" => Some("最大音频预览"),
        "Max Archive Preview" => Some("最大压缩包预览"),
        "Max Document Preview" => Some("最大文档预览"),
        "Preview Expand Levels" => Some("预览展开层数"),
        "levels" => Some("层"),
        "Size is unlimited: previewing very large files may use a lot of memory." => {
            Some("大小不限制：预览超大文件可能占用大量内存。")
        }
        "MiB" => Some("MiB"),
        "(default)" => Some("（默认）"),
        "File is too large to preview" => Some("文件过大，无法预览"),
        "Preview is only available for UTF-8 text files" => {
            Some("仅支持预览 UTF-8 文本文件")
        }
        "Preview is only available for directories, archives, audio files, images, and UTF-8 text files" => {
            Some("仅支持预览目录、归档、音频、图像和 UTF-8 文本文件")
        }
        "Archive preview supports .zip, .tar, .tar.gz, .tgz, .7z, and .rar files" => {
            Some("归档预览支持 .zip、.tar、.tar.gz、.tgz、.7z 和 .rar 文件")
        }
        "Image preview has invalid dimensions" => Some("图像预览尺寸无效"),
        "Animated image preview has invalid dimensions" => Some("动态图像预览尺寸无效"),
        "Animated image preview has no frames" => Some("动态图像预览没有可用帧"),
        "This GIF has too many frames to inspect safely" => Some("此 GIF 帧数过多，无法安全解析"),
        "Selected item is no longer available" => Some("所选项目已不可用"),
        "Task was interrupted and cannot safely resume" => Some("任务已中断，无法安全恢复"),
        "Finish the current file operation prompt before dropping files" => {
            Some("请先完成当前文件操作提示，再拖放文件")
        }
        "Delete local and remote items separately so local files can use Trash" => {
            Some("请分别删除本地和远程项目，以便本地文件可以移入回收站")
        }
        "Saved view state could not be restored; opening the home directory." => {
            Some("无法恢复已保存的视图状态；将打开主目录。")
        }
        "Strong Verification" => Some("强校验"),
        "Compare copied file content hashes after standard metadata checks." => {
            Some("在标准元数据检查后，对复制文件的内容哈希再做比较。")
        }
        "Focus Path" => Some("聚焦路径栏"),
        "Back" => Some("后退"),
        "Forward" => Some("前进"),
        "Up Folder" => Some("上级文件夹"),
        "Select Up" => Some("选择上一个"),
        "Select Down" => Some("选择下一个"),
        "Select Parent Column" => Some("选择父列"),
        "Select Child Column" => Some("选择子列"),
        "Dismiss" => Some("关闭当前层"),
        "Select All" => Some("全选"),
        "Copy Named Key" => Some("复制命名键"),
        "Paste Named Key" => Some("粘贴命名键"),
        "Cut" => Some("剪切"),
        "Cut Named Key" => Some("剪切命名键"),
        "Undo File Operation" => Some("撤销文件操作"),
        "Redo File Operation" => Some("重做文件操作"),
        "Automatic" => Some("自动"),
        "Ghostty" => Some("Ghostty"),
        "GNOME Console" => Some("GNOME Console"),
        "GNOME Terminal" => Some("GNOME 终端"),
        "Konsole" => Some("Konsole"),
        "Xfce Terminal" => Some("Xfce 终端"),
        "Tilix" => Some("Tilix"),
        "Alacritty" => Some("Alacritty"),
        "Foot" => Some("Foot"),
        "Kitty" => Some("Kitty"),
        "WezTerm" => Some("WezTerm"),
        "Table Data" => Some("表数据"),
        "SQL Query" => Some("SQL 查询"),
        "Run" => Some("运行"),
        "This database has no tables" => Some("此数据库没有表"),
        "Loading table data..." => Some("正在加载表数据..."),
        "Select a table" => Some("选择一个表"),
        "Running query..." => Some("正在执行查询..."),
        "Type a SQL query and press Run" => Some("输入 SQL 查询后点击运行"),
        "Statement executed. No result rows." => Some("语句已执行，无结果行。"),
        "Only the first 200 rows are shown." => Some("仅显示前 200 行。"),
        "Filter tables" => Some("过滤表"),
        "No tables match" => Some("没有匹配的表"),
        "SQL query (Enter to run)" => Some("SQL 查询（回车执行）"),
        "SQLite database preview is not available" => Some("SQLite 数据库预览不可用"),
        _ => None,
    }
}

#[cfg(test)]
#[path = "localization/tests.rs"]
mod tests;
