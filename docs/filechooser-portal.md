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

安装包只注册后端声明，不改变系统默认。要接管，编辑：

    ~/.config/xdg-desktop-portal/portals.conf

选择一种写法：

    # 全部 portal 接口默认用 bennu（不推荐，只做 FileChooser）
    [preferred]
    default=bennu

    # 推荐：仅文件选择用 bennu，其余保持系统默认
    [preferred]
    org.freedesktop.impl.portal.FileChooser=bennu

改完重启 xdg-desktop-portal：

    systemctl --user restart xdg-desktop-portal

## 回退

删除或注释掉 portals.conf 中的 bennu 行，再重启 xdg-desktop-portal，
系统自动回退到默认后端（如 xdg-desktop-portal-gtk）。

## 验证

    bash scripts/test-filechooser-portal.sh

脚本直接调用后端 D-Bus 服务核对协议（不经 portal 主进程）。
