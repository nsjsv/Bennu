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
        "%1 opens the super box; Shift+Tab switches the mode while it is open.",
        "%1 打开超级输入框；打开后按 Shift+Tab 切换模式。",
    ),
    (
        "Shift+Tab switches the mode while the super box is open.",
        "超级输入框打开后按 Shift+Tab 切换模式。",
    ),
    ("Super Box Opens In", "超级输入框默认模式"),
    ("Find a feature, action, setting or favorite", "找功能、动作、设置或收藏"),
    ("Go to a path; ~ is your home folder", "输入路径，~ 是主目录"),
    ("Name of the new file; end it with / for a folder", "新文件的名字；以 / 结尾是文件夹"),
    ("Nothing matches", "没有找到"),
    (
        "Type a path, or start with / or ~ in search. Tab completes it, → takes the faded suggestion.",
        "输入路径；在搜索模式里以 / 或 ~ 开头也行。Tab 补全，→ 接受淡色的建议。",
    ),
    (
        "Type to find what to do. Start with f:, g:, n: or s: to find files, go to a path, make a new item or run a command; Tab in an empty box lists them. *.mp4 | an action runs actions on files.",
        "打字找要做的事。以 f:、g:、n:、s: 开头是找文件、跳到路径、新建、执行命令；框空着按 Tab 列出它们。*.mp4 | 动作 对文件执行动作。",
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
    ("A command for your shell", "交给 shell 执行的命令"),
    ("Commands run in a folder; open one first", "命令要在文件夹里执行，先打开一个文件夹"),
    ("Find files in the index", "在索引里找文件"),
    ("Finished, exit status 0", "完成，退出码 0"),
    ("Running %1", "正在执行 %1"),
    ("Stop", "停止"),
    ("Move to Terminal", "转到终端"),
    ("Exit status %1", "退出码 %1"),
    ("Ended by signal: %1", "被信号结束：%1"),
    (
        "Type a command. It runs in this folder with your shell, the selected items are its arguments (\"$@\", in fish $argv). Tab completes, → takes the faded suggestion. Full-screen programs move to the terminal panel; Ctrl+Enter runs the command there.",
        "输入命令。用你的 shell 在这个文件夹里执行，选中的项作为参数（\"$@\"，fish 里是 $argv）。Tab 补全，→ 接受淡色的建议。全屏的程序会转到终端面板；Ctrl+Enter 直接在终端面板里执行。",
    ),
    ("Type what to look for; filters only narrow it down", "写上要找的词，筛选只是缩小范围"),
    (
        "Type what to look for. type:, time: and size: narrow it down, for example type:images time:7d size:>10m. Ctrl+Enter opens the search page.",
        "输入要找的词。用 类型:、时间:、大小: 缩小范围，例如 类型:图片 时间:7天 大小:>10m。Ctrl+Enter 打开搜索页。",
    ),
    ("Unknown time “%1”. You can use: %2", "不认识的时间“%1”，可以写：%2"),
    ("Unknown type “%1”. You can use: %2", "不认识的类型“%1”，可以写：%2"),
    ("Write sizes like >10m, <1g or 10m-1g", "大小写成 >10m、<1g 或 10m-1g"),
    ("%1 selected as arguments", "选中的 %1 项作为参数"),
    ("In %1", "在 %1"),
    ("Last run %1", "上次运行 %1"),
    // 管道
    (
        "Only names in this folder can be matched here; start with f: to look further",
        "这里只能挑当前文件夹里的名字；要找别处的，用 f: 开头",
    ),
    ("Write a step after each |", "每个 | 后面写一步"),
    ("“%1” has to be the last step", "“%1”只能是最后一步"),
    (
        "Unknown step “%1”. Write an action's name, or one of: %2",
        "不认识的步骤“%1”。写一个动作的名字，或者：%2",
    ),
    ("“%1” takes nothing after it", "“%1”后面不用写东西"),
    ("“%1” is no longer there", "“%1”已经不在了"),
    (
        "A pipe that ends with a service menu cannot be saved as an action",
        "以服务菜单结尾的管道不能存成动作",
    ),
    (
        "An action in Settings has unsaved changes; save or discard it first",
        "设置里有一个动作还没保存，先保存或者放弃它",
    ),
    (
        "Select the items first, or write which ones before |",
        "先选中要处理的项，或者在 | 前面写要哪些",
    ),
    ("No item matches", "没有对得上的项"),
    ("More than %1 items; narrow it down", "超过 %1 项了，缩小一下范围"),
    ("%1 items → %2", "%1 项 → %2"),
    ("Enter runs it · Ctrl+S saves it as an action", "回车执行 · Ctrl+S 存成动作"),
    // 计算、改名、发送
    ("This cannot be worked out", "算不出来"),
    ("Enter copies it", "回车复制"),
    ("Select the items to rename first", "先选中要改名的项"),
    ("Only items in the current folder can be renamed here", "这里只能改当前文件夹里的项"),
    ("Two items would both be named “%1”", "会有两项都叫“%1”"),
    ("The names stay the same", "名字没有变"),
    ("New name; {n} numbers several items", "新名字；几项时 {n} 是序号"),
    (
        "Type the new name. With several items selected it is a pattern: {n} is the number, {name}, {stem} and {ext} the old name, for example photo-{n}.{ext}.",
        "输入新名字。选中几项时是模板：{n} 是序号，{name}、{stem}、{ext} 是原来的名字，比如 照片-{n}.{ext}。",
    ),
    ("%1 items in all", "一共 %1 项"),
    ("%1 selected", "选中了 %1 项"),
    ("Select what to send first", "先选中要发送的项"),
    (
        "No device to send to: turn on transfers, or wait for a device nearby",
        "没有能发的设备：打开传输，或者等附近的设备出现",
    ),
    ("Expand from the first column", "从第一栏往下展开"),
    ("Its folder as the first column", "所在文件夹作第一栏"),
    ("Open the file", "直接打开文件"),
    ("Opening a Found File in Columns View", "多栏视图里打开找到的文件"),
    ("Other views always open its folder and select it.", "其他视图总是进入所在文件夹并选中它。"),
];
