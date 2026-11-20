//! 自定义动作：动作文件的错误说明、设置页、菜单、通知。

pub(super) const ENTRIES: &[(&str, &str)] = &[
    // 执行命令这一步
    ("Run Command", "执行命令"),
    ("Could not start %1: %2", "没能启动 %1：%2"),
    ("%1 exited with status %2", "%1 退出，状态码 %2"),
    ("%1 was stopped by signal %2", "%1 被信号 %2 停止"),
    // 设置的“动作”页
    (
        "Each .toml file in %1 is an action: it shows up in the context menu of the items it fits, and can have a shortcut on the Shortcuts page. Changes take effect as soon as a file is saved.",
        "%1 里的每个 .toml 文件是一个动作：它会出现在适用项目的右键菜单里，也可以在“快捷键”页给它绑键。文件一保存就生效。",
    ),
    ("No actions yet", "还没有动作"),
    (
        "Open the folder and add a file; the documentation has examples.",
        "打开文件夹放一个文件进去就行，说明文档里有例子。",
    ),
    ("Not Used", "没有使用"),
    ("These files are skipped until they are fixed.", "这些文件改好之前不会用。"),
    ("Open the Actions Folder", "打开动作文件夹"),
    ("%1, line %2", "%1，第 %2 行"),
    ("Everything", "所有项目"),
    // 动作的步骤和运行
    ("Actions", "动作"),
    (
        "“%1” does not apply to the selected items",
        "“%1”不适用于选中的项目",
    ),
    ("Convert to %1", "转换为 %1"),
    ("Compress to %1", "压缩为 %1"),
    ("Move the selected items to the trash", "把选中的项移到回收站"),
    ("Run %1", "执行 %1"),
    ("%1 · %2 (%3/%4)", "%1 · %2（%3/%4）"),
    ("The task was interrupted", "任务被中断了"),
    ("There is nothing left to work on", "没有可以处理的项了"),
    ("None of the items fit this step (%1)", "没有能交给这一步的项（%1）"),
    ("%1: cannot be converted", "%1：不能转换"),
    ("%1: cannot be converted to %2", "%1：不能转换为 %2"),
    (
        "This computer cannot convert to %1 (ffmpeg or its encoder is missing)",
        "这台电脑不能转换为 %1（缺少 ffmpeg 或它的编码器）",
    ),
    ("%1: not an archive", "%1：不是压缩包"),
    ("No free name for the archive in %1", "%1 里找不到空着的压缩包名字"),
    ("%1 is not a folder", "%1 不是文件夹"),
    ("%1: %2", "%1：%2"),
    ("Cannot create the folder %1", "没法新建文件夹 %1"),
    (
        "%1 items could not be processed, for example %2",
        "有 %1 项没处理成，比如 %2",
    ),
    // 动作文件写错时的说明
    (
        "A “{” is not closed (write {{ for a brace)",
        "“{” 没有配对的“}”（要写大括号本身请写 {{）",
    ),
    (
        "{%1} cannot be used here. You can use: %2",
        "这里不能用 {%1}。可以用：%2",
    ),
    (
        "The folder must start with /, ~/ or {dir}",
        "文件夹必须以 /、~/ 或 {dir} 开头",
    ),
    (
        "The command cannot be split into arguments: %1",
        "命令没法切成参数：%1",
    ),
    (
        "{files} must be an argument on its own",
        "{files} 必须单独作为一个参数",
    ),
    (
        "Use either {files} or the per-item variables ({file} {name} {stem} {ext} {dir}), not both",
        "{files} 和按每一项算的变量（{file} {name} {stem} {ext} {dir}）只能用一种",
    ),
    ("The action needs a name (name = \"…\")", "动作要有名字（name = \"…\"）"),
    ("The name cannot be empty", "名字不能是空的"),
    (
        "types cannot be empty; leave it out to show the action for everything",
        "types 不能是空的；要对所有项显示，就不写 types",
    ),
    (
        "“%1” is not a MIME type (for example video/*, image/png or inode/directory)",
        "“%1”不是 MIME 类型（比如 video/*、image/png 或 inode/directory）",
    ),
    (
        "The action needs at least one step ([[steps]])",
        "动作至少要有一步（[[steps]]）",
    ),
    ("trash has to be the last step", "trash 只能是最后一步"),
    ("Each step needs exactly one of: %1", "每一步只能写下面其中一个：%1"),
    ("Unknown format “%1”. You can use: %2", "不认识的格式“%1”。可以用：%2"),
    (
        "Unknown archive format “%1”. You can use: zip 7z tar.gz",
        "不认识的压缩格式“%1”。可以用：zip 7z tar.gz",
    ),
    ("extract is “here” or “folder”", "extract 只能是“here”或“folder”"),
    ("trash is “current” or “selected”", "trash 只能是“current”或“selected”"),
    ("copy_paths only takes true", "copy_paths 只能写 true"),
    (
        "notify is “none”, or a [notify] table with a title and an optional body",
        "notify 只能是“none”，或者是带 title（可加 body）的 [notify] 表",
    ),
    ("The file name has to be text", "文件名必须是文字"),
    ("It is not a regular file", "这不是普通文件"),
    ("It is too large to be an action file", "文件太大，不像动作文件"),
    ("It is not UTF-8 text", "这不是 UTF-8 文字"),
    (
        "It belongs to another user, so it is not used",
        "它属于别的用户，所以不使用",
    ),
    (
        "Other users can change it, so it is not used",
        "别的用户能修改它，所以不使用",
    ),
    // Dolphin 服务菜单
    ("The quotes or brackets in Exec do not match", "Exec 里的引号或括号没有配对"),
    ("Exec ends with a lone %", "Exec 以单独一个 % 结尾"),
    ("It has no [Desktop Entry] group", "没有 [Desktop Entry] 组"),
    (
        "It does not say which items it is for (MimeType=), so it is never shown",
        "没有写对哪些类型显示（MimeType=），所以永远不会显示",
    ),
    ("It has no actions (Actions=)", "没有动作（Actions=）"),
    ("It has no Exec line", "没有 Exec 行"),
    ("The group is missing", "缺少这个组"),
    (
        "It is not executable, so it is not used. If you trust it, run: chmod +x %1",
        "它没有执行权限，所以不使用。确认可信后运行：chmod +x %1",
    ),
    ("%1 failed: %2", "%1 失败：%2"),
    ("a line break", "换行"),
    ("a control character", "控制字符"),
    (
        "%1 contains %2, and the command hands the name on to another command line, so it was not run",
        "%1 里有%2，而这条命令会把文件名交给另一层命令行，为了安全没有执行",
    ),
    ("The program %1 is not executable", "程序 %1 没有执行权限"),
    ("Could not find the program “%1”", "找不到程序“%1”"),
    ("Dolphin Service Menus", "Dolphin 服务菜单"),
    (
        "Read from kio/servicemenus in the data folders, by the same rules as Dolphin. Ones turned off in Dolphin's settings are left out. They can have shortcuts too.",
        "从各数据目录的 kio/servicemenus 读取，规则和 Dolphin 一样。在 Dolphin 设置里关掉的不列出。它们也能绑快捷键。",
    ),
];
