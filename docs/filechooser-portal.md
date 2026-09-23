# Bennu 文件选择后端

`bennu-portal` 是独立的 xdg-desktop-portal FileChooser 后端进程。接管系统
文件选择框后，浏览器、Flatpak 应用等走 portal 协议的软件弹出"选文件 /
另存为"时显示的是 Bennu 的选择窗口。

## 支持范围

- `OpenFile`：单选/多选（`multiple`）、选文件夹（`directory`）、文件类型
  过滤（`filters` / `current_filter`）、自定义确认文案（`accept_label`）
- `SaveFile`：默认文件名（`current_name`）、覆盖二次确认
- `SaveFiles`：不支持（显式返回 NotSupported）
- 起始目录优先级：调用方 `current_folder` > 上次记忆（`~/.config/bennu/portal.toml`）
  > 主目录

## 手动接管（安装后不会自动生效）

安装包只注册后端声明，不改变系统默认。配置文件按桌面环境命名，
放在 `~/.config/xdg-desktop-portal/`（用户级，覆盖发行版同名文件）：

| 桌面环境 | 配置文件名 |
|---|---|
| GNOME | `gnome-portals.conf` |
| KDE Plasma | `kde-portals.conf` |
| Hyprland / Sway / niri | `hyprland-portals.conf` / `sway-portals.conf` / `niri-portals.conf` |
| Xfce / MATE / LXQt / Budgie / COSMIC | `xfce-portals.conf` 等同名规则 |
| 其他 / 未知（i3、Openbox 等） | `portals.conf` |

内容统一（推荐只接管文件选择，`default` 保持系统原有回退链）：

    [preferred]
    org.freedesktop.impl.portal.FileChooser=bennu;

注意：

- `$XDG_CURRENT_DESKTOP` 决定读取哪个文件名；同名键重复时取最后一行。
- 改完重启 xdg-desktop-portal：`systemctl --user restart xdg-desktop-portal`。
- 需要 xdg-desktop-portal ≥ 1.17（`portals.conf` 机制；更老版本只认
  `.portal` 的 `UseIn` 字段，本后端故意不写）。

## 应用侧是否走 portal（与桌面环境无关）

- Flatpak / Snap 应用：强制走 portal，零配置生效。
- Firefox：非沙箱默认不走；`about:config` 设
  `widget.use-xdg-desktop-portal.file-picker = 1`，或以 `GTK_USE_PORTAL=1`
  启动。
- Chromium / Electron：Wayland 下默认走；X11 需手动开启对应 flag。
- 原生 GTK 应用：设 `GTK_USE_PORTAL=1` 强制走（影响全部 GTK 对话框）。
- Qt 应用：非 Flatpak 默认不走 portal（Qt 6.6+ 才有 portal 对话框）。

## 回退

删除或注释掉 portals.conf 中的 bennu 行，再重启 xdg-desktop-portal，
系统自动回退到默认后端（如 xdg-desktop-portal-gtk）。

## 常驻生命周期

`bennu-portal` 常驻运行：首次拉起后不再空闲自退，后续唤出直接复用进程，
避免每次冷启动约 2 秒的首帧延迟（代价是约 20MB 常驻内存，与
xdg-desktop-portal-gtk 的常驻形态一致）。

### 安装包版

- 安装包提供 systemd user unit `bennu-portal.service`
  （`/usr/lib/systemd/user/`，`Type=dbus`），随图形会话启停
  （`PartOf=graphical-session.target`）。
- D-Bus service 文件中的 `SystemdService=bennu-portal.service` 把
  activation 交给 systemd 托管：首次文件选择请求拉起 unit，之后常驻到
  会话结束。

### 开发环境（scripts/install-bennu-dev.sh）

脚本安装 dev 专用 unit `bennu-portal-dev.service`（命名对齐
`bennu-search-dev.service` 惯例，避免遮蔽系统安装包的同名 unit）：

- `ExecStart=%h/.local/bin/bennu-portal`，二进制仍由开发者手动维护；
- 用户级 D-Bus service 文件
  （`~/.local/share/dbus-1/services/org.freedesktop.impl.portal.desktop.bennu.service`）
  由脚本幂等维护 `SystemdService=bennu-portal-dev.service` 一行，
  已有手写内容（如自定义 `Exec=`）保持不变；
- portal 进程在启动第一步设定渲染链（核显 → 独显 → 软件渲染）：
  `ICED_BACKEND=wgpu,tiny-skia`，并把 wgpu 首选钉在驱动显示器的 GPU（与主软件
  DisplayGpu 偏好同配方：`WGPU_POWER_PREF` + `MESA_VK_DEVICE_SELECT` +
  `VK_LOADER_DRIVERS_SELECT`，检测逻辑在独立 crate `display-renderer`，
  纯 sysfs 读取）。已存在的同名环境变量不覆盖，保留运维/实验入口。共享主题库
  renderer-neutral，renderer 选择由各进程自己负责，避免 systemd、
  D-Bus activation 和手动启动出现不同结果；
- `systemctl --user enable` 挂到 `graphical-session.target`，登录自启。

这个设置只选择 portal 的 renderer，不创建隐藏窗口，也不改变每次请求创建真实选择
窗口的语义。安装包和 dev unit 不重复声明环境变量，避免 renderer 选择分散在多个
启动入口。

### 升级流程

二进制更新后必须重启服务，否则旧进程继续用旧代码服务请求：

    # 开发环境：先构建并复制新二进制
    cargo build --release -p portal-backend
    cp target/release/bennu-portal ~/.local/bin/bennu-portal
    systemctl --user restart bennu-portal-dev

    # 安装包版：deb/rpm/AUR 升级后
    systemctl --user restart bennu-portal

### 排障

- 查日志：`journalctl --user -u bennu-portal`（开发环境 unit 名为
  `bennu-portal-dev`）。
- bus 名 `org.freedesktop.impl.portal.desktop.bennu` 被残留进程占用导致
  新进程起不来：先 `systemctl --user stop bennu-portal`，再手动前台运行
  `/usr/bin/bennu-portal`（开发环境为 `~/.local/bin/bennu-portal`）
  定位原因。

## 验证

直接调用后端 D-Bus 服务（不经 xdg-desktop-portal 主进程），调用会
阻塞到窗口操作完成，直接返回 `(response, results)`：

    gdbus call --session \
        --dest org.freedesktop.impl.portal.desktop.bennu \
        --object-path /org/freedesktop/portal/desktop \
        --method org.freedesktop.impl.portal.FileChooser.OpenFile \
        "/org/freedesktop/portal/desktop/request/<sender>/<token>" \
        "" "" "标题" {}
