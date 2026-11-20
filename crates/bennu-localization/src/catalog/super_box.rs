//! 超级输入框：模式、结果的种类、提示。

pub(super) const ENTRIES: &[(&str, &str)] = &[
    ("Super Box", "超级输入框"),
    // 模式（“搜索”“路径”在别的表里）
    ("New", "新建"),
    // 结果的种类
    ("Feature", "功能"),
    ("Favorite", "收藏"),
    // 不能执行时的说明
    ("“%1” cannot be used as a name", "“%1”不能当名字"),
    ("Archives cannot be changed here", "压缩包里不能改动"),
    ("New items cannot be made here", "这里不能新建"),
    (
        "Relative paths need a folder; type a path starting with / or ~",
        "这里没有当前文件夹，路径要以 / 或 ~ 开头",
    ),
    ("There is already an item named “%1” here", "这里已经有“%1”了"),
    (
        "Type a name; end it with / to make a folder",
        "输入名字；以 / 结尾是新建文件夹",
    ),
    // 框和设置页
    (
        "%1 opens the super box; Tab switches the mode while it is open.",
        "%1 打开超级输入框；打开后按 Tab 切换模式。",
    ),
    ("Tab switches the mode while the super box is open.", "超级输入框打开后按 Tab 切换模式。"),
    ("Super Box Opens In", "超级输入框默认模式"),
    ("Tab switches the mode", "Tab 切换模式"),
    ("Find a feature, action, setting or favorite", "找功能、动作、设置或收藏"),
    ("Go to a path; ~ is your home folder", "输入路径，~ 是主目录"),
    ("Name of the new file; end it with / for a folder", "新文件的名字；以 / 结尾是文件夹"),
    ("Nothing matches", "没有找到"),
    ("Type a path, or start with / or ~ in search.", "输入路径；在搜索模式里以 / 或 ~ 开头也行。"),
    (
        "Type to find what to do. Tab switches to going to a path or making a new item; g: and n: do it too.",
        "打字找要做的事。按 Tab 切到跳转路径或新建，打 g: 或 n: 也行。",
    ),
    ("Go to %1", "跳到 %1"),
    ("New file “%1”", "新建文件“%1”"),
    ("New folder “%1”", "新建文件夹“%1”"),
    // 文件、命令模式
    (
        "The search service is not running; start it on the Search settings page",
        "搜索服务没在运行，可以在“搜索”设置页启动",
    ),
    ("The index changed; type again", "索引刚变过，请再打一次"),
    ("Stopped", "已停止"),
    ("A command for your shell", "交给 shell 执行的命令"),
    ("Commands run in a folder; open one first", "命令要在文件夹里执行，先打开一个文件夹"),
    ("Ctrl+Enter runs it in the terminal", "Ctrl+Enter 在终端里执行"),
    ("Find files in the index", "在索引里找文件"),
    ("Finished, exit status 0", "完成，退出码 0"),
    ("Running %1", "正在执行 %1"),
    ("Stop", "停止"),
    ("The %1 selected items are its arguments", "选中的 %1 项作为参数"),
    (
        "Type a command. It runs in this folder with your shell, the selected items are its arguments (\"$@\", in fish $argv). Ctrl+Enter runs it in the terminal panel.",
        "输入命令。用你的 shell 在这个文件夹里执行，选中的项作为参数（\"$@\"，fish 里是 $argv）。Ctrl+Enter 在终端面板里执行。",
    ),
    ("Type what to look for; filters only narrow it down", "写上要找的词，筛选只是缩小范围"),
    (
        "Type what to look for. type:, time: and size: narrow it down, for example type:images time:7d size:>10m. Ctrl+Enter opens the search page.",
        "输入要找的词。用 类型:、时间:、大小: 缩小范围，例如 类型:图片 时间:7天 大小:>10m。Ctrl+Enter 打开搜索页。",
    ),
    ("Unknown time “%1”. You can use: %2", "不认识的时间“%1”，可以写：%2"),
    ("Unknown type “%1”. You can use: %2", "不认识的类型“%1”，可以写：%2"),
    ("Write sizes like >10m, <1g or 10m-1g", "大小写成 >10m、<1g 或 10m-1g"),
];
