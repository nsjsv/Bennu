# Bennu 文件选择后端（xdg-desktop-portal FileChooser）

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

## 验证

直接调用后端 D-Bus 服务（不经 xdg-desktop-portal 主进程），调用会
阻塞到窗口操作完成，直接返回 `(response, results)`：

    gdbus call --session \
        --dest org.freedesktop.impl.portal.desktop.bennu \
        --object-path /org/freedesktop/portal/desktop \
        --method org.freedesktop.impl.portal.FileChooser.OpenFile \
        "/org/freedesktop/portal/desktop/request/<sender>/<token>" \
        "" "" "标题" {}
